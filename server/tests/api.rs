//! Golden tests for the HTTP layer against responses captured from the Flask
//! service at `capture.py`'s fixed clock.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use tower::ServiceExt;

use radar_frame::config::paths::Paths;
use radar_frame::config::settings::Settings;
use radar_frame::config::Config;
use radar_frame::http::{self, AppState};
use radar_frame::service::clock::{Clock, FixedClock};
use radar_frame::service::engine::{Engine, Renderer};
use radar_frame::sources::mock::MockProvider;
use radar_frame::sources::nws;
use radar_frame::sources::{
    AlertSource, Forecast, ForecastSource, Object, RadarSource, RadarStatus, RadarStatusSource,
    Sources,
};
use radar_frame::state::{Mode, ModeSelect};

const NOW: &str = "2026-10-21T15:30:00Z";

const CONFIG: &str = "\
[logging]
level = INFO

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
    DateTime::parse_from_rfc3339(NOW)
        .unwrap()
        .with_timezone(&Utc)
}

fn manifest(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn golden(name: &str) -> PathBuf {
    manifest(&format!("tests/fixtures/api/golden/{name}"))
}

struct Fixture {
    _dir: tempfile::TempDir,
    paths: Paths,
    config: Config,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("config/config.ini"), CONFIG).unwrap();
        std::fs::copy(
            manifest("tests/fixtures/frame/ktlx-mini/streets.png"),
            root.join("data/streets.png"),
        )
        .unwrap();

        let paths = Paths::new(root);
        radar_frame::view::palette::write_eink_png(&paths.palette_img()).unwrap();

        Fixture {
            _dir: dir,
            paths,
            config: Config::parse(CONFIG).unwrap(),
        }
    }

    fn settings(&self) -> Settings {
        Settings::from_config(&self.config).unwrap()
    }

    /// Drop persisted state so a recreated app starts from defaults, as a
    /// fresh installation would.
    fn reset_state(&self) {
        match std::fs::remove_file(self.paths.state_file()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("reset state: {error}"),
        }
    }
}

struct TestProvider {
    forecast: Forecast,
    alerts: Vec<radar_frame::view::forecast::Alert>,
    status: RadarStatus,
}

#[async_trait]
impl ForecastSource for TestProvider {
    async fn forecast(&self) -> radar_frame::error::Result<Forecast> {
        Ok(self.forecast.clone())
    }
}

#[async_trait]
impl AlertSource for TestProvider {
    async fn alerts(&self) -> radar_frame::error::Result<Vec<radar_frame::view::forecast::Alert>> {
        Ok(self.alerts.clone())
    }
}

#[async_trait]
impl RadarStatusSource for TestProvider {
    async fn radar_status(&self) -> radar_frame::error::Result<RadarStatus> {
        Ok(self.status)
    }
}

/// Provider whose upstream calls can be failed independently, for the
/// last-good and degraded-frame paths.
struct FlakyProvider {
    fail_forecast: AtomicBool,
    fail_alerts: AtomicBool,
    fail_status: AtomicBool,
}

