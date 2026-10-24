//! The HTTP surface: `/`, `/settings`, `/frame`, and the stylesheet.
//!
//! Request and response semantics follow `server/api.py`, including the 304
//! polling contract and the auto/manual state machine.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Form, Query, State as AxumState};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::Duration;
use chrono_tz::Tz;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::clock::Clock;
use crate::config::Config;
use crate::engine::Renderer;
use crate::error::{Error, Result};
use crate::frame::PublishedFrame;
use crate::modes::{self, Deps};
use crate::paths::Paths;
use crate::sources::{ForecastProvider, RadarSource};
use crate::state::{Mode, ModeSelect, State};

pub struct AppState {
    pub config: Config,
    pub paths: Paths,
    pub timezone: Tz,
    pub clock: Arc<dyn Clock>,
    pub provider: Arc<dyn ForecastProvider>,
    pub radar: Arc<dyn RadarSource>,
    pub engine: Arc<dyn Renderer>,
    pub state: Mutex<State>,
}

impl AppState {
    pub fn new(
        config: Config,
        paths: Paths,
        clock: Arc<dyn Clock>,
        provider: Arc<dyn ForecastProvider>,
        radar: Arc<dyn RadarSource>,
        engine: Arc<dyn Renderer>,
    ) -> Result<Self> {
        let timezone = config.timezone("forecast", "timezone")?;

        // A frame left by the previous process is still the last good one;
        // publish it so the first poll survives an upstream outage.
        let mut state = State::new();
        state.frame = PublishedFrame::load(&paths.output_img()).map(Arc::new);

        Ok(AppState {
            config,
            paths,
            timezone,
            clock,
            provider,
            radar,
            engine,
            state: Mutex::new(state),
        })
    }

    /// Swap the mode from the NWS radar status. True when it changed.
    async fn update_mode(&self, state: &mut State) -> Result<bool> {
        let previous = state.mode;
        let status = self.provider.radar_status().await?;

        tracing::info!("radar status: up={} vcp={}", status.up, status.vcp);

        if !status.up {
            tracing::error!("radar down");
            state.mode = Mode::Clear;
        } else if status.vcp == -1 {
            tracing::error!("some issue with nws api");
            state.mode = Mode::Clear;
        } else if status.clear_air_mode() {
            state.mode = Mode::Clear;
        } else {
            state.mode = Mode::Storm;
        }

        Ok(state.mode != previous)
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/settings", post(settings))
        .route("/frame", get(frame))
        .route("/static/style.css", get(stylesheet))
        .with_state(state)
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
    let previous = state.mode;

    state.mode_select = mode_select;
    if mode_select == ModeSelect::Manual {
        state.manual_til = Some(app.clock.now() + Duration::days(1));
    } else {
        state.manual_til = None;
    }

    if mode_select == ModeSelect::Manual {
        if let Some(mode) = requested_mode {
            state.mode = mode;
        }
    }

    if previous != state.mode {
        state.last_updated = chrono::DateTime::UNIX_EPOCH;
    }

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

    if state.mode_select == ModeSelect::Manual && state.manual_til.is_none_or(|til| now > til) {
        state.mode_select = ModeSelect::Auto;
        tracing::info!("manual mode expired");
    }

    if state.mode_select == ModeSelect::Auto {
        match app.update_mode(&mut state).await {
            Ok(changed) => {
                if changed {
                    state.last_updated = chrono::DateTime::UNIX_EPOCH;
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

    if now > state.last_updated + state.mode.interval() {
        let deps = Deps {
            paths: &app.paths,
            config: &app.config,
            engine: &app.engine,
            provider: app.provider.as_ref(),
            radar: app.radar.as_ref(),
            now,
        };

        let mode = state.mode;
        let cycle = match modes::run(mode, &deps, &mut state).await {
            Ok(cycle) => cycle,
            Err(error) => {
                tracing::error!("{} mode failed: {error}", mode.name());
                return last_good(&state, &headers);
            }
        };

        let frame = match std::fs::read(app.paths.output_img()) {
            Ok(bytes) => match PublishedFrame::new(bytes, now, Some(mode), cycle.degraded) {
                Ok(frame) => frame,
                Err(error) => {
                    tracing::error!("{}: {error}", app.paths.output_img().display());
                    return last_good(&state, &headers);
                }
            },
            Err(error) => {
                tracing::error!("{}: {error}", app.paths.output_img().display());
                return last_good(&state, &headers);
            }
        };

        let frame = Arc::new(frame);
        state.last_updated = now;
        state.frame = Some(Arc::clone(&frame));
        drop(state);

        tracing::info!(
            "published frame: mode={} bytes={} degraded={:?}",
            mode.name(),
            frame.bytes.len(),
            frame.degraded
        );
        frame.response(&headers)
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
        .await
        .map_err(|e| Error(format!("server: {e}")))
}
