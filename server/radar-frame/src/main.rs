use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use radar_frame::api::{self, AppState};
use radar_frame::clock::{Clock, SystemClock};
use radar_frame::config::Config;
use radar_frame::engine::{Engine, Renderer};
use radar_frame::error::Result;
use radar_frame::modes::{self, Deps};
use radar_frame::paths::Paths;
use radar_frame::setup;
use radar_frame::sources::mock::MockProvider;
use radar_frame::sources::nexrad::S3Radar;
use radar_frame::sources::online::OnlineProvider;
use radar_frame::sources::osm::Overpass;
use radar_frame::sources::{ForecastProvider, RadarSource};
use radar_frame::state::State;

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
    /// Render the radar and e-ink frames from a local Archive II volume
    RenderVolume(RenderVolume),
    /// Render the clear-sky e-ink frame
    RenderClear(MockArgs),
    /// Render the storm e-ink frame from the latest NEXRAD volume
    RenderStorm(MockArgs),
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

fn context() -> Result<(Paths, Config)> {
    let root = std::env::current_dir()
        .map_err(|e| radar_frame::error::Error::new(format!("current directory: {e}")))?;
    let paths = Paths::new(root);
    let config = Config::load(&paths.config())?;
    Ok((paths, config))
}

fn init_logging(paths: &Paths, config: &Config) {
    let level = config.get_opt("logging", "level").unwrap_or("INFO");
    radar_frame::logging::init(paths, level);
}

fn http_client(config: &Config) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(config.user_agent())
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
        return Err(radar_frame::error::Error::new(format!(
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
        .map_err(|e| radar_frame::error::Error::new(format!("render task: {e}")))??;

    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || engine.render_storm(&[], now))
        .await
        .map_err(|e| radar_frame::error::Error::new(format!("render task: {e}")))??;

    println!("radar frame: {}", paths.radar_img().display());
    println!("e-ink frame: {}", paths.output_img().display());
    Ok(())
}

async fn render_clear(args: MockArgs) -> Result<()> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);

    let client = http_client(&config)?;
    let provider = provider(&config, &client, args.mock);
    let forecast = provider.forecast().await?.clone();
    let alerts = provider.alerts().await?;

    let engine = Engine::new(paths, &config)?;
    let now = SystemClock.now();
    tokio::task::spawn_blocking(move || engine.render_clear(&forecast, alerts, now))
        .await
        .map_err(|e| radar_frame::error::Error::new(format!("render task: {e}")))??;

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
    modes::run(radar_frame::state::Mode::Storm, &deps, &mut state).await
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Serve(args) => serve(args).await,
        Command::Setup(args) => setup_command(args).await,
        Command::RenderVolume(args) => render_volume(args).await,
        Command::RenderClear(args) => render_clear(args).await,
        Command::RenderStorm(args) => render_storm(args).await,
    }
}

fn main() -> ExitCode {
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
