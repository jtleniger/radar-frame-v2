//! `/status`: the operational summary consumed by the display and operators.

use std::sync::Arc;

use axum::extract::State as AxumState;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::service::setup;
use crate::state::{Mode, State};
use crate::telemetry::metrics::Sample;

use super::pages::fields;
use super::AppState;

pub async fn status(AxumState(app): AxumState<Arc<AppState>>) -> Response {
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

/// A ready process has published a frame that is no older than two mode
/// intervals and can still write to `data/`.
pub fn readiness(state: &State, now: DateTime<Utc>, data_writable: bool) -> (bool, &'static str) {
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
