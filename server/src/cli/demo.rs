//! `radar-frame demo`: exercise one data path end to end, live or mocked.
//!
//! Each action is a self-contained diagnostic: it prints what it fetched and
//! where it wrote the frame, so a failure names the layer that broke.

use std::sync::Arc;

use clap::{Args, Subcommand};

use crate::config::{Config, Paths, Settings};
use crate::error::{Error, Result};
use crate::service::clock::{Clock, SystemClock};
use crate::service::modes::{self, Services};
use crate::service::renderer::{FrameRenderer, Renderer};
use crate::service::setup;
use crate::sources::mock::MockRadar;
use crate::sources::nexrad::S3Radar;
use crate::sources::{RadarSource, Sources};
use crate::state::{Mode, State};
use crate::telemetry::metrics::Metrics;
use crate::view::forecast::{Alert, AlertLevel};

use super::{context, http_client, init_logging, sources};

#[derive(Subcommand)]
pub enum DemoCommand {
    /// Fetch the forecast and alerts and render the clear-sky frame
    Clear(DemoArgs),
    /// Download the latest NEXRAD volume and render the storm frame
    Radar(DemoArgs),
    /// Fetch the active NWS alerts and print them
    Warnings(DemoArgs),
}

#[derive(Args)]
pub struct DemoArgs {
    /// Use the built-in mock forecast, alerts, and radar volume instead of
    /// upstream data
    #[arg(long)]
    mock: bool,
}

pub async fn run(command: DemoCommand) -> Result<()> {
    match command {
        DemoCommand::Clear(args) => run_clear(args).await,
        DemoCommand::Radar(args) => run_radar(args).await,
        DemoCommand::Warnings(args) => run_warnings(args).await,
    }
}

/// Settings plus a client and providers, shared by every demo action.
struct DemoEnvironment {
    paths: Paths,
    config: Config,
    client: reqwest::Client,
    sources: Sources,
}

fn demo_environment(mock: bool) -> Result<DemoEnvironment> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);
    let client = http_client(&config)?;
    let sources = sources(&config, &client, mock);
    Ok(DemoEnvironment {
        paths,
        config,
        client,
        sources,
    })
}

async fn run_clear(args: DemoArgs) -> Result<()> {
    let environment = demo_environment(args.mock)?;
    let settings = Settings::from_config(&environment.config)?;
    setup::create_data_dir(&environment.paths)?;
    setup::create_palette(&environment.paths)?;

    let forecast = environment.sources.forecast.forecast().await?;
    let alerts = environment.sources.alerts.alerts().await?;
    let now = SystemClock.now();

    let renderer = FrameRenderer::new(environment.paths.clone(), &settings);
    let rendered = alerts.clone();
    let input = forecast.clone();
    tokio::task::spawn_blocking(move || renderer.render_clear(&input, &rendered, now))
        .await
        .map_err(|e| Error::render(format!("render task: {e}")))??;

    println!(
        "forecast: {:.1} F, {} hourly, {} daily",
        forecast.current.temp_f,
        forecast.hourly.len(),
        forecast.daily.len()
    );
    print_alerts(&alerts, now);
    println!("frame: {}", environment.paths.output_img().display());
    Ok(())
}

async fn run_radar(args: DemoArgs) -> Result<()> {
    let environment = demo_environment(args.mock)?;
    let settings = Settings::from_config(&environment.config)?;
    setup::create_data_dir(&environment.paths)?;
    setup::create_palette(&environment.paths)?;

    let radar: Arc<dyn RadarSource> = if args.mock {
        Arc::new(MockRadar)
    } else {
        Arc::new(S3Radar::new(environment.client.clone()))
    };
    let renderer: Arc<dyn Renderer> =
        Arc::new(FrameRenderer::new(environment.paths.clone(), &settings));
    let now = SystemClock.now();
    let metrics = Metrics::new(now);
    let mut state = State::new();
    let services = Services {
        paths: &environment.paths,
        settings: &settings,
        renderer: &renderer,
        forecast: environment.sources.forecast.as_ref(),
        alerts: environment.sources.alerts.as_ref(),
        radar: radar.as_ref(),
        metrics: &metrics,
        now,
    };

    let cycle = modes::run(Mode::Storm, &services, &mut state).await?;

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
    println!("radar frame: {}", environment.paths.radar_img().display());
    println!("frame: {}", environment.paths.output_img().display());
    Ok(())
}

async fn run_warnings(args: DemoArgs) -> Result<()> {
    let environment = demo_environment(args.mock)?;
    let alerts = environment.sources.alerts.alerts().await?;
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
