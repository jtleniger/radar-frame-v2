//! Command-line parsing and dispatch. Each command group lives in its own
//! module; this file owns the shared setup helpers and the entry point.

mod demo;
mod render;
mod serve;
mod view;

use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};

use crate::config::{Config, Paths};
use crate::error::Result;
use crate::service::setup;
use crate::sources::mock::MockProvider;
use crate::sources::online::OnlineProvider;
use crate::sources::osm::Overpass;
use crate::sources::Sources;

#[derive(Parser)]
#[command(name = "radar-frame", version, about = "Radar frame HTTP service")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP service
    Serve(serve::Serve),
    /// Create the data directory, palette, and street mask
    Setup(Setup),
    /// Exercise a live or mocked data path end to end
    #[command(subcommand)]
    Demo(demo::DemoCommand),
    /// Low-level rendering and fixture tools
    #[command(subcommand)]
    Render(render::RenderCommand),
    /// Compose e-ink frames and write the palette
    #[command(subcommand)]
    View(view::ViewCommand),
}

#[derive(Args)]
struct Setup {
    /// Re-download and re-rasterize the street mask
    #[arg(long)]
    force_streets: bool,
}

pub(super) fn context() -> Result<(Paths, Config)> {
    let root = std::env::current_dir()
        .map_err(|e| crate::error::Error::io(format!("current directory: {e}")))?;
    let paths = Paths::new(root);
    let config = Config::load(&paths.config())?;
    Ok((paths, config))
}

pub(super) fn init_logging(paths: &Paths, config: &Config) {
    let level = config.get_opt("logging", "level").unwrap_or("INFO");
    crate::telemetry::logging::init(paths, level);
}

pub(super) fn http_client(config: &Config) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(config.user_agent())
        .gzip(true)
        .connect_timeout(crate::sources::http::CONNECT_TIMEOUT)
        .timeout(crate::sources::http::REQUEST_TIMEOUT)
        .build()?)
}

pub(super) fn sources(config: &Config, client: &reqwest::Client, mock: bool) -> Sources {
    if mock {
        Sources::from_provider(Arc::new(MockProvider::demo()))
    } else {
        Sources::from_provider(Arc::new(OnlineProvider::new(
            client.clone(),
            config.clone(),
        )))
    }
}

async fn setup_command(args: Setup) -> Result<()> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);

    setup::create_data_dir(&paths)?;
    setup::create_palette(&paths)?;

    let client = http_client(&config)?;
    setup::create_streets(&paths, &config, &Overpass::new(client), args.force_streets).await
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Serve(args) => serve::serve(args).await,
        Command::Setup(args) => setup_command(args).await,
        Command::Demo(command) => demo::run(command).await,
        Command::Render(command) => render::run(command).await,
        Command::View(command) => view::run(command),
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
