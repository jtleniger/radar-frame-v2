use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use clap::{Args, Parser, Subcommand};

use crate::api::{self, AppState};
use crate::clock::{Clock, SystemClock};
use crate::config::Config;
use crate::engine::{Engine, Renderer};
use crate::error::Result;
use crate::modes::{self, Deps};
use crate::paths::Paths;
use crate::setup;
use crate::sources::mock::MockProvider;
use crate::sources::nexrad::S3Radar;
use crate::sources::online::OnlineProvider;
use crate::sources::osm::Overpass;
use crate::sources::{ForecastProvider, RadarSource};
use crate::state::State;

#[derive(Parser)]
#[command(name = "radar-frame", version, about = "Radar frame HTTP service")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP service
    Serve(Serve),
    /// Create the data directory, palette, and street mask
    Setup(Setup),
    /// Render frames for inspection
    #[command(subcommand)]
    Render(RenderCommand),
    /// Compose e-ink frames and write the palette
    #[command(subcommand)]
    View(ViewCommand),
}

#[derive(Subcommand)]
enum RenderCommand {
    /// Render the radar and e-ink frames from a local Archive II volume
    Volume(RenderVolume),
    /// Render the storm e-ink frame from the latest NEXRAD volume
    Storm(MockArgs),
    /// Render the clear-sky e-ink frame
    Clear(MockArgs),
    /// Render a radar frame straight from an Archive II volume
    Archive(ArchiveArgs),
    /// Rasterize OSM highway ways into an 8-bit street mask PNG
    Streets(StreetsArgs),
}

#[derive(Subcommand)]
enum ViewCommand {
    /// Compose the storm frame from a radar frame and active alerts
    Storm(StormArgs),
    /// Compose the clear-sky frame from current, hourly, and daily forecasts
    Clear(ClearArgs),
    /// Write the seven-colour e-ink palette PNG
    Palette(PaletteArgs),
}

#[derive(Args)]
struct Serve {
    #[arg(long, default_value = "0.0.0.0:8000")]
    bind: SocketAddr,
    /// Use mock forecast data
    #[arg(long)]
    mock: bool,
}

#[derive(Args)]
struct Setup {
    /// Re-download and re-rasterize the street mask
    #[arg(long)]
    force_streets: bool,
}

#[derive(Args)]
struct RenderVolume {
    /// Archive II volume, plain or gzip-compressed
    #[arg(long)]
    file: PathBuf,
    /// Minimum product value to render
    #[arg(long, default_value_t = 10.0)]
    minimum: f64,
    /// Re-download and re-rasterize the street mask
    #[arg(long)]
    force_streets: bool,
}

#[derive(Args)]
struct MockArgs {
    /// Use mock forecast data
    #[arg(long)]
    mock: bool,
}

#[derive(Args)]
struct ArchiveArgs {
    #[arg(long, allow_hyphen_values = true)]
    min_lon: f64,
    #[arg(long, allow_hyphen_values = true)]
    min_lat: f64,
    #[arg(long, allow_hyphen_values = true)]
    max_lon: f64,
    #[arg(long, allow_hyphen_values = true)]
    max_lat: f64,
    #[arg(long)]
    width: u32,
    #[arg(long)]
    height: u32,
    /// NEXRAD Archive II volume, plain or gzip-compressed
    #[arg(long)]
    archive: PathBuf,
    /// Minimum product value to include
    #[arg(long, allow_hyphen_values = true)]
    minimum: Option<f32>,
    /// Maximum product value to include
    #[arg(long, allow_hyphen_values = true)]
    maximum: Option<f32>,
    /// Elevation or range of elevations: N or N-M (inclusive)
    #[arg(short = 'e', long, default_value = "1-3")]
    elevations: String,
    /// Optional text color table; defaults to the embedded gdal-colors table
    #[arg(long)]
    colors: Option<PathBuf>,
    /// 8-bit grayscale street mask, same dimensions as the frame
    #[arg(long)]
    streets: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
struct StreetsArgs {
    #[arg(long, allow_hyphen_values = true)]
    min_lon: f64,
    #[arg(long, allow_hyphen_values = true)]
    min_lat: f64,
    #[arg(long, allow_hyphen_values = true)]
    max_lon: f64,
    #[arg(long, allow_hyphen_values = true)]
    max_lat: f64,
    #[arg(long)]
    width: u32,
    #[arg(long)]
    height: u32,
    /// Overpass OSM XML file
    #[arg(long)]
    input: PathBuf,
    /// Output grayscale PNG path
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
struct FrameArgs {
    /// Palette PNG written by `radar-frame view palette`
    #[arg(long)]
    palette: PathBuf,
    #[arg(long)]
    output: PathBuf,
    /// IANA time zone used for the info bar
    #[arg(long)]
    timezone: String,
    /// Override the current time (RFC 3339), for tests and fixtures
    #[arg(long)]
    now: Option<String>,
}

#[derive(Args)]
struct StormArgs {
    /// Radar frame PNG (800x444)
    #[arg(long)]
    radar: PathBuf,
    #[command(flatten)]
    frame: FrameArgs,
}

#[derive(Args)]
struct ClearArgs {
    #[command(flatten)]
    frame: FrameArgs,
}

#[derive(Args)]
struct PaletteArgs {
    #[arg(long)]
    output: PathBuf,
}

fn context() -> Result<(Paths, Config)> {
    let root = std::env::current_dir()
        .map_err(|e| crate::error::Error::new(format!("current directory: {e}")))?;
    let paths = Paths::new(root);
    let config = Config::load(&paths.config())?;
    Ok((paths, config))
}

fn init_logging(paths: &Paths, config: &Config) {
    let level = config.get_opt("logging", "level").unwrap_or("INFO");
    crate::logging::init(paths, level);
}

fn http_client(config: &Config) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(config.user_agent())
        .gzip(true)
        .timeout(Duration::from_secs(30))
        .build()?)
}