impl FlakyProvider {
    fn new() -> Self {
        FlakyProvider {
            fail_forecast: AtomicBool::new(false),
            fail_alerts: AtomicBool::new(false),
            fail_status: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl ForecastSource for FlakyProvider {
    async fn forecast(&self) -> radar_frame::error::Result<Forecast> {
        if self.fail_forecast.load(Ordering::SeqCst) {
            return Err(radar_frame::error::Error::new("forecast unavailable"));
        }
        Ok(MockProvider::new().forecast_data().clone())
    }
}

#[async_trait]
impl AlertSource for FlakyProvider {
    async fn alerts(&self) -> radar_frame::error::Result<Vec<radar_frame::view::forecast::Alert>> {
        if self.fail_alerts.load(Ordering::SeqCst) {
            return Err(radar_frame::error::Error::new("alerts unavailable"));
        }
        Ok(Vec::new())
    }
}

#[async_trait]
impl RadarStatusSource for FlakyProvider {
    async fn radar_status(&self) -> radar_frame::error::Result<RadarStatus> {
        if self.fail_status.load(Ordering::SeqCst) {
            return Err(radar_frame::error::Error::new("status unavailable"));
        }
        Ok(RadarStatus { up: true, vcp: 31 })
    }
}

/// Provider that counts each upstream call and can fail them independently.
struct CountingProvider {
    forecast: Forecast,
    alerts: Vec<radar_frame::view::forecast::Alert>,
    status: RadarStatus,
    forecast_calls: AtomicUsize,
    alert_calls: AtomicUsize,
    status_calls: AtomicUsize,
    fail_forecast: AtomicBool,
    fail_alerts: AtomicBool,
    fail_status: AtomicBool,
}

impl CountingProvider {
    fn new() -> Self {
        CountingProvider {
            forecast: MockProvider::new().forecast_data().clone(),
            alerts: Vec::new(),
            status: RadarStatus { up: true, vcp: 31 },
            forecast_calls: AtomicUsize::new(0),
            alert_calls: AtomicUsize::new(0),
            status_calls: AtomicUsize::new(0),
            fail_forecast: AtomicBool::new(false),
            fail_alerts: AtomicBool::new(false),
            fail_status: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl ForecastSource for CountingProvider {
    async fn forecast(&self) -> radar_frame::error::Result<Forecast> {
        self.forecast_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_forecast.load(Ordering::SeqCst) {
            return Err(radar_frame::error::Error::new("forecast unavailable"));
        }
        Ok(self.forecast.clone())
    }
}

#[async_trait]
impl AlertSource for CountingProvider {
    async fn alerts(&self) -> radar_frame::error::Result<Vec<radar_frame::view::forecast::Alert>> {
        self.alert_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_alerts.load(Ordering::SeqCst) {
            return Err(radar_frame::error::Error::new("alerts unavailable"));
        }
        Ok(self.alerts.clone())
    }
}

#[async_trait]
impl RadarStatusSource for CountingProvider {
    async fn radar_status(&self) -> radar_frame::error::Result<RadarStatus> {
        self.status_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_status.load(Ordering::SeqCst) {
            return Err(radar_frame::error::Error::new("status unavailable"));
        }
        Ok(self.status)
    }
}

/// Provider whose radar status alternates between clear-air and precipitation
/// on every call, for the mode-hysteresis test.
struct FlappingProvider {
    forecast: Forecast,
    calls: AtomicUsize,
}

impl FlappingProvider {
    fn new() -> Self {
        FlappingProvider {
            forecast: MockProvider::new().forecast_data().clone(),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ForecastSource for FlappingProvider {
    async fn forecast(&self) -> radar_frame::error::Result<Forecast> {
        Ok(self.forecast.clone())
    }
}

#[async_trait]
impl AlertSource for FlappingProvider {
    async fn alerts(&self) -> radar_frame::error::Result<Vec<radar_frame::view::forecast::Alert>> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl RadarStatusSource for FlappingProvider {
    async fn radar_status(&self) -> radar_frame::error::Result<RadarStatus> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(if call.is_multiple_of(2) {
            RadarStatus { up: true, vcp: 31 }
        } else {
            RadarStatus { up: true, vcp: 212 }
        })
    }
}

/// Provider that parks the first Clear cycle until the test releases it, so
/// read endpoints can be exercised while an update is in flight.
struct GateProvider {
    forecast: Forecast,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
    forecast_calls: AtomicUsize,
}

impl GateProvider {
    fn new() -> Self {
        GateProvider {
            forecast: MockProvider::new().forecast_data().clone(),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            forecast_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ForecastSource for GateProvider {
    async fn forecast(&self) -> radar_frame::error::Result<Forecast> {
        self.forecast_calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        self.release.notified().await;
        Ok(self.forecast.clone())
    }
}

#[async_trait]
impl AlertSource for GateProvider {
    async fn alerts(&self) -> radar_frame::error::Result<Vec<radar_frame::view::forecast::Alert>> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl RadarStatusSource for GateProvider {
    async fn radar_status(&self) -> radar_frame::error::Result<RadarStatus> {
        Ok(RadarStatus { up: true, vcp: 31 })
    }
}

/// Real clear/storm views with a switch for failing the clear render.
struct ToggleRenderer {
    engine: Engine,
    radar_png: PathBuf,
    fail_clear: AtomicBool,
    clear_calls: AtomicUsize,
}

impl ToggleRenderer {
    fn new(fixture: &Fixture, radar_png: PathBuf) -> Self {
        ToggleRenderer {
            engine: Engine::new(fixture.paths.clone(), &fixture.settings()),
            radar_png,
            fail_clear: AtomicBool::new(false),
            clear_calls: AtomicUsize::new(0),
        }
    }
}

impl Renderer for ToggleRenderer {
    fn render_radar(
        &self,
        _volume: &Path,
        _minimum: f64,
        output: &Path,
    ) -> radar_frame::error::Result<radar_frame::render::archive::Summary> {
        std::fs::copy(&self.radar_png, output)?;
        Ok(radar_frame::render::archive::Summary {
            elevations: vec![1],
            radials: 1,
            gates: 1,
        })
    }

    fn render_storm(
        &self,
        alerts: &[radar_frame::view::forecast::Alert],
        now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        self.engine.render_storm(alerts, now)
    }

    fn render_clear(
        &self,
        forecast: &Forecast,
        alerts: &[radar_frame::view::forecast::Alert],
        now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        self.clear_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_clear.load(Ordering::SeqCst) {
            return Err(radar_frame::error::Error::new("render failed"));
        }
        self.engine.render_clear(forecast, alerts, now)
    }
}

struct TestRenderer {
    engine: Engine,
    radar_png: PathBuf,
}

impl Renderer for TestRenderer {
    fn render_radar(
        &self,
        _volume: &Path,
        _minimum: f64,
        output: &Path,
    ) -> radar_frame::error::Result<radar_frame::render::archive::Summary> {
        // Radar rasterization is covered by frame-render's own goldens; this
        // fixture volume only carries elevation 1.
        std::fs::copy(&self.radar_png, output)?;
        Ok(radar_frame::render::archive::Summary {
            elevations: vec![1],
            radials: 1,
            gates: 1,
        })
    }

    fn render_storm(
        &self,
        alerts: &[radar_frame::view::forecast::Alert],
        now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        self.engine.render_storm(alerts, now)
    }

    fn render_clear(
        &self,
        forecast: &Forecast,
        alerts: &[radar_frame::view::forecast::Alert],
        now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        self.engine.render_clear(forecast, alerts, now)
    }
}

/// Publishes fixed bytes regardless of the clock, so a second cycle produces
/// the same ETag and the `If-None-Match` path is observable.
struct StaticRenderer {
    output: PathBuf,
    png: PathBuf,
}

impl Renderer for StaticRenderer {
    fn render_radar(
        &self,
        _volume: &Path,
        _minimum: f64,
        output: &Path,
    ) -> radar_frame::error::Result<radar_frame::render::archive::Summary> {
        std::fs::copy(&self.png, output)?;
        Ok(radar_frame::render::archive::Summary {
            elevations: vec![1],
            radials: 1,
            gates: 1,
        })
    }

    fn render_storm(
        &self,
        _alerts: &[radar_frame::view::forecast::Alert],
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        std::fs::copy(&self.png, &self.output)?;
        Ok(())
    }

    fn render_clear(
        &self,
        _forecast: &Forecast,
        _alerts: &[radar_frame::view::forecast::Alert],
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<()> {
        std::fs::copy(&self.png, &self.output)?;
        Ok(())
    }
}

#[derive(Default)]
struct FakeRadar {
    object: Option<Object>,
    bytes: Vec<u8>,
    downloads: Arc<AtomicUsize>,
}

#[async_trait]
impl RadarSource for FakeRadar {
    async fn latest(
        &self,
        _nexrad_id: &str,
        _now: DateTime<Utc>,
    ) -> radar_frame::error::Result<Option<Object>> {
        Ok(self.object.clone())
    }

    async fn download(
        &self,
        _object: &Object,
        destination: &Path,
    ) -> radar_frame::error::Result<u64> {
        self.downloads.fetch_add(1, Ordering::SeqCst);
        std::fs::write(destination, &self.bytes)?;
        Ok(self.bytes.len() as u64)
    }
}

/// A clock the test can move forward between polls.
struct TestClock {
    now: AtomicI64,
}

impl TestClock {
    fn new(at: DateTime<Utc>) -> Self {
        TestClock {
            now: AtomicI64::new(at.timestamp()),
        }
    }

    fn advance(&self, by: chrono::Duration) {
        self.now.fetch_add(by.num_seconds(), Ordering::SeqCst);
    }

    fn set(&self, at: DateTime<Utc>) {
        self.now.store(at.timestamp(), Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(self.now.load(Ordering::SeqCst), 0).unwrap()
    }
}

fn real_renderer(fixture: &Fixture) -> Arc<dyn Renderer> {
    Arc::new(Engine::new(fixture.paths.clone(), &fixture.settings()))
}

fn app<P>(
    fixture: &Fixture,
    provider: Arc<P>,
    radar: Arc<dyn RadarSource>,
    renderer: Arc<dyn Renderer>,
) -> (Router, Arc<AppState>)
where
    P: ForecastSource + AlertSource + RadarStatusSource + 'static,
{
    app_with_clock(
        fixture,
        provider,
        radar,
        renderer,
        Arc::new(FixedClock(now())),
    )
}

fn app_with_clock<P>(
    fixture: &Fixture,
    provider: Arc<P>,
    radar: Arc<dyn RadarSource>,
    renderer: Arc<dyn Renderer>,
    clock: Arc<dyn Clock>,
) -> (Router, Arc<AppState>)
where
    P: ForecastSource + AlertSource + RadarStatusSource + 'static,
{
    let state = Arc::new(AppState::new(
        fixture.settings(),
        fixture.paths.clone(),
        clock,
        Sources::from_provider(provider),
        radar,
        renderer,
    ));
    (http::router(Arc::clone(&state)), state)
}

async fn get(router: &Router, uri: &str) -> axum::response::Response {
    router
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn get_with_headers(
    router: &Router,
    uri: &str,
    headers: HeaderMap,
) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }
    router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn post_form(router: &Router, uri: &str, body: &'static str) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

fn assert_matches_png(actual: &[u8], expected: &Path) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("actual.png");
    std::fs::write(&path, actual).unwrap();

    let actual = radar_frame::view::image::Image::from_png(&path).unwrap();
    let expected = radar_frame::view::image::Image::from_png(expected).unwrap();

    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height),
        "frame size"
    );

    let mut different = 0usize;
    for y in 0..actual.height {
        for x in 0..actual.width {
            if actual.pixel(x, y) != expected.pixel(x, y) {
                different += 1;
            }
        }
    }
    assert_eq!(
        different,
        0,
        "{} differing pixels of {}",
        different,
        actual.width * actual.height
    );
}

#[tokio::test]
async fn index_and_settings_match_flask() {
    let fixture = Fixture::new();

    let (router, _) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );
    let response = get(&router, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    assert_eq!(
        body_bytes(response).await,
        std::fs::read(golden("index.html")).unwrap()
    );

    let response = post_form(&router, "/settings", "mode-select=Auto").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(response).await,
        std::fs::read(golden("settings-auto.html")).unwrap()
    );

    fixture.reset_state();
    let (router, _) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );
    let response = post_form(&router, "/settings", "mode-select=Manual&mode=Storm").await;
    assert_eq!(
        body_bytes(response).await,
        std::fs::read(golden("settings-manual-storm.html")).unwrap()
    );

    let response = get(&router, "/").await;
    assert_eq!(
        body_bytes(response).await,
        std::fs::read(golden("index-manual-storm.html")).unwrap()
    );

    fixture.reset_state();
    let (router, _) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );
    let response = post_form(&router, "/settings", "mode-select=Manual").await;
    assert_eq!(
        body_bytes(response).await,
        std::fs::read(golden("settings-manual-no-mode.html")).unwrap()
    );
}

#[tokio::test]
async fn debug_frame_serves_the_output_file() {
    let fixture = Fixture::new();
    let png = std::fs::read(golden("debug-frame.png")).unwrap();
    std::fs::write(fixture.paths.output_img(), &png).unwrap();

    let (router, _) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );
    let response = get(&router, "/frame?debug=true").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(body_bytes(response).await, png);
}

#[tokio::test]
async fn clear_frame_matches_flask_and_returns_304_until_due() {
    let fixture = Fixture::new();
    let (router, state) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_matches_png(&body_bytes(response).await, &golden("clear-frame.png"));

    {
        let snapshot = state.snapshot();
        assert_eq!(snapshot.state.mode, Mode::Clear);
        assert_eq!(snapshot.state.last_updated, now());
    }

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn storm_frame_matches_flask() {
    let fixture = Fixture::new();
    let alerts = nws::parse_alerts(
        &std::fs::read_to_string(manifest("tests/fixtures/api/nws-alerts.json")).unwrap(),
    )
    .unwrap();
    let radar = FakeRadar {
        object: Some(Object {
            key: "2026/10/21/KTLX/KTLX20261021_153000_V06".into(),
            last_modified: now() - chrono::Duration::minutes(30),
        }),
        bytes: std::fs::read(manifest("tests/fixtures/nexrad/ktlx-mini.ar2")).unwrap(),
        downloads: Arc::new(AtomicUsize::new(0)),
    };
    let downloads = Arc::clone(&radar.downloads);

    let provider = TestProvider {
        forecast: MockProvider::new().forecast_data().clone(),
        alerts,
        status: RadarStatus { up: true, vcp: 212 },
    };

    let renderer = Arc::new(TestRenderer {
        engine: Engine::new(fixture.paths.clone(), &fixture.settings()),
        radar_png: golden("debug-frame.png"),
    });

    let (router, state) = app(&fixture, Arc::new(provider), Arc::new(radar), renderer);

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_matches_png(&body_bytes(response).await, &golden("storm-frame.png"));

    assert_eq!(downloads.load(Ordering::SeqCst), 1);
    {
        let snapshot = state.snapshot();
        assert_eq!(snapshot.state.mode, Mode::Storm);
        assert_eq!(
            snapshot.state.radar_last_updated,
            now() - chrono::Duration::minutes(30)
        );
        assert_eq!(snapshot.state.last_updated, now());
    }

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn manual_mode_expires_back_to_auto() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let (router, state) = app_with_clock(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
        clock.clone(),
    );

    // Publish a frame so the next poll is scheduled, then select Manual
    // without changing the mode.
    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    assert_eq!(
        post_form(&router, "/settings", "mode-select=Manual")
            .await
            .status(),
        StatusCode::OK
    );

    clock.advance(chrono::Duration::days(1) + chrono::Duration::seconds(1));
    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);

    // Python expires the selection but leaves the stale timestamp in place.
    let snapshot = state.snapshot();
    assert_eq!(snapshot.state.mode_select, ModeSelect::Auto);
    assert!(snapshot.state.manual_til.is_some());
    assert_eq!(snapshot.state.mode, Mode::Clear);
}

#[tokio::test]
async fn settings_rejects_unknown_values() {
    let fixture = Fixture::new();
    let (router, _) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );

    let response = post_form(&router, "/settings", "mode-select=Sometimes").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = post_form(&router, "/settings", "mode-select=Manual&mode=Fog").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn operational_endpoints_report_frame_and_counters() {
    let fixture = Fixture::new();
    let (router, state) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );

    let response = get(&router, "/healthz").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"ok\n");

    let response = get(&router, "/readyz").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = get(&router, "/readyz").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"ready\n");

    let response = get(&router, "/status").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let status: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();

    assert_eq!(status["mode"], "Clear");
    assert_eq!(status["ready"], true);
    assert_eq!(status["data_writable"], true);
    assert_eq!(status["frame"]["available"], true);
    assert!(status["frame"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(status["counters"]["cycles_total"], 1);
    assert_eq!(status["counters"]["cycles_failed"], 0);
    assert!(status["counters"]["upstream_total"].as_u64().unwrap() >= 3);
    assert_eq!(status["last_cycle"]["outcome"], "ok");
    assert_eq!(status["failures"], 0);
    assert!(status["last_attempt"].is_string());
    assert!(status["next_attempt"].is_string());
    assert_eq!(status["radar_status"]["up"], true);
    assert_eq!(status["radar_status"]["vcp"], 31);
    assert_eq!(state.metrics.requests_total(), 5);
}

#[tokio::test]
async fn readiness_rejects_a_stale_frame() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let (router, _) = app_with_clock(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
        clock.clone(),
    );

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);

    // A Clear frame is ready for two 30-minute intervals.
    clock.advance(Mode::Clear.interval() * 2 + chrono::Duration::seconds(1));

    let response = get(&router, "/readyz").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(String::from_utf8(body_bytes(response).await)
        .unwrap()
        .contains("frame is stale"));

    let response = get(&router, "/status").await;
    let status: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
    assert_eq!(status["ready"], false);
    assert_eq!(status["ready_reason"], "frame is stale");
}

#[tokio::test]
async fn stylesheet_is_served() {
    let fixture = Fixture::new();
    let (router, _) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );

    let response = get(&router, "/static/style.css").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/css; charset=utf-8"
    );
    assert_eq!(
        body_bytes(response).await,
        include_bytes!("../static/style.css").to_vec()
    );
}

#[tokio::test]
async fn radar_status_failure_keeps_the_last_mode() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let mut provider = CountingProvider::new();
    provider.status = RadarStatus { up: true, vcp: 212 };
    let provider = Arc::new(provider);
    let renderer = Arc::new(ToggleRenderer::new(&fixture, golden("debug-frame.png")));

    let (router, state) = app_with_clock(
        &fixture,
        Arc::clone(&provider),
        Arc::new(FakeRadar::default()),
        Arc::clone(&renderer) as Arc<dyn Renderer>,
        clock.clone(),
    );

    // The first sample establishes Storm mode and publishes a frame.
    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    assert_eq!(state.snapshot().state.mode, Mode::Storm);

    provider.fail_status.store(true, Ordering::SeqCst);
    clock.advance(chrono::Duration::seconds(61));

    assert_eq!(
        get(&router, "/frame").await.status(),
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(state.snapshot().state.mode, Mode::Storm);
}

#[tokio::test]
async fn alert_failure_publishes_a_degraded_frame() {
    let fixture = Fixture::new();
    let provider = Arc::new(FlakyProvider::new());
    provider.fail_alerts.store(true, Ordering::SeqCst);

    let (router, state) = app(
        &fixture,
        provider,
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_matches_png(&body_bytes(response).await, &golden("clear-frame.png"));

    let snapshot = state.snapshot();
    let frame = snapshot
        .state
        .frame
        .as_ref()
        .expect("degraded frame is published");
    assert!(frame
        .degraded
        .as_deref()
        .is_some_and(|reason| reason.contains("alerts")));
}

#[tokio::test]
async fn failed_cycle_serves_the_last_good_frame() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let renderer = Arc::new(ToggleRenderer::new(&fixture, golden("debug-frame.png")));

    let (router, state) = app_with_clock(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        Arc::clone(&renderer) as Arc<dyn Renderer>,
        clock.clone(),
    );

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG].clone();
    let first = body_bytes(response).await;
    let last_updated = state.snapshot().state.last_updated;

    // Fail the next cycle and make it due.
    renderer.fail_clear.store(true, Ordering::SeqCst);
    clock.advance(Mode::Clear.interval());

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(body_bytes(response).await, first);

    // A failed cycle must not advance the published timestamp.
    assert_eq!(state.snapshot().state.last_updated, last_updated);
}

#[tokio::test]
async fn first_cycle_failure_returns_503() {
    let fixture = Fixture::new();
    let provider = Arc::new(FlakyProvider::new());
    provider.fail_forecast.store(true, Ordering::SeqCst);

    let (router, state) = app(
        &fixture,
        provider,
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "60");
    assert!(state.snapshot().state.frame.is_none());
    assert_eq!(state.metrics.cycles_total(), 1);
    assert_eq!(state.metrics.cycles_failed(), 1);
    assert_eq!(
        state.metrics.last_cycle().unwrap().outcome,
        radar_frame::telemetry::metrics::Outcome::Failed
    );
}

#[tokio::test]
async fn matching_if_none_match_returns_304() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let (router, _) = app_with_clock(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        Arc::new(StaticRenderer {
            output: fixture.paths.output_img(),
            png: golden("debug-frame.png"),
        }),
        clock.clone(),
    );

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG].clone();

    clock.advance(Mode::Clear.interval());

    let mut headers = HeaderMap::new();
    headers.insert(header::IF_NONE_MATCH, etag);
    let response = get_with_headers(&router, "/frame", headers).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert!(body_bytes(response).await.is_empty());

    clock.advance(Mode::Clear.interval());
    let mut headers = HeaderMap::new();
    headers.insert(header::IF_NONE_MATCH, "\"not-the-tag\"".parse().unwrap());
    let response = get_with_headers(&router, "/frame", headers).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn debug_frame_uses_published_bytes() {
    let fixture = Fixture::new();
    let (router, _) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    let published = body_bytes(response).await;

    // A writer replacing the debug artifact must not change what is served.
    std::fs::write(fixture.paths.output_img(), b"torn write").unwrap();

    let response = get(&router, "/frame?debug=true").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, published);
}

#[tokio::test]
async fn debug_frame_maps_read_errors() {
    let fixture = Fixture::new();
    let (router, _) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );

