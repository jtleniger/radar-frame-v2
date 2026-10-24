//! Route handlers for the index, settings, frame poll, and probes.

use std::sync::Arc;

use axum::extract::{Form, Query, State as AxumState};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::error::{Error, Result};
use crate::service::setup;
use crate::state::State;

use super::pages::{fields, html, FrameQuery, IndexPage, SettingsForm, SettingsPage};
use super::status::{readiness, Status};
use super::updater::Update;
use super::AppState;

pub async fn healthz() -> Response {
    text_response(StatusCode::OK, "ok\n")
}

pub async fn readyz(AxumState(app): AxumState<Arc<AppState>>) -> Response {
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

fn text_response(status: StatusCode, body: &str) -> Response {
    let mut response = (status, body.to_string()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

pub async fn index(AxumState(app): AxumState<Arc<AppState>>) -> Result<Response> {
    let status = Status::build(&app);
    html(IndexPage::new(
        &app.settings.nexrad_id,
        status,
        app.settings.timezone,
    ))
}

pub async fn settings(
    AxumState(app): AxumState<Arc<AppState>>,
    Form(form): Form<SettingsForm>,
) -> Result<Response> {
    let Some(mode_select) = crate::state::ModeSelect::parse(&form.mode_select) else {
        return Ok((StatusCode::BAD_REQUEST, "unknown mode-select").into_response());
    };
    let requested_mode = match form.mode.as_deref() {
        Some(value) => match crate::state::Mode::parse(value) {
            Some(mode) => Some(mode),
            None => return Ok((StatusCode::BAD_REQUEST, "unknown mode").into_response()),
        },
        None => None,
    };

    if !app.apply_settings(mode_select, requested_mode).await {
        return Err(Error::unavailable(60, "updater unavailable"));
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

pub async fn frame(
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
        return match png_file(&app.paths.output_img()).await {
            Ok(response) => response,
            Err(error) => error.into_response(),
        };
    }

    match app.update().await {
        Update::Published(frame) => frame.response(&headers),
        Update::Failed(error) => last_good(&app.snapshot().state, &headers, error),
        Update::Skipped => {
            let snapshot = app.snapshot();
            if snapshot.state.failures > 0 {
                // A cycle failed and the backoff has not elapsed. Keep serving
                // the last good frame rather than a bare 304, so a client that
                // restarted during the outage still gets a usable image.
                last_good(
                    &snapshot.state,
                    &headers,
                    Error::unavailable(60, "last cycle failed; retrying after backoff"),
                )
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
fn last_good(state: &State, headers: &HeaderMap, error: Error) -> Response {
    match state.frame.clone() {
        Some(frame) => {
            tracing::warn!(error = %error, "serving last good frame after failed cycle");
            frame.response(headers)
        }
        None => {
            tracing::warn!(error = %error, "no frame available");
            // The typed map supplies the status and `Retry-After`; a failed
            // cycle is always retryable, even when the cause was internal.
            Error::unavailable(error.retry_after().unwrap_or(60), "no frame available yet")
                .into_response()
        }
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

async fn png_file(path: &std::path::Path) -> Result<Response> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(([(header::CONTENT_TYPE, "image/png")], bytes).into_response()),
        Err(error) => {
            tracing::error!("{}: {error}", path.display());
            Err(Error::io_path(path, &error))
        }
    }
}

pub async fn stylesheet() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../../static/style.css"),
    )
        .into_response()
}
