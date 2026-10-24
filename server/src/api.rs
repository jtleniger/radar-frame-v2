//! The HTTP surface: `/`, `/settings`, `/frame`, and the stylesheet.
//!
//! Request and response semantics follow `server/api.py`, including the 304
//! polling contract and the auto/manual state machine.
//!
//! One background updater owns the mutable [`State`]. It performs the status
//! probe and the render cycle, then publishes an immutable [`Snapshot`] to a
//! watch channel. Handlers answer from that snapshot and never hold a lock
//! across network I/O: a `/frame` poll only wakes the updater when work is due
//! and waits, bounded, for the result.

use std::sync::Arc;
use std::time::{Duration, Instant};

use askama::Template;
use axum::extract::{Form, Query, State as AxumState};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch};

use crate::clock::Clock;
use crate::engine::Renderer;
use crate::error::{Error, Result};
use crate::frame::PublishedFrame;
use crate::metrics::{elapsed_ms, Metrics, Outcome, Sample};
use crate::modes::{self, Deps};
use crate::paths::Paths;
use crate::settings::Settings;
use crate::setup;
use crate::sources::{RadarSource, Sources};
use crate::state::{Mode, ModeSelect, State, STATUS_TTL};

/// Upper bound for one update cycle. Each upstream call is bounded by the
/// HTTP client timeouts; this caps the whole sequence plus rendering so the
/// updater always becomes responsive again.
const CYCLE_DEADLINE: Duration = Duration::from_secs(180);

/// How often the updater wakes without a poll, so the schedule keeps running
/// when the display client is offline.
const TICK: Duration = Duration::from_secs(60);

/// How long a `/frame` poll waits for the updater to finish the work it woke.
const UPDATE_DEADLINE: Duration = Duration::from_secs(185);

/// Immutable schedule and frame published by the updater. Handlers clone the
/// handle, never the mutable state.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: State,
}

pub struct AppState {
    pub settings: Settings,
    pub paths: Paths,
    pub clock: Arc<dyn Clock>,
    pub sources: Sources,
    pub radar: Arc<dyn RadarSource>,
    pub engine: Arc<dyn Renderer>,
    pub metrics: Arc<Metrics>,
    snapshot: watch::Receiver<Arc<Snapshot>>,
    commands: mpsc::UnboundedSender<Command>,
}

impl AppState {
    /// Build the shared state and start the updater task.
    ///
    /// Must be called inside a Tokio runtime: the updater runs as its own
    /// task and exits when this value (and with it the command sender) drops.
    pub fn new(
        settings: Settings,
        paths: Paths,
        clock: Arc<dyn Clock>,
        sources: Sources,
        radar: Arc<dyn RadarSource>,
        engine: Arc<dyn Renderer>,
    ) -> Self {
        let started_at = clock.now();

        // A frame left by the previous process is still the last good one;
        // publish it so the first poll survives an upstream outage. The
        // persisted schedule is restored with it, so a restart does not
        // immediately repeat upstream work.
        let mut state = State::load(&paths.state_file());
        state.frame = PublishedFrame::load(&paths.output_img()).map(Arc::new);

        let (snapshots, snapshot) = watch::channel(Arc::new(Snapshot {
            state: state.clone(),
        }));
        let (commands, receiver) = mpsc::unbounded_channel();
        let metrics = Arc::new(Metrics::new(started_at));

        let updater = Updater {
            settings: settings.clone(),
            paths: paths.clone(),
            clock: Arc::clone(&clock),
            sources: sources.clone(),
            radar: Arc::clone(&radar),
            engine: Arc::clone(&engine),
            metrics: Arc::clone(&metrics),
            state,
            snapshots,
        };
        tokio::spawn(updater.run(receiver));

        AppState {
            settings,
            paths,
            clock,
            sources,
            radar,
            engine,
            metrics,
            snapshot,
            commands,
        }
    }

    /// The schedule and frame published by the last completed update.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.borrow().clone()
    }

    /// Ask the updater to run whatever is due and return the outcome.
    ///
    /// Concurrent polls queue behind the single updater, so only one of them
    /// performs the status probe and the cycle; the rest see the refreshed
    /// schedule and report [`Update::Skipped`].
    async fn update(&self) -> Update {
        let (reply, response) = oneshot::channel();
        if self.commands.send(Command::Update { reply }).is_err() {
            return Update::Failed;
        }
        match tokio::time::timeout(UPDATE_DEADLINE, response).await {
            Ok(Ok(update)) => update,
            _ => Update::Failed,
        }
    }

    /// Apply a `/settings` change through the updater. Returns false when the
    /// updater has stopped.
    async fn apply_settings(&self, mode_select: ModeSelect, mode: Option<Mode>) -> bool {
        let (reply, response) = oneshot::channel();
        if self
            .commands
            .send(Command::Settings {
                mode_select,
                mode,
                reply,
            })
            .is_err()
        {
            return false;
        }
        response.await.is_ok()
    }
}