    let response = get(&router, "/frame?debug=true").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Anything other than a missing artifact is a server error, not a 404.
    std::fs::create_dir(fixture.paths.output_img()).unwrap();
    let (router, _) = app(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
    );
    let response = get(&router, "/frame?debug=true").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn radar_status_is_cached_for_a_minute_and_single_flight() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let provider = Arc::new(CountingProvider::new());
    let (router, state) = app_with_clock(
        &fixture,
        Arc::clone(&provider),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
        clock.clone(),
    );

    // Two concurrent polls share one status request.
    let (first, second) = tokio::join!(get(&router, "/frame"), get(&router, "/frame"));
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(provider.status_calls.load(Ordering::SeqCst), 1);

    // Still inside the TTL: no new request.
    assert_eq!(
        get(&router, "/frame").await.status(),
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(provider.status_calls.load(Ordering::SeqCst), 1);

    // Past the TTL: exactly one refresh, even though no cycle is due.
    clock.advance(chrono::Duration::seconds(61));
    assert_eq!(
        get(&router, "/frame").await.status(),
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(provider.status_calls.load(Ordering::SeqCst), 2);

    let snapshot = state.snapshot();
    assert_eq!(
        snapshot.state.radar_status,
        Some(RadarStatus { up: true, vcp: 31 })
    );
    assert_eq!(snapshot.state.failures, 0);
}

#[tokio::test]
async fn concurrent_polls_run_one_cycle_and_do_not_block_reads() {
    let fixture = Fixture::new();
    let provider = Arc::new(GateProvider::new());
    let renderer = Arc::new(ToggleRenderer::new(&fixture, golden("debug-frame.png")));
    let (router, state) = app(
        &fixture,
        Arc::clone(&provider),
        Arc::new(FakeRadar::default()),
        Arc::clone(&renderer) as Arc<dyn Renderer>,
    );

    // Four polls arrive together, as they do when several clients or retries
    // hit the server at once.
    let mut polls = Vec::new();
    for _ in 0..4 {
        let router = router.clone();
        polls.push(tokio::spawn(async move {
            get(&router, "/frame").await.status()
        }));
    }

    // The cycle is now parked inside the forecast call.
    provider.started.notified().await;

    // Read endpoints answer from the snapshot while the cycle holds no lock.
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), get(&router, "/"))
        .await
        .expect("the index must not wait for the cycle");
    assert_eq!(response.status(), StatusCode::OK);
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), get(&router, "/status"))
        .await
        .expect("status must not wait for the cycle");
    assert_eq!(response.status(), StatusCode::OK);

    provider.release.notify_one();

    let mut statuses = Vec::new();
    for poll in polls {
        statuses.push(poll.await.unwrap());
    }
    assert_eq!(
        statuses.iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "exactly one poll runs the cycle: {statuses:?}"
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::NOT_MODIFIED)
            .count(),
        3,
        "the rest keep the published frame: {statuses:?}"
    );

    // The queue of polls can only serialize decisions, never duplicate the
    // upstream call or the render.
    assert_eq!(provider.forecast_calls.load(Ordering::SeqCst), 1);
    assert_eq!(renderer.clear_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.snapshot().state.last_updated, now());
}

