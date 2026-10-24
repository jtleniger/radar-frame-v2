//! `radar-frame check`: exercise one data path end to end, live or mocked.
//!
//! Each action is a self-contained diagnostic: it prints what it fetched and
//! where it wrote the frame, so a failure names the layer that broke.

use std::sync::Arc;

use clap::{Args, Subcommand};

use crate::config::{Config, Paths, Settings};
use crate::error::{Error, Result};
use crate::service::clock::{Clock, SystemClock};
use crate::service::engine::{Engine, Renderer};
use crate::service::modes::{self, Deps};
use crate::service::setup;
use crate::sources::nexrad::S3Radar;
use crate::sources::{RadarSource, Sources};
use crate::state::{Mode, State};
use crate::telemetry::metrics::Metrics;
use crate::view::forecast::{Alert, AlertLevel};

use super::{context, http_client, init_logging, sources};

#[derive(Subcommand)]
pub enum CheckCommand {
    /// Fetch the forecast and alerts and render the clear-sky frame
    Clear(CheckArgs),
    /// Download the latest NEXRAD volume and render the storm frame
    Radar(CheckArgs),
    /// Fetch the active NWS alerts and print them
    Warnings(CheckArgs),
}

#[derive(Args)]
pub struct CheckArgs {
    /// Use the built-in mock forecast and alerts instead of upstream data
    #[arg(long)]
    mock: bool,
}

pub async fn run(command: CheckCommand) -> Result<()> {
    match command {
        CheckCommand::Clear(args) => check_clear(args).await,
        CheckCommand::Radar(args) => check_radar(args).await,
        CheckCommand::Warnings(args) => check_warnings(args).await,
    }
}

/// Settings plus a client and providers, shared by every check action.
struct Data {
    paths: Paths,
    config: Config,
    client: reqwest::Client,
    sources: Sources,
}

fn data(mock: bool) -> Result<Data> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);
    let client = http_client(&config)?;
    let sources = sources(&config, &client, mock);
    Ok(Data {
        paths,
        config,
        client,
        sources,
    })
}

async fn check_clear(args: CheckArgs) -> Result<()> {
    let data = data(args.mock)?;
    let settings = Settings::from_config(&data.config)?;
    setup::create_data_dir(&data.paths)?;
    setup::create_palette(&data.paths)?;

    let forecast = data.sources.forecast.forecast().await?;
    let alerts = data.sources.alerts.alerts().await?;
    let now = SystemClock.now();

    let engine = Engine::new(data.paths.clone(), &settings);
    let rendered = alerts.clone();
    let input = forecast.clone();
    tokio::task::spawn_blocking(move || engine.render_clear(&input, &rendered, now))
        .await
        .map_err(|e| Error::render(format!("render task: {e}")))??;

    println!(
        "forecast: {:.1} F, {} hourly, {} daily",
        forecast.current.temp_f,
        forecast.hourly.len(),
        forecast.daily.len()
    );
    print_alerts(&alerts, now);
    println!("frame: {}", data.paths.output_img().display());
    Ok(())
}

async fn check_radar(args: CheckArgs) -> Result<()> {
    let data = data(args.mock)?;
    let settings = Settings::from_config(&data.config)?;
    setup::create_data_dir(&data.paths)?;
    setup::create_palette(&data.paths)?;

    let radar: Arc<dyn RadarSource> = Arc::new(S3Radar::new(data.client.clone()));
    let engine: Arc<dyn Renderer> = Arc::new(Engine::new(data.paths.clone(), &settings));
    let now = SystemClock.now();
    let metrics = Metrics::new(now);
    let mut state = State::new();
    let deps = Deps {
        paths: &data.paths,
        settings: &settings,
        engine: &engine,
        forecast: data.sources.forecast.as_ref(),
        alerts: data.sources.alerts.as_ref(),
        radar: radar.as_ref(),
        metrics: &metrics,
        now,
    };

    let cycle = modes::run(Mode::Storm, &deps, &mut state).await?;

    println!(
        "volume: {}",
        cycle.volume_key.as_deref().unwrap_or("(last good)")
    );
    println!(
        "radials: {}, gates: {}, elevations: {:?}",
        cycle.radials, cycle.gates, cycle.elevations
    );
    println!("warnings: {} active", cycle.alerts);
    println!("render: {} ms", cycle.render_ms);
    if let Some(reason) = cycle.degraded {
        println!("degraded: {reason}");
    }
    println!("radar frame: {}", data.paths.radar_img().display());
    println!("frame: {}", data.paths.output_img().display());
    Ok(())
}

async fn check_warnings(args: CheckArgs) -> Result<()> {
    let data = data(args.mock)?;
    let alerts = data.sources.alerts.alerts().await?;
    print_alerts(&alerts, SystemClock.now());
    Ok(())
}

fn print_alerts(alerts: &[Alert], now: chrono::DateTime<chrono::Utc>) {
    let active = alerts.iter().filter(|alert| alert.is_active(now)).count();
    println!("warnings: {active} active of {}", alerts.len());

    for alert in alerts {
        println!(
            "  [{}] {} ({}) {} .. {}",
            level_name(alert.level),
            alert.event,
            alert.status,
            alert.effective.to_rfc3339(),
            alert.expires.to_rfc3339()
        );
    }
}

fn level_name(level: AlertLevel) -> &'static str {
    match level {
        AlertLevel::Info => "info",
        AlertLevel::Watch => "watch",
        AlertLevel::Warning => "warning",
        AlertLevel::Emergency => "emergency",
    }
}