enum Command {
    Update {
        reply: oneshot::Sender<Update>,
    },
    Settings {
        mode_select: ModeSelect,
        mode: Option<Mode>,
        reply: oneshot::Sender<()>,
    },
}

/// What one [`Updater::update`] pass did.
enum Update {
    /// Nothing was due; the last published frame still stands.
    Skipped,
    /// A cycle rendered and validated a new frame.
    Published(Arc<PublishedFrame>),
    /// A cycle was due and failed; the last good frame is still published.
    Failed,
}

/// Owns [`State`] and is the only task that mutates it.
struct Updater {
    settings: Settings,
    paths: Paths,
    clock: Arc<dyn Clock>,
    sources: Sources,
    radar: Arc<dyn RadarSource>,
    engine: Arc<dyn Renderer>,
    metrics: Arc<Metrics>,
    state: State,
    snapshots: watch::Sender<Arc<Snapshot>>,
}

impl Updater {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        // The first tick is one period out: the initial update is driven by
        // the first poll (or the next tick), never by task startup.
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + TICK, TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let now = self.clock.now();
                    let _ = self.update(now).await;
                }
                command = commands.recv() => {
                    let Some(command) = command else { break };
                    match command {
                        Command::Update { reply } => {
                            let now = self.clock.now();
                            let update = self.update(now).await;
                            let _ = reply.send(update);
                        }
                        Command::Settings { mode_select, mode, reply } => {
                            self.set_mode(mode_select, mode);
                            let _ = reply.send(());
                        }
                    }
                }
            }
        }
    }

    /// One pass: expire a manual selection, refresh the status sample when its
    /// TTL has passed, and run a cycle when the schedule allows it.
    async fn update(&mut self, now: DateTime<Utc>) -> Update {
        let mut changed = self.state.expire_manual(now);
        if changed {
            tracing::info!("manual mode expired");
        }

        if self.state.mode_select == ModeSelect::Auto {
            match update_mode(&mut self.state, &self.sources, &self.metrics, now).await {
                Ok(true) => {
                    self.state.reschedule_now();
                    changed = true;
                }
                Ok(false) => {}
                Err(error) => {
                    // A status outage must not block rendering or change the
                    // last known mode; the cycle below still runs on schedule.
                    tracing::error!("radar status: {error}");
                    changed = true;
                }
            }
        }

        if changed {
            self.persist();
            self.publish();
        }

        if !self.state.cycle_due(now) {
            return Update::Skipped;
        }

        let update = self.run_cycle(now).await;
        self.persist();
        self.publish();
        update
    }

    fn set_mode(&mut self, mode_select: ModeSelect, mode: Option<Mode>) {
        tracing::info!("Set mode select to {}", mode_select.name());
        let now = self.clock.now();
        self.state.apply_settings(mode_select, mode, now);
        self.persist();
        self.publish();
    }

    async fn run_cycle(&mut self, now: DateTime<Utc>) -> Update {
        let mode = self.state.mode;
        let deps = Deps {
            paths: &self.paths,
            settings: &self.settings,
            engine: &self.engine,
            forecast: self.sources.forecast.as_ref(),
            alerts: self.sources.alerts.as_ref(),
            radar: self.radar.as_ref(),
            metrics: self.metrics.as_ref(),
            now,
        };

        let cycle_started = Instant::now();
        let cycle =
            match tokio::time::timeout(CYCLE_DEADLINE, modes::run(mode, &deps, &mut self.state))
                .await
            {
                Ok(result) => result,
                Err(_) => Err(Error::new(format!(
                    "cycle exceeded the {} s deadline",
                    CYCLE_DEADLINE.as_secs()
                ))),
            };
        let cycle = match cycle {
            Ok(cycle) => cycle,
            Err(error) => return self.fail(now, mode, cycle_started, error),
        };

        let bytes = match std::fs::read(self.paths.output_img()) {
            Ok(bytes) => bytes,
            Err(error) => {
                return self.fail(
                    now,
                    mode,
                    cycle_started,
                    Error::new(format!("{}: {error}", self.paths.output_img().display())),
                );
            }
        };
        let frame = match PublishedFrame::new(bytes, now, Some(mode), cycle.degraded.clone()) {
            Ok(frame) => frame,
            Err(error) => return self.fail(now, mode, cycle_started, error),
        };

        let frame = Arc::new(frame);
        self.state.cycle_succeeded(now, mode);
        self.state.frame = Some(Arc::clone(&frame));

        let outcome = if cycle.degraded.is_some() {
            Outcome::Degraded
        } else {
            Outcome::Ok
        };
        let elapsed = cycle_started.elapsed();
        self.metrics.cycle(self.clock.now(), elapsed, outcome);
        tracing::info!(
            event = "cycle",
            mode = mode.name(),
            outcome = outcome.as_str(),
            elapsed_ms = elapsed_ms(elapsed),
            volume_key = cycle.volume_key.as_deref().unwrap_or("-"),
            elevations = ?cycle.elevations,
            radials = cycle.radials,
            gates = cycle.gates,
            alerts = cycle.alerts,
            render_ms = cycle.render_ms,
            frame_bytes = frame.bytes.len(),
            frame_sha = frame.etag.trim_matches('"'),
            degraded = cycle.degraded.as_deref().unwrap_or("-"),
            "cycle complete"
        );
        Update::Published(frame)
    }

    fn fail(&mut self, now: DateTime<Utc>, mode: Mode, started: Instant, error: Error) -> Update {
        let elapsed = started.elapsed();
        self.metrics
            .cycle(self.clock.now(), elapsed, Outcome::Failed);
        tracing::warn!(
            event = "cycle",
            mode = mode.name(),
            outcome = "failed",
            elapsed_ms = elapsed_ms(elapsed),
            error = %error,
            "cycle failed"
        );
        tracing::error!("{} mode failed: {error}", mode.name());
        self.state.cycle_failed(now, mode);
        Update::Failed
    }

    /// Publish the state for handlers. A failure means no handler is
    /// listening, which is fine: the updater is shutting down.
    fn publish(&self) {
        self.snapshots.send_replace(Arc::new(Snapshot {
            state: self.state.clone(),
        }));
    }

    fn persist(&self) {
        if let Err(error) = self.state.save(&self.paths.state_file()) {
            tracing::warn!(event = "state", error = %error, "could not persist state");
        }
    }
}

