//! `radar-frame serve`: validate the configuration and run the HTTP service.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Args;

use crate::error::Result;
use crate::http;
use crate::service::clock::SystemClock;
use crate::service::engine::{Engine, Renderer};
use crate::service::setup;
use crate::sources::nexrad::S3Radar;
use crate::sources::osm::Overpass;
use crate::sources::RadarSource;

use super::{context, http_client, init_logging, sources};

#[derive(Args)]
pub struct Serve {
    #[arg(long, default_value = "0.0.0.0:8000")]
    bind: SocketAddr,
    /// Use mock forecast data
    #[arg(long)]
    mock: bool,
}

pub async fn serve(args: Serve) -> Result<()> {
    let (paths, config) = context()?;
    init_logging(&paths, &config);

    setup::create_data_dir(&paths)?;
    setup::create_palette(&paths)?;

    // Fail before binding when the first frame could not be produced: a bad
    // bbox, missing zone, default user agent, unusable palette, or read-only
    // data directory. The street mask is validated but remains optional.
    let settings = crate::service::startup::validate(&config, &paths)?;

    log_startup(&args, &paths, &settings);

    let client = http_client(&config)?;

    // The overlay is cosmetic: build it in the background so an Overpass
    // outage can neither delay startup nor fail the service. Every failure is
    // retried with exponential backoff until the mask is current.
    let streets_paths = paths.clone();
    let streets_config = config.clone();
    let streets_client = client.clone();
    tokio::spawn(async move {
        if let Err(error) = setup::retry_streets(
            &streets_paths,
            &streets_config,
            &Overpass::new(streets_client),
            setup::StreetsRetry::default(),
        )
        .await
        {
            tracing::error!(
                event = "streets_unavailable",
                error = %error,
                "street overlay remains unavailable"
            );
        }
    });

    let radar: Arc<dyn RadarSource> = Arc::new(S3Radar::new(client.clone()));
    let engine: Arc<dyn Renderer> = Arc::new(Engine::new(paths.clone(), &settings));
    let state = Arc::new(http::AppState::new(
        settings,
        paths,
        Arc::new(SystemClock),
        sources(&config, &client, args.mock),
        radar,
        engine,
    ));

    http::serve(state, args.bind).await
}

fn log_startup(args: &Serve, paths: &crate::config::Paths, settings: &crate::config::Settings) {
    let bbox = format!(
        "{},{},{},{}",
        settings.bounds.min_lon,
        settings.bounds.min_lat,
        settings.bounds.max_lon,
        settings.bounds.max_lat
    );
    let zones = format!("{},{}", settings.zone, settings.fire_zone);
    tracing::info!(
        event = "startup",
        version = env!("CARGO_PKG_VERSION"),
        bind = %args.bind,
        station = %settings.nexrad_id,
        bbox = %bbox,
        timezone = %settings.timezone,
        zones = %zones,
        user_agent = %settings.user_agent,
        data = %paths.data_dir().display(),
        data_writable = setup::data_writable(paths),
        streets = settings.streets,
        endpoint_forecast = crate::sources::open_meteo::BASE_URL,
        endpoint_nws = crate::sources::nws::BASE_URL,
        endpoint_radar = crate::sources::nexrad::BASE_URL,
        endpoint_streets = crate::sources::osm::URL,
        "startup"
    );
}
