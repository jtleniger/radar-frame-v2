//! The HTTP surface: `/`, `/settings`, `/frame`, and the stylesheet.
//!
//! Request and response semantics follow `server/api.py`, including the 304
//! polling contract and the auto/manual state machine.

use std::sync::Arc;
use std::time::Instant;

use askama::Template;
use axum::extract::{Form, Query, State as AxumState};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::clock::Clock;
use crate::config::Config;
use crate::engine::Renderer;
use crate::error::{Error, Result};
use crate::frame::PublishedFrame;
use crate::metrics::{elapsed_ms, Metrics, Outcome, Sample};
use crate::modes::{self, Deps};
use crate::paths::Paths;
use crate::setup;
use crate::sources::{RadarSource, Sources};
use crate::state::{Mode, ModeSelect, State};

/// Upper bound for one `/frame` render cycle. Each upstream call is bounded
/// by the HTTP client timeouts; this caps the whole sequence plus rendering so
/// one poll cannot hold the state lock forever.
const CYCLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(180);

pub struct AppState {
    pub config: Config,
    pub paths: Paths,
    pub timezone: Tz,
    pub clock: Arc<dyn Clock>,
    pub sources: Sources,
    pub radar: Arc<dyn RadarSource>,
    pub engine: Arc<dyn Renderer>,
    pub metrics: Metrics,
    pub state: Mutex<State>,
}

impl AppState {
    pub fn new(
        config: Config,
        paths: Paths,
        clock: Arc<dyn Clock>,
        sources: Sources,
        radar: Arc<dyn RadarSource>,
        engine: Arc<dyn Renderer>,
    ) -> Result<Self> {
        let timezone = config.timezone("forecast", "timezone")?;
        let started_at = clock.now();

        // A frame left by the previous process is still the last good one;
        // publish it so the first poll survives an upstream outage. The
        // persisted schedule is restored with it, so a restart does not
        // immediately repeat upstream work.
        let mut state = State::load(&paths.state_file());
        state.frame = PublishedFrame::load(&paths.output_img()).map(Arc::new);

        Ok(AppState {
            config,
            paths,
            timezone,
            clock,
            sources,
            radar,
            engine,
            metrics: Metrics::new(started_at),
            state: Mutex::new(state),
        })
    }