/// Refresh the radar-status sample when its TTL has passed. True when the mode
/// changed.
///
/// The timestamps are recorded even when the request fails, so an outage is
/// retried once per TTL instead of on every poll. A failure keeps the
/// last-known-good status and the current mode.
async fn update_mode(
    state: &mut State,
    sources: &Sources,
    metrics: &Metrics,
    now: DateTime<Utc>,
) -> Result<bool> {
    if now < state.status_next_attempt {
        return Ok(false);
    }

    let started = Instant::now();
    let status = sources.status.radar_status().await;
    metrics.upstream(now, started.elapsed(), status.is_ok());

    state.status_checked_at = now;
    state.status_next_attempt = now + STATUS_TTL;

    let status = status?;
    tracing::info!("radar status: up={} vcp={}", status.up, status.vcp);
    if !status.up {
        tracing::error!("radar down");
    } else if status.vcp == -1 {
        tracing::error!("some issue with nws api");
    }

    // A change is accepted only after consecutive agreeing samples and a
    // minimum dwell; the first sample after startup bootstraps the mode.
    let changed = state.observe_status(status, now);
    if changed {
        tracing::info!(event = "mode", mode = state.mode.name(), "mode switched");
    }
    Ok(changed)
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/settings", post(settings))
        .route("/frame", get(frame))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/status", get(status))
        .route("/static/style.css", get(stylesheet))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            count_request,
        ))
        .with_state(state)
}

async fn count_request(
    AxumState(app): AxumState<Arc<AppState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    app.metrics.request();
    next.run(request).await
}

async fn healthz() -> Response {
    text_response(StatusCode::OK, "ok\n")
}

async fn readyz(AxumState(app): AxumState<Arc<AppState>>) -> Response {
    let now = app.clock.now();
    let snapshot = app.snapshot();
    let (ready, reason) = readiness(&snapshot.state, now, setup::data_writable(&app.paths));

    if ready {
        text_response(StatusCode::OK, "ready\n")
    } else {
        text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("not ready: {reason}\n"),
        )
    }
}