fn provider(config: &Config, client: &reqwest::Client, mock: bool) -> Arc<dyn ForecastProvider> {
    if mock {
        Arc::new(MockProvider::new())
    } else {
        Arc::new(OnlineProvider::new(client.clone(), config.clone()))
    }
}

async fn serve(args: Serve) -> Result<()> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);

    setup::create_data_dir(&paths)?;
    setup::create_palette(&paths)?;

    let client = http_client(&config)?;
    setup::create_streets(&paths, &config, &Overpass::new(client.clone()), false).await?;

    let radar: Arc<dyn RadarSource> = Arc::new(S3Radar::new(client.clone()));
    let engine: Arc<dyn Renderer> = Arc::new(Engine::new(paths.clone(), &config)?);
    let state = Arc::new(AppState::new(
        config.clone(),
        paths,
        Arc::new(SystemClock),
        provider(&config, &client, args.mock),
        radar,
        engine,
    )?);

    api::serve(state, args.bind).await
}

async fn setup_command(args: Setup) -> Result<()> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);

    setup::create_data_dir(&paths)?;
    setup::create_palette(&paths)?;

    let client = http_client(&config)?;
    setup::create_streets(&paths, &config, &Overpass::new(client), args.force_streets).await
}

async fn render_volume(args: RenderVolume) -> Result<()> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);

    if !args.file.is_file() {
        return Err(crate::error::Error::new(format!(
            "{}: no such file",
            args.file.display()
        )));
    }

    setup::create_data_dir(&paths)?;
    setup::create_palette(&paths)?;

    let client = http_client(&config)?;
    setup::create_streets(&paths, &config, &Overpass::new(client), args.force_streets).await?;

    let engine = Arc::new(Engine::new(paths.clone(), &config)?);
    let volume = args.file.clone();
    let radar_engine = Arc::clone(&engine);
    tokio::task::spawn_blocking(move || radar_engine.render_radar(&volume, args.minimum))
        .await
        .map_err(|e| crate::error::Error::new(format!("render task: {e}")))??;

    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || engine.render_storm(&[], now))
        .await
        .map_err(|e| crate::error::Error::new(format!("render task: {e}")))??;

    println!("radar frame: {}", paths.radar_img().display());
    println!("e-ink frame: {}", paths.output_img().display());
    Ok(())
}

async fn render_clear(args: MockArgs) -> Result<()> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);

    let client = http_client(&config)?;
    let provider = provider(&config, &client, args.mock);
    let forecast = provider.forecast().await?;
    let alerts = provider.alerts().await?;

    let engine = Engine::new(paths, &config)?;
    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || engine.render_clear(&forecast, &alerts, now))
        .await
        .map_err(|e| crate::error::Error::new(format!("render task: {e}")))??;

    Ok(())
}