#[tokio::test]
async fn radar_status_failure_is_not_retried_until_the_ttl_expires() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let provider = Arc::new(CountingProvider::new());
    let (router, state) = app_with_clock(
        &fixture,
        Arc::clone(&provider),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
        clock.clone(),
    );

    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    provider.fail_status.store(true, Ordering::SeqCst);

    clock.advance(chrono::Duration::seconds(61));
    assert_eq!(
        get(&router, "/frame").await.status(),
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(provider.status_calls.load(Ordering::SeqCst), 2);

    // The failed request is cached too, and the last good status and mode
    // survive the outage.
    assert_eq!(
        get(&router, "/frame").await.status(),
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(provider.status_calls.load(Ordering::SeqCst), 2);

    let snapshot = state.snapshot();
    assert_eq!(
        snapshot.state.radar_status,
        Some(RadarStatus { up: true, vcp: 31 })
    );
    assert_eq!(snapshot.state.mode, Mode::Clear);
}

#[tokio::test]
async fn failed_cycle_backs_off_and_keeps_serving_the_last_good_frame() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let renderer = Arc::new(ToggleRenderer::new(&fixture, golden("debug-frame.png")));
    let (router, state) = app_with_clock(
        &fixture,
        Arc::new(MockProvider::new()),
        Arc::new(FakeRadar::default()),
        Arc::clone(&renderer) as Arc<dyn Renderer>,
        clock.clone(),
    );

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    let first = body_bytes(response).await;
    assert_eq!(renderer.clear_calls.load(Ordering::SeqCst), 1);

    renderer.fail_clear.store(true, Ordering::SeqCst);
    clock.advance(Mode::Clear.interval());

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, first);
    assert_eq!(renderer.clear_calls.load(Ordering::SeqCst), 2);

    let (next_attempt, failures) = {
        let snapshot = state.snapshot();
        (snapshot.state.next_attempt, snapshot.state.failures)
    };
    assert_eq!(failures, 1);
    assert!(next_attempt > clock.now(), "a retry must be scheduled");

    // Before the retry is due every poll serves the last good frame without
    // touching the renderer or upstream again.
    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    assert_eq!(renderer.clear_calls.load(Ordering::SeqCst), 2);

    clock.set(next_attempt);
    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    assert_eq!(renderer.clear_calls.load(Ordering::SeqCst), 3);
    assert_eq!(state.snapshot().state.failures, 2);
}

