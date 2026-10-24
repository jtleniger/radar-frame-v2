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

    async fn download(
        &self,
        _object: &Object,
        _destination: &std::path::Path,
    ) -> radar_frame::error::Result<u64> {
        unreachable!("clear mode does not download radar data")
    }
}

struct BytesRadar {
    object: Object,
    bytes: Vec<u8>,
}

#[async_trait]
impl RadarSource for BytesRadar {
    async fn latest(
        &self,
        _nexrad_id: &str,
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<Option<Object>> {
        Ok(Some(self.object.clone()))
    }

    async fn download(
        &self,
        _object: &Object,
        destination: &std::path::Path,
    ) -> radar_frame::error::Result<u64> {
        std::fs::write(destination, &self.bytes)?;
        Ok(self.bytes.len() as u64)
    }
}

struct FailingRenderer;

impl Renderer for FailingRenderer {
    fn render_radar(
        &self,
        _volume: &std::path::Path,
        _minimum: f64,
        _output: &std::path::Path,
    ) -> radar_frame::error::Result<radar_frame::render::archive::Summary> {
        Err(radar_frame::error::Error::new("volume rejected"))
    }

    fn render_storm(
        &self,
        _alerts: &[Alert],
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        unreachable!("a rejected volume never reaches the storm view")
    }

    fn render_clear(
        &self,
        _forecast: &Forecast,
        _alerts: &[Alert],
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        unreachable!("a rejected volume never reaches the clear view")
    }
}

#[tokio::test]
async fn rejected_volume_keeps_the_last_good_pair() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.data_dir()).unwrap();
    let config = Config::parse(CONFIG).unwrap();

    std::fs::write(paths.radar_raw(), b"old-volume").unwrap();
    std::fs::write(paths.radar_img(), b"old-frame").unwrap();

    let engine: Arc<dyn Renderer> = Arc::new(FailingRenderer);
    let provider = MockProvider::new();
    let radar = BytesRadar {
        object: Object {
            key: "2026/10/21/KTLX/KTLX20261021_153000_V06".into(),
            last_modified: now(),
        },
        bytes: b"not a volume".to_vec(),
    };
    let metrics = radar_frame::metrics::Metrics::new(now());
    let mut state = State::new();

    let deps = Deps {
        paths: &paths,
        config: &config,
        engine: &engine,
        provider: &provider,
        radar: &radar,
        metrics: &metrics,
        now: now(),
    };
    assert!(modes::run(Mode::Storm, &deps, &mut state).await.is_err());

    assert_eq!(state.radar_last_updated, DateTime::UNIX_EPOCH);
    assert_eq!(std::fs::read(paths.radar_raw()).unwrap(), b"old-volume");
    assert_eq!(std::fs::read(paths.radar_img()).unwrap(), b"old-frame");
    assert!(!paths.radar_part().exists());
    assert!(!paths.radar_img_part().exists());
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
    let metrics = radar_frame::metrics::Metrics::new(now());
    let mut state = State::new();

    let mut frames = Vec::new();
    for _ in 0..2 {
        let deps = Deps {
            paths: &paths,
            config: &config,
            engine: &engine,
            provider: &provider,
            radar: &radar,
            metrics: &metrics,
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