async fn status(AxumState(app): AxumState<Arc<AppState>>) -> Response {
    let now = app.clock.now();
    let writable = setup::data_writable(&app.paths);
    let snapshot = app.snapshot();
    let state = &snapshot.state;

    let (ready, reason) = readiness(state, now, writable);
    let fields = fields(state, app.settings.timezone);

    let frame = match state.frame.as_ref() {
        Some(frame) => json!({
            "available": true,
            "generated_at": timestamp(frame.generated_at),
            "age_seconds": age_seconds(now, frame.generated_at),
            "bytes": frame.bytes.len(),
            "etag": &frame.etag,
            "mode": frame.mode.map(Mode::name),
            "degraded": frame.degraded.as_deref(),
        }),
        None => json!({ "available": false }),
    };

    let body = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "started_at": app.metrics.started_at().and_then(timestamp),
        "uptime_seconds": app.metrics.started_at().map(|started| age_seconds(now, started)),
        "mode": state.mode.name(),
        "mode_select": fields.mode_select,
        "manual_til": state.manual_til.and_then(timestamp),
        "last_updated": timestamp(state.last_updated),
        "last_attempt": timestamp(state.last_attempt),
        "next_attempt": timestamp(state.next_attempt),
        "failures": state.failures,
        "radar_status": state.radar_status.map(|status| json!({
            "up": status.up,
            "vcp": status.vcp,
            "checked_at": timestamp(state.status_checked_at),
            "age_seconds": age_seconds(now, state.status_checked_at),
        })),
        "radar_volume": {
            "key": state.radar_key.as_deref(),
            "last_updated": timestamp(state.radar_last_updated),
        },
        "ready": ready,
        "ready_reason": reason,
        "data_writable": writable,
        "frame": frame,
        "counters": {
            "requests_total": app.metrics.requests_total(),
            "cycles_total": app.metrics.cycles_total(),
            "cycles_failed": app.metrics.cycles_failed(),
            "cycles_degraded": app.metrics.cycles_degraded(),
            "upstream_total": app.metrics.upstream_total(),
            "upstream_failed": app.metrics.upstream_failed(),
        },
        "last_cycle": sample(app.metrics.last_cycle(), now),
        "last_upstream": sample(app.metrics.last_upstream(), now),
    });

    (
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string()),
    )
        .into_response()
}