async fn render_storm(args: MockArgs) -> Result<()> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);

    let client = http_client(&config)?;
    let radar: Arc<dyn RadarSource> = Arc::new(S3Radar::new(client.clone()));
    let engine: Arc<dyn Renderer> = Arc::new(Engine::new(paths.clone(), &config)?);
    let provider = provider(&config, &client, args.mock);

    let mut state = State::new();
    let deps = Deps {
        paths: &paths,
        config: &config,
        engine: &engine,
        provider: provider.as_ref(),
        radar: radar.as_ref(),
        now: SystemClock.now(),
    };
    modes::run(crate::state::Mode::Storm, &deps, &mut state)
        .await
        .map(|_| ())
}

fn render_archive(args: ArchiveArgs) -> Result<()> {
    let elevations =
        crate::nexrad::elevations::parse_elevations(&args.elevations).ok_or_else(|| {
            crate::error::Error::new(format!("invalid elevations {}", args.elevations))
        })?;

    crate::render::archive::render_archive(&crate::render::archive::Options {
        min_lon: args.min_lon,
        min_lat: args.min_lat,
        max_lon: args.max_lon,
        max_lat: args.max_lat,
        width: args.width as usize,
        height: args.height as usize,
        archive: args.archive,
        elevations,
        minimum: args.minimum,
        maximum: args.maximum,
        colors: args.colors,
        streets: args.streets,
        output: args.output,
    })?;
    Ok(())
}

fn render_streets(args: StreetsArgs) -> Result<()> {
    crate::render::streets::render_streets(&crate::render::streets::Options {
        min_lon: args.min_lon,
        min_lat: args.min_lat,
        max_lon: args.max_lon,
        max_lat: args.max_lat,
        width: args.width as usize,
        height: args.height as usize,
        input: args.input,
        output: args.output,
    })?;
    Ok(())
}

fn now(value: Option<String>) -> Result<DateTime<Utc>> {
    match value {
        Some(value) => DateTime::parse_from_rfc3339(&value)
            .map(|time| time.with_timezone(&Utc))
            .map_err(|e| crate::error::Error::new(format!("{value}: {e}"))),
        None => Ok(Utc::now()),
    }
}

fn timezone(value: &str) -> Result<Tz> {
    value
        .parse()
        .map_err(|_| crate::error::Error::new(format!("unknown time zone {value}")))
}

fn stdin_json<T: serde::de::DeserializeOwned>() -> Result<T> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(crate::error::Error::from)?;
    serde_json::from_str(&input).map_err(crate::error::Error::from)
}

fn view_storm(args: StormArgs) -> Result<()> {
    let alerts: Vec<crate::view::forecast::Alert> = stdin_json()?;
    let radar = crate::view::image::Image::from_png(&args.radar)?;
    let palette = crate::view::palette::Palette::from_png(&args.frame.palette)?;
    let image = crate::view::views::render_storm(
        &crate::view::font::Fonts::new(),
        &radar,
        &alerts,
        now(args.frame.now)?,
        timezone(&args.frame.timezone)?,
        &palette,
    )?;
    image.write_png(&args.frame.output)?;
    Ok(())
}

fn view_clear(args: ClearArgs) -> Result<()> {
    let input: crate::view::forecast::ClearInput = stdin_json()?;
    let palette = crate::view::palette::Palette::from_png(&args.frame.palette)?;
    let image = crate::view::views::render_clear(
        &crate::view::font::Fonts::new(),
        &input.forecast,
        &input.alerts,
        now(args.frame.now)?,
        timezone(&args.frame.timezone)?,
        &palette,
    )?;
    image.write_png(&args.frame.output)?;
    Ok(())
}

fn view_palette(args: PaletteArgs) -> Result<()> {
    crate::view::palette::write_eink_png(&args.output)?;
    Ok(())
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Serve(args) => serve(args).await,
        Command::Setup(args) => setup_command(args).await,
        Command::Render(RenderCommand::Volume(args)) => render_volume(args).await,
        Command::Render(RenderCommand::Storm(args)) => render_storm(args).await,
        Command::Render(RenderCommand::Clear(args)) => render_clear(args).await,
        Command::Render(RenderCommand::Archive(args)) => render_archive(args),
        Command::Render(RenderCommand::Streets(args)) => render_streets(args),
        Command::View(ViewCommand::Storm(args)) => view_storm(args),
        Command::View(ViewCommand::Clear(args)) => view_clear(args),
        Command::View(ViewCommand::Palette(args)) => view_palette(args),
    }
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("radar-frame: runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("radar-frame: {error}");
            ExitCode::FAILURE
        }
    }
}