    /// Swap the mode from the NWS radar status. True when it changed.
    ///
    /// The sample is reused for [`crate::state::STATUS_TTL`]; a failure keeps
    /// the last known status and mode and is not retried until the TTL passes.
    /// The state lock is held across the call, so a second concurrent poll
    /// finds the refreshed timestamp and takes the cached path.
    async fn update_mode(
        &self,
        state: &mut State,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        if now < state.status_next_attempt {
            return Ok(false);
        }

        let started = Instant::now();
        let status = self.sources.status.radar_status().await;
        self.metrics
            .upstream(now, started.elapsed(), status.is_ok());

        state.status_checked_at = now;
        state.status_next_attempt = now + crate::state::STATUS_TTL;

        let status = match status {
            Ok(status) => status,
            Err(error) => {
                // Keep the last-known-good status and the current mode.
                return Err(error);
            }
        };

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
}

/// Write the schedule and provenance after a state change. A failure is
/// reported but never fails the request: the in-memory state remains correct
/// and the next mutation retries.
fn persist(paths: &Paths, state: &State) {
    if let Err(error) = state.save(&paths.state_file()) {
        tracing::warn!(event = "state", error = %error, "could not persist state");
    }
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
    let (ready, reason) = {
        let state = app.state.lock().await;
        readiness(&state, now, setup::data_writable(&app.paths))
    };

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
    let (
        fields,
        mode,
        manual_til,
        last_updated,
        frame,
        last_attempt,
        next_attempt,
        failures,
        radar_status,
        status_checked_at,
        radar_key,
        radar_last_updated,
        ready,
        reason,
    ) = {
        let state = app.state.lock().await;
        let (ready, reason) = readiness(&state, now, writable);
        (
            fields(&state, app.timezone),
            state.mode,
            state.manual_til,
            state.last_updated,
            state.frame.clone(),
            state.last_attempt,
            state.next_attempt,
            state.failures,
            state.radar_status,
            state.status_checked_at,
            state.radar_key.clone(),
            state.radar_last_updated,
            ready,
            reason,
        )
    };

    let frame = match frame {
        Some(frame) => json!({
            "available": true,
            "generated_at": timestamp(frame.generated_at),
            "age_seconds": age_seconds(now, frame.generated_at),
            "bytes": frame.bytes.len(),
            "etag": frame.etag,
            "mode": frame.mode.map(Mode::name),
            "degraded": frame.degraded,
        }),
        None => json!({ "available": false }),
    };

    let body = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "started_at": app.metrics.started_at().and_then(timestamp),
        "uptime_seconds": app.metrics.started_at().map(|started| age_seconds(now, started)),
        "mode": mode.name(),
        "mode_select": fields.mode_select,
        "manual_til": manual_til.and_then(timestamp),
        "last_updated": timestamp(last_updated),
        "last_attempt": timestamp(last_attempt),
        "next_attempt": timestamp(next_attempt),
        "failures": failures,
        "radar_status": radar_status.map(|status| json!({
            "up": status.up,
            "vcp": status.vcp,
            "checked_at": timestamp(status_checked_at),
            "age_seconds": age_seconds(now, status_checked_at),
        })),
        "radar_volume": {
            "key": radar_key,
            "last_updated": timestamp(radar_last_updated),
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
fn readiness(
    state: &State,
    now: chrono::DateTime<chrono::Utc>,
    data_writable: bool,
) -> (bool, &'static str) {
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

fn timestamp(time: chrono::DateTime<chrono::Utc>) -> Option<String> {
    (time != chrono::DateTime::UNIX_EPOCH).then(|| time.to_rfc3339())
}

fn age_seconds(now: chrono::DateTime<chrono::Utc>, then: chrono::DateTime<chrono::Utc>) -> i64 {
    now.signed_duration_since(then).num_seconds().max(0)
}

fn sample(sample: Option<Sample>, now: chrono::DateTime<chrono::Utc>) -> Value {
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
    let state = app.state.lock().await;
    let fields = fields(&state, app.timezone);

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

    tracing::info!("Set mode select to {}", mode_select.name());

    let mut state = app.state.lock().await;
    state.apply_settings(mode_select, requested_mode, app.clock.now());
    persist(&app.paths, &state);

    let fields = fields(&state, app.timezone);
    drop(state);

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
        let published = app.state.lock().await.frame.clone();
        if let Some(frame) = published {
            return frame.response(&headers);
        }
        return png_file(&app.paths.output_img()).await;
    }

    let now = app.clock.now();
    let mut state = app.state.lock().await;

    tracing::info!("last updated: {}", state.last_updated);
    tracing::info!("now: {now}");

    if state.expire_manual(now) {
        tracing::info!("manual mode expired");
        persist(&app.paths, &state);
    }

    if state.mode_select == ModeSelect::Auto {
        match app.update_mode(&mut state, now).await {
            Ok(changed) => {
                if changed {
                    state.reschedule_now();
                    persist(&app.paths, &state);
                }
            }
            Err(error) => {
                // A status outage must not block rendering or change the
                // last known mode; the cycle below still runs on schedule.
                tracing::error!("radar status: {error}");
            }
        }
    } else {
        tracing::info!("manual mode, maintaining state");
    }

    if state.cycle_due(now) {
        let deps = Deps {
            paths: &app.paths,
            config: &app.config,
            engine: &app.engine,
            forecast: app.sources.forecast.as_ref(),
            alerts: app.sources.alerts.as_ref(),
            radar: app.radar.as_ref(),
            metrics: &app.metrics,
            now,
        };

        let mode = state.mode;
        let cycle_started = Instant::now();
        let cycle =
            match tokio::time::timeout(CYCLE_DEADLINE, modes::run(mode, &deps, &mut state)).await {
                Ok(result) => result,
                Err(_) => Err(Error::new(format!(
                    "cycle exceeded the {} s deadline",
                    CYCLE_DEADLINE.as_secs()
                ))),
            };
        let cycle = match cycle {
            Ok(cycle) => cycle,
            Err(error) => {
                let elapsed = cycle_started.elapsed();
                app.metrics.cycle(app.clock.now(), elapsed, Outcome::Failed);
                tracing::warn!(
                    event = "cycle",
                    mode = mode.name(),
                    outcome = "failed",
                    elapsed_ms = elapsed_ms(elapsed),
                    error = %error,
                    "cycle failed"
                );
                tracing::error!("{} mode failed: {error}", mode.name());
                state.cycle_failed(now, mode);
                persist(&app.paths, &state);
                return last_good(&state, &headers);
            }
        };

        let frame = match std::fs::read(app.paths.output_img()) {
            Ok(bytes) => {
                match PublishedFrame::new(bytes, now, Some(mode), cycle.degraded.clone()) {
                    Ok(frame) => frame,
                    Err(error) => {
                        let elapsed = cycle_started.elapsed();
                        app.metrics.cycle(app.clock.now(), elapsed, Outcome::Failed);
                        tracing::warn!(
                            event = "cycle",
                            mode = mode.name(),
                            outcome = "failed",
                            elapsed_ms = elapsed_ms(elapsed),
                            error = %error,
                            "cycle failed"
                        );
                        tracing::error!("{}: {error}", app.paths.output_img().display());
                        state.cycle_failed(now, mode);
                        persist(&app.paths, &state);
                        return last_good(&state, &headers);
                    }
                }
            }
            Err(error) => {
                let elapsed = cycle_started.elapsed();
                app.metrics.cycle(app.clock.now(), elapsed, Outcome::Failed);
                tracing::warn!(
                    event = "cycle",
                    mode = mode.name(),
                    outcome = "failed",
                    elapsed_ms = elapsed_ms(elapsed),
                    error = %error,
                    "cycle failed"
                );
                tracing::error!("{}: {error}", app.paths.output_img().display());
                state.cycle_failed(now, mode);
                persist(&app.paths, &state);
                return last_good(&state, &headers);
            }
        };

        let frame = Arc::new(frame);
        state.cycle_succeeded(now, mode);
        state.frame = Some(Arc::clone(&frame));
        persist(&app.paths, &state);
        drop(state);

        let outcome = if cycle.degraded.is_some() {
            Outcome::Degraded
        } else {
            Outcome::Ok
        };
        let elapsed = cycle_started.elapsed();
        app.metrics.cycle(app.clock.now(), elapsed, outcome);
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
        frame.response(&headers)
    } else if state.failures > 0 {
        // A cycle failed and the backoff has not elapsed. Keep serving the
        // last good frame rather than a bare 304, so a client that restarted
        // during the outage still gets a usable image.
        last_good(&state, &headers)
    } else {
        // The polling contract: no new frame is due, so tell the client to
        // keep what it has. Include the tag for clients that track it.
        let etag = state.frame.as_ref().map(|frame| frame.etag.clone());
        drop(state);

        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Some(etag) = etag {
            if let Ok(value) = HeaderValue::from_str(&etag) {
                response.headers_mut().insert(header::ETAG, value);
            }
        }
        response
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