fn text_response(status: StatusCode, body: &str) -> Response {
    let mut response = (status, body.to_string()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// A ready process has published a frame that is no older than two mode
/// intervals and can still write to `data/`.
fn readiness(state: &State, now: DateTime<Utc>, data_writable: bool) -> (bool, &'static str) {
    let Some(frame) = state.frame.as_ref() else {
        return (false, "no frame published");
    };
    if now.signed_duration_since(frame.generated_at) > state.mode.interval() * 2 {
        return (false, "frame is stale");
    }
    if !data_writable {
        return (false, "data directory is not writable");
    }
    (true, "ready")
}

fn timestamp(time: DateTime<Utc>) -> Option<String> {
    (time != DateTime::UNIX_EPOCH).then(|| time.to_rfc3339())
}

fn age_seconds(now: DateTime<Utc>, then: DateTime<Utc>) -> i64 {
    now.signed_duration_since(then).num_seconds().max(0)
}

fn sample(sample: Option<Sample>, now: DateTime<Utc>) -> Value {
    match sample {
        Some(sample) => json!({
            "at": sample.at.to_rfc3339(),
            "age_seconds": age_seconds(now, sample.at),
            "elapsed_ms": sample.elapsed_ms,
            "outcome": sample.outcome.as_str(),
        }),
        None => Value::Null,
    }
}

struct Fields {
    mode: &'static str,
    mode_select: &'static str,
    last_updated: String,
    manual_til: String,
    has_manual_til: bool,
}

fn fields(state: &State, timezone: Tz) -> Fields {
    let manual_til = state.manual_til_str(timezone);
    Fields {
        mode: state.mode.name(),
        mode_select: state.mode_select.name(),
        // Jinja rendered `None` for the initial timestamp; keep the text.
        last_updated: state
            .last_updated_str(timezone)
            .unwrap_or_else(|| "None".to_string()),
        has_manual_til: manual_til.is_some(),
        manual_til: manual_til.unwrap_or_default(),
    }
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    mode: &'static str,
    mode_select: &'static str,
    last_updated: String,
    manual_til: String,
    has_manual_til: bool,
}

#[derive(Template)]
#[template(path = "partials/settings.html")]
struct SettingsPage {
    mode: &'static str,
    mode_select: &'static str,
    manual_til: String,
    has_manual_til: bool,
}

fn html<T: Template>(page: T) -> Response {
    match page.render() {
        Ok(body) => Html(body).into_response(),
        Err(error) => {
            tracing::error!("template: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn index(AxumState(app): AxumState<Arc<AppState>>) -> Response {
    let snapshot = app.snapshot();
    let fields = fields(&snapshot.state, app.settings.timezone);

    html(IndexPage {
        mode: fields.mode,
        mode_select: fields.mode_select,
        last_updated: fields.last_updated,
        manual_til: fields.manual_til,
        has_manual_til: fields.has_manual_til,
    })
}

#[derive(Deserialize)]
struct SettingsForm {
    #[serde(rename = "mode-select")]
    mode_select: String,
    #[serde(default)]
    mode: Option<String>,
}

async fn settings(
    AxumState(app): AxumState<Arc<AppState>>,
    Form(form): Form<SettingsForm>,
) -> Response {
    let Some(mode_select) = ModeSelect::parse(&form.mode_select) else {
        return (StatusCode::BAD_REQUEST, "unknown mode-select").into_response();
    };
    let requested_mode = match form.mode.as_deref() {
        Some(value) => match Mode::parse(value) {
            Some(mode) => Some(mode),
            None => return (StatusCode::BAD_REQUEST, "unknown mode").into_response(),
        },
        None => None,
    };

    if !app.apply_settings(mode_select, requested_mode).await {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "updater unavailable\n");
    }

    let snapshot = app.snapshot();
    let fields = fields(&snapshot.state, app.settings.timezone);

    html(SettingsPage {
        mode: fields.mode,
        mode_select: fields.mode_select,
        manual_til: fields.manual_til,
        has_manual_til: fields.has_manual_til,
    })
}

#[derive(Deserialize)]
struct FrameQuery {
    debug: Option<String>,
}

async fn frame(
    AxumState(app): AxumState<Arc<AppState>>,
    Query(query): Query<FrameQuery>,
    headers: HeaderMap,
) -> Response {
    if query.debug.as_deref() == Some("true") {
        tracing::info!("debug param set, sending current output");
        // Prefer the published bytes so debug reads cannot observe a writer
        // midway through `frame.png`.
        if let Some(frame) = app.snapshot().state.frame.clone() {
            return frame.response(&headers);
        }
        return png_file(&app.paths.output_img()).await;
    }

    match app.update().await {
        Update::Published(frame) => frame.response(&headers),
        Update::Failed => last_good(&app.snapshot().state, &headers),
        Update::Skipped => {
            let snapshot = app.snapshot();
            if snapshot.state.failures > 0 {
                // A cycle failed and the backoff has not elapsed. Keep serving
                // the last good frame rather than a bare 304, so a client that
                // restarted during the outage still gets a usable image.
                last_good(&snapshot.state, &headers)
            } else {
                // The polling contract: no new frame is due, so tell the
                // client to keep what it has. Include the tag for clients that
                // track it.
                not_modified(&snapshot.state)
            }
        }
    }
}

/// Serve the last good frame after a failed cycle, or 503 before the first
/// frame has ever been published.
fn last_good(state: &State, headers: &HeaderMap) -> Response {
    match state.frame.clone() {
        Some(frame) => {
            tracing::warn!("serving last good frame after failed cycle");
            frame.response(headers)
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "60")],
            "no frame available yet",
        )
            .into_response(),
    }
}

fn not_modified(state: &State) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    if let Some(etag) = state
        .frame
        .as_ref()
        .and_then(|frame| HeaderValue::from_str(&frame.etag).ok())
    {
        response.headers_mut().insert(header::ETAG, etag);
    }
    response
}

async fn png_file(path: &std::path::Path) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(error) => {
            tracing::error!("{}: {error}", path.display());
            if error.kind() == std::io::ErrorKind::NotFound {
                StatusCode::NOT_FOUND.into_response()
            } else {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
}

async fn stylesheet() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../static/style.css"),
    )
        .into_response()
}

/// Bind and serve until the process is stopped.
pub async fn serve(state: Arc<AppState>, address: std::net::SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|e| Error(format!("{address}: {e}")))?;
    tracing::info!("listening on {address}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| Error(format!("server: {e}")))?;
    tracing::info!(event = "shutdown", "server stopped");
    Ok(())
}

/// Resolves on SIGTERM or SIGINT so `axum` drains in-flight requests instead
/// of dropping connections mid-response.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        tracing::info!(event = "shutdown", signal = "SIGINT", "stopping");
                    }
                    _ = terminate.recv() => {
                        tracing::info!(event = "shutdown", signal = "SIGTERM", "stopping");
                    }
                }
            }
            Err(error) => {
                tracing::error!(
                    event = "shutdown",
                    signal = "SIGTERM",
                    error = %error,
                    "signal handler unavailable; waiting for SIGINT"
                );
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!(event = "shutdown", signal = "SIGINT", "stopping");
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!(event = "shutdown", signal = "SIGINT", "stopping");
    }
}
