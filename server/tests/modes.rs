//! Cycle-boundary behavior of the mode pipeline.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use radar_frame::config::Config;
use radar_frame::engine::{Engine, Renderer};
use radar_frame::modes::{self, Deps};
use radar_frame::paths::Paths;
use radar_frame::sources::mock::MockProvider;
use radar_frame::sources::{Forecast, ForecastProvider, Object, RadarSource, RadarStatus};
use radar_frame::state::{Mode, State};
use radar_frame::view::forecast::Alert;
use radar_frame::view::palette::write_eink_png;

const CONFIG: &str = "\
[radar]
nexrad_id = KTLX

[bbox]
min_lat = 35.35
max_lat = 35.39
min_lon = -97.26
max_lon = -97.21

[forecast]
timezone = America/Chicago

[nws]
user_agent = radar-frame (test@example.com)
zone = OKZ025
fire_zone = OKZ025
";

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-21T15:30:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

struct CountingProvider {
    calls: AtomicUsize,
}

impl CountingProvider {
    fn new() -> Self {
        CountingProvider {
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ForecastProvider for CountingProvider {
    async fn forecast(&self) -> radar_frame::error::Result<Forecast> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut forecast = MockProvider::new().forecast_data().clone();
        // The current panel prints this value, so each cycle renders a
        // different frame when (and only when) the provider is consulted.
        forecast.current.temp_f = 60.0 + call as f64;
        Ok(forecast)
    }

    async fn alerts(&self) -> radar_frame::error::Result<Vec<Alert>> {
        Ok(Vec::new())
    }

    async fn radar_status(&self) -> radar_frame::error::Result<RadarStatus> {
        Ok(RadarStatus { up: true, vcp: 31 })
    }
}

struct NoRadar;

#[async_trait]
impl RadarSource for NoRadar {
    async fn latest(
        &self,
        _nexrad_id: &str,
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<Option<Object>> {
        Ok(None)
    }

    async fn download(&self, _object: &Object) -> radar_frame::error::Result<Vec<u8>> {
        unreachable!("clear mode does not download radar data")
    }
}

#[tokio::test]
async fn clear_mode_fetches_a_new_forecast_each_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.data_dir()).unwrap();
    let config = Config::parse(CONFIG).unwrap();
    write_eink_png(&paths.palette_img()).unwrap();

    let engine: Arc<dyn Renderer> = Arc::new(Engine::new(paths.clone(), &config).unwrap());
    let provider = CountingProvider::new();
    let radar = NoRadar;
    let mut state = State::new();

    let mut frames = Vec::new();
    for _ in 0..2 {
        let deps = Deps {
            paths: &paths,
            config: &config,
            engine: &engine,
            provider: &provider,
            radar: &radar,
            now: now(),
        };
        modes::run(Mode::Clear, &deps, &mut state).await.unwrap();
        frames.push(std::fs::read(paths.output_img()).unwrap());
    }

    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        2,
        "each Clear cycle must fetch its own forecast"
    );
    assert_ne!(
        frames[0], frames[1],
        "the second cycle must publish the new forecast"
    );
}