#[tokio::test]
async fn forecast_failure_serves_the_last_good_forecast() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let provider = Arc::new(CountingProvider::new());
    let (router, state) = app_with_clock(
        &fixture,
        Arc::clone(&provider),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
        clock.clone(),
    );

    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    assert_eq!(provider.forecast_calls.load(Ordering::SeqCst), 1);

    provider.fail_forecast.store(true, Ordering::SeqCst);
    clock.advance(Mode::Clear.interval());

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(provider.forecast_calls.load(Ordering::SeqCst), 2);

    let snapshot = state.snapshot();
    let degraded = snapshot
        .state
        .frame
        .as_ref()
        .unwrap()
        .degraded
        .as_deref()
        .unwrap()
        .to_string();
    assert!(degraded.contains("forecast"), "{degraded}");
    assert!(degraded.contains("using last good"), "{degraded}");

    // The degraded frame is a normal success, so the next poll is a 304.
    assert_eq!(
        get(&router, "/frame").await.status(),
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(provider.forecast_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn alert_failure_serves_the_last_good_alerts() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let provider = Arc::new(CountingProvider::new());
    let (router, state) = app_with_clock(
        &fixture,
        Arc::clone(&provider),
        Arc::new(FakeRadar::default()),
        real_renderer(&fixture),
        clock.clone(),
    );

    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    assert_eq!(provider.alert_calls.load(Ordering::SeqCst), 1);

    provider.fail_alerts.store(true, Ordering::SeqCst);
    clock.advance(Mode::Clear.interval());

    let response = get(&router, "/frame").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(provider.alert_calls.load(Ordering::SeqCst), 2);

    let snapshot = state.snapshot();
    let degraded = snapshot
        .state
        .frame
        .as_ref()
        .unwrap()
        .degraded
        .as_deref()
        .unwrap()
        .to_string();
    assert!(degraded.contains("alerts"), "{degraded}");
    assert!(degraded.contains("using last good"), "{degraded}");
}

#[tokio::test]
async fn flapping_radar_status_does_not_switch_or_render_each_poll() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let renderer = Arc::new(ToggleRenderer::new(&fixture, golden("debug-frame.png")));
    let (router, state) = app_with_clock(
        &fixture,
        Arc::new(FlappingProvider::new()),
        Arc::new(FakeRadar::default()),
        Arc::clone(&renderer) as Arc<dyn Renderer>,
        clock.clone(),
    );

    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    assert_eq!(renderer.clear_calls.load(Ordering::SeqCst), 1);

    // Each poll past the status TTL observes the opposite mode, but neither
    // sample run is long enough to accept a change.
    for _ in 0..4 {
        clock.advance(chrono::Duration::seconds(61));
        assert_eq!(
            get(&router, "/frame").await.status(),
            StatusCode::NOT_MODIFIED
        );
    }

    let snapshot = state.snapshot();
    assert_eq!(snapshot.state.mode, Mode::Clear);
    assert_eq!(renderer.clear_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn restart_restores_the_schedule_and_reuses_the_volume() {
    let fixture = Fixture::new();
    let clock = Arc::new(TestClock::new(now()));
    let object = Object {
        key: "2026/10/21/KTLX/KTLX20261021_150000_V06".into(),
        last_modified: now() - chrono::Duration::minutes(30),
    };
    let bytes = std::fs::read(manifest("tests/fixtures/nexrad/ktlx-mini.ar2")).unwrap();
    let downloads = Arc::new(AtomicUsize::new(0));

    let radar = Arc::new(FakeRadar {
        object: Some(object.clone()),
        bytes: bytes.clone(),
        downloads: Arc::clone(&downloads),
    });
    let renderer = Arc::new(TestRenderer {
        engine: Engine::new(fixture.paths.clone(), &fixture.settings()),
        radar_png: golden("debug-frame.png"),
    });
    let provider = Arc::new(TestProvider {
        forecast: MockProvider::new().forecast_data().clone(),
        alerts: Vec::new(),
        status: RadarStatus { up: true, vcp: 212 },
    });

    let (router, state) = app_with_clock(
        &fixture,
        provider.clone(),
        radar.clone(),
        renderer.clone(),
        clock.clone(),
    );
    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    assert_eq!(downloads.load(Ordering::SeqCst), 1);

    let next_attempt = state.snapshot().state.next_attempt;
    assert_eq!(state.snapshot().state.mode, Mode::Storm);
    assert!(fixture.paths.state_file().is_file());

    // Restart: same data directory, same clock, fresh in-memory objects.
    drop(router);
    drop(state);
    let (router, state) = app_with_clock(&fixture, provider, radar, renderer, clock.clone());

    // The persisted schedule is still in the future, so the first poll after
    // the restart is a 304 rather than a refetch.
    assert_eq!(
        get(&router, "/frame").await.status(),
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(downloads.load(Ordering::SeqCst), 1);
    {
        let snapshot = state.snapshot();
        assert_eq!(snapshot.state.mode, Mode::Storm);
        assert_eq!(
            snapshot.state.radar_key.as_deref(),
            Some(object.key.as_str())
        );
        assert_eq!(snapshot.state.radar_last_updated, object.last_modified);
    }

    // When the next cycle is due, the volume on disk is newer-or-equal to the
    // upstream object, so it is reused instead of downloaded again.
    clock.set(next_attempt);
    assert_eq!(get(&router, "/frame").await.status(), StatusCode::OK);
    assert_eq!(downloads.load(Ordering::SeqCst), 1);
}
