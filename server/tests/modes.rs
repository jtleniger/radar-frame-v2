//! Cycle-boundary behavior of the mode pipeline.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use radar_frame::config::paths::Paths;
use radar_frame::config::settings::Settings;
use radar_frame::config::Config;
use radar_frame::service::modes::{self, Services};
use radar_frame::service::renderer::{FrameRenderer, Renderer};
use radar_frame::sources::mock::MockProvider;
use radar_frame::sources::{
    AlertSource, Forecast, ForecastSource, RadarObject, RadarSource, RadarStatus, RadarStatusSource,
};
use radar_frame::state::{Mode, State};
use radar_frame::view::forecast::Alert;
use radar_frame::view::image::Image;
use radar_frame::view::palette::{write_eink_png, EINK_COLORS};

const CONFIG: &str = "\
[radar]
nexrad_id = KTLX

[bbox]
min_lat = 35.35
max_lat = 35.39
min_lon = -97.26
max_lon = -97.21

[forecast]
lat = 35.37
lon = -97.23
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
impl ForecastSource for CountingProvider {
    async fn forecast(&self) -> radar_frame::error::Result<Forecast> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut forecast = MockProvider::new().forecast_data().clone();
        // The current panel prints this value, so each cycle renders a
        // different frame when (and only when) the provider is consulted.
        forecast.current.temp_f = 60.0 + call as f64;
        Ok(forecast)
    }
}

#[async_trait]
impl AlertSource for CountingProvider {
    async fn alerts(&self) -> radar_frame::error::Result<Vec<Alert>> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl RadarStatusSource for CountingProvider {
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
    ) -> radar_frame::error::Result<Option<RadarObject>> {
        Ok(None)
    }

    async fn download(
        &self,
        _object: &RadarObject,
        _destination: &std::path::Path,
    ) -> radar_frame::error::Result<u64> {
        unreachable!("clear mode does not download radar data")
    }
}

struct BytesRadar {
    radar_object: RadarObject,
    bytes: Vec<u8>,
}

#[async_trait]
impl RadarSource for BytesRadar {
    async fn latest(
        &self,
        _nexrad_id: &str,
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<Option<RadarObject>> {
        Ok(Some(self.radar_object.clone()))
    }

    async fn download(
        &self,
        _object: &RadarObject,
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

    fn render_photo(&self, _photo: &std::path::Path) -> radar_frame::error::Result<()> {
        unreachable!("a rejected volume never reaches the photo view")
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

    let renderer: Arc<dyn Renderer> = Arc::new(FailingRenderer);
    let provider = MockProvider::new();
    let radar = BytesRadar {
        radar_object: RadarObject {
            key: "2026/10/21/KTLX/KTLX20261021_153000_V06".into(),
            last_modified: now(),
        },
        bytes: b"not a volume".to_vec(),
    };
    let metrics = radar_frame::telemetry::metrics::Metrics::new(now());
    let mut state = State::new();
    let settings = Settings::from_config(&config).unwrap();

    let services = Services {
        paths: &paths,
        settings: &settings,
        renderer: &renderer,
        forecast: &provider,
        alerts: &provider,
        radar: &radar,
        metrics: &metrics,
        now: now(),
    };
    assert!(modes::run(Mode::Storm, &services, &mut state)
        .await
        .is_err());

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

    let settings = Settings::from_config(&config).unwrap();
    let renderer: Arc<dyn Renderer> = Arc::new(FrameRenderer::new(paths.clone(), &settings));
    let provider = CountingProvider::new();
    let radar = NoRadar;
    let metrics = radar_frame::telemetry::metrics::Metrics::new(now());
    let mut state = State::new();

    let mut frames = Vec::new();
    for _ in 0..2 {
        let services = Services {
            paths: &paths,
            settings: &settings,
            renderer: &renderer,
            forecast: &provider,
            alerts: &provider,
            radar: &radar,
            metrics: &metrics,
            now: now(),
        };
        modes::run(Mode::Clear, &services, &mut state)
            .await
            .unwrap();
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

struct ListingFails;

#[async_trait]
impl RadarSource for ListingFails {
    async fn latest(
        &self,
        _nexrad_id: &str,
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<Option<RadarObject>> {
        Err(radar_frame::error::Error::new("listing failed"))
    }

    async fn download(
        &self,
        _object: &RadarObject,
        _destination: &std::path::Path,
    ) -> radar_frame::error::Result<u64> {
        unreachable!("the listing already failed")
    }
}

struct DownloadFails;

#[async_trait]
impl RadarSource for DownloadFails {
    async fn latest(
        &self,
        _nexrad_id: &str,
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<Option<RadarObject>> {
        Ok(Some(RadarObject {
            key: "2026/10/21/KTLX/KTLX20261021_153000_V06".into(),
            last_modified: now(),
        }))
    }

    async fn download(
        &self,
        _object: &RadarObject,
        _destination: &std::path::Path,
    ) -> radar_frame::error::Result<u64> {
        Err(radar_frame::error::Error::new("download failed"))
    }
}

/// Writes a placeholder radar frame without parsing the volume.
struct RecordingRenderer;

impl Renderer for RecordingRenderer {
    fn render_radar(
        &self,
        _volume: &std::path::Path,
        _minimum: f64,
        output: &std::path::Path,
    ) -> radar_frame::error::Result<radar_frame::render::archive::Summary> {
        std::fs::write(output, b"png")?;
        Ok(radar_frame::render::archive::Summary {
            elevations: vec![1],
            radials: 1,
            gates: 1,
        })
    }

    fn render_storm(
        &self,
        _alerts: &[Alert],
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        Ok(())
    }

    fn render_clear(
        &self,
        _forecast: &Forecast,
        _alerts: &[Alert],
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        Ok(())
    }

    fn render_photo(&self, _photo: &std::path::Path) -> radar_frame::error::Result<()> {
        unreachable!("photo mode is not exercised here")
    }
}

/// Run one Storm cycle against a radar source that must fail, with a volume
/// already on disk. The cycle succeeds from the last good volume and marks
/// itself degraded.
async fn storm_with_failing_radar(radar: &dyn RadarSource) -> modes::Cycle {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.data_dir()).unwrap();
    std::fs::write(paths.radar_raw(), b"old-volume").unwrap();

    let config = Config::parse(CONFIG).unwrap();
    let renderer: Arc<dyn Renderer> = Arc::new(RecordingRenderer);
    let provider = MockProvider::new();
    let metrics = radar_frame::telemetry::metrics::Metrics::new(now());
    let mut state = State::new();
    let settings = Settings::from_config(&config).unwrap();
    let services = Services {
        paths: &paths,
        settings: &settings,
        renderer: &renderer,
        forecast: &provider,
        alerts: &provider,
        radar,
        metrics: &metrics,
        now: now(),
    };

    let cycle = modes::run(Mode::Storm, &services, &mut state)
        .await
        .unwrap();

    assert_eq!(std::fs::read(paths.radar_raw()).unwrap(), b"old-volume");
    assert_eq!(state.radar_last_updated, DateTime::UNIX_EPOCH);
    assert!(cycle
        .degraded
        .as_deref()
        .is_some_and(|reason| reason.contains("radar")));
    cycle
}

#[tokio::test]
async fn listing_failure_uses_the_last_good_volume() {
    storm_with_failing_radar(&ListingFails).await;
}

#[tokio::test]
async fn download_failure_uses_the_last_good_volume() {
    storm_with_failing_radar(&DownloadFails).await;
}

fn write_photo(path: &std::path::Path, base: [u8; 3]) {
    let mut image = Image::new(800, 480, [0, 0, 0, 255]);
    for y in 0..480u32 {
        for x in 0..800u32 {
            let ramp = (x * 255 / 799) as u8;
            image.put_pixel(
                x as i64,
                y as i64,
                [
                    base[0].wrapping_add(ramp),
                    base[1].wrapping_add(ramp),
                    base[2].wrapping_add(ramp),
                    255,
                ],
            );
        }
    }
    image.write_png(path).unwrap();
}

#[tokio::test]
async fn photo_mode_rotates_in_name_order_and_dithers() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.data_dir()).unwrap();
    std::fs::create_dir_all(paths.photos_dir()).unwrap();
    write_eink_png(&paths.palette_img()).unwrap();
    write_photo(&paths.photos_dir().join("b.png"), [0, 0, 0]);
    write_photo(&paths.photos_dir().join("a.png"), [200, 30, 30]);
    write_photo(&paths.photos_dir().join("c.png"), [30, 30, 200]);

    let config = Config::parse(CONFIG).unwrap();
    let settings = Settings::from_config(&config).unwrap();
    let renderer: Arc<dyn Renderer> = Arc::new(FrameRenderer::new(paths.clone(), &settings));
    let provider = MockProvider::new();
    let radar = NoRadar;
    let metrics = radar_frame::telemetry::metrics::Metrics::new(now());
    let mut state = State::new();

    let mut order = Vec::new();
    for _ in 0..4 {
        let services = Services {
            paths: &paths,
            settings: &settings,
            renderer: &renderer,
            forecast: &provider,
            alerts: &provider,
            radar: &radar,
            metrics: &metrics,
            now: now(),
        };
        let cycle = modes::run(Mode::Photo, &services, &mut state)
            .await
            .unwrap();
        order.push(cycle.photo.unwrap());
    }

    assert_eq!(order, ["a.png", "b.png", "c.png", "a.png"]);
    assert_eq!(state.photo_last.as_deref(), Some("a.png"));

    let output = Image::from_png(&paths.output_img()).unwrap();
    assert_eq!((output.width, output.height), (800, 480));
    let mut colors = Vec::new();
    for y in 0..output.height {
        for x in 0..output.width {
            let pixel = output.pixel(x, y);
            assert!(
                EINK_COLORS.contains(&[pixel[0], pixel[1], pixel[2]]),
                "pixel {x},{y} is not a palette colour: {pixel:?}"
            );
            if !colors.contains(&[pixel[0], pixel[1], pixel[2]]) {
                colors.push([pixel[0], pixel[1], pixel[2]]);
            }
        }
    }
    assert!(
        colors.len() > 1,
        "dithering a gradient must use more than one palette colour"
    );
}

#[tokio::test]
async fn photo_mode_skips_a_wrong_sized_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.data_dir()).unwrap();
    std::fs::create_dir_all(paths.photos_dir()).unwrap();
    write_eink_png(&paths.palette_img()).unwrap();
    Image::new(10, 10, [255, 255, 255, 255])
        .write_png(&paths.photos_dir().join("a.png"))
        .unwrap();
    write_photo(&paths.photos_dir().join("b.png"), [10, 100, 200]);

    let config = Config::parse(CONFIG).unwrap();
    let settings = Settings::from_config(&config).unwrap();
    let renderer: Arc<dyn Renderer> = Arc::new(FrameRenderer::new(paths.clone(), &settings));
    let provider = MockProvider::new();
    let radar = NoRadar;
    let metrics = radar_frame::telemetry::metrics::Metrics::new(now());
    let mut state = State::new();
    let services = Services {
        paths: &paths,
        settings: &settings,
        renderer: &renderer,
        forecast: &provider,
        alerts: &provider,
        radar: &radar,
        metrics: &metrics,
        now: now(),
    };

    let cycle = modes::run(Mode::Photo, &services, &mut state)
        .await
        .unwrap();
    assert_eq!(cycle.photo.as_deref(), Some("b.png"));
}

#[tokio::test]
async fn photo_mode_fails_without_a_photo_directory() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path());
    std::fs::create_dir_all(paths.data_dir()).unwrap();
    write_eink_png(&paths.palette_img()).unwrap();

    let config = Config::parse(CONFIG).unwrap();
    let settings = Settings::from_config(&config).unwrap();
    let renderer: Arc<dyn Renderer> = Arc::new(FrameRenderer::new(paths.clone(), &settings));
    let provider = MockProvider::new();
    let radar = NoRadar;
    let metrics = radar_frame::telemetry::metrics::Metrics::new(now());
    let mut state = State::new();
    let services = Services {
        paths: &paths,
        settings: &settings,
        renderer: &renderer,
        forecast: &provider,
        alerts: &provider,
        radar: &radar,
        metrics: &metrics,
        now: now(),
    };

    let error = modes::run(Mode::Photo, &services, &mut state)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("photos"), "{error}");
}
