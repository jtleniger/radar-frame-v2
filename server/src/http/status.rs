//! `/status`: the operational summary consumed by the display, operators, and
//! the home page.
//!
//! The home page renders the same [`Status`] value the JSON endpoint serves,
//! so the two views cannot drift. Ages and readiness are computed at build
//! time from the snapshot and the clock passed in.

use std::sync::Arc;

use axum::extract::State as AxumState;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::service::setup;
use crate::state::{Mode, State};

use super::AppState;

/// Everything `/status` exposes. Fields are copied or derived from one
/// snapshot, so a report is internally consistent.
#[derive(Debug, Serialize)]
pub struct Status {
    /// Report time. Used for relative ages on the home page; not serialized.
    #[serde(skip)]
    pub now: DateTime<Utc>,
    pub version: &'static str,
    pub started_at: Option<DateTime<Utc>>,
    pub uptime_seconds: Option<i64>,
    pub mode: &'static str,
    pub mode_select: &'static str,
    pub manual_til: Option<DateTime<Utc>>,
    pub last_updated: Option<DateTime<Utc>>,
    pub last_attempt: Option<DateTime<Utc>>,
    pub next_attempt: Option<DateTime<Utc>>,
    pub failures: u32,
    pub radar_status: Option<RadarReport>,
    pub radar_volume: RadarVolume,
    pub ready: bool,
    pub ready_reason: &'static str,
    pub data_writable: bool,
    pub frame: FrameReport,
    pub counters: Counters,
    pub last_cycle: Option<SampleReport>,
    pub last_upstream: Option<SampleReport>,
}

#[derive(Debug, Serialize)]
pub struct RadarReport {
    pub up: bool,
    pub vcp: i32,
    pub checked_at: Option<DateTime<Utc>>,
    pub age_seconds: i64,
}

#[derive(Debug, Serialize)]
pub struct RadarVolume {
    pub key: Option<String>,
    pub last_updated: Option<DateTime<Utc>>,
}

/// The published frame. `available: false` is a bare object, matching the
/// original JSON shape; the published variant carries the full details.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum FrameReport {
    Published {
        available: bool,
        generated_at: Option<DateTime<Utc>>,
        age_seconds: i64,
        bytes: usize,
        etag: String,
        mode: Option<&'static str>,
        degraded: Option<String>,
    },
    Missing {
        available: bool,
    },
}

#[derive(Debug, Serialize)]
pub struct Counters {
    pub requests_total: u64,
    pub cycles_total: u64,
    pub cycles_failed: u64,
    pub cycles_degraded: u64,
    pub upstream_total: u64,
    pub upstream_failed: u64,
}

#[derive(Debug, Serialize)]
pub struct SampleReport {
    pub at: DateTime<Utc>,
    pub age_seconds: i64,
    pub elapsed_ms: u64,
    pub outcome: &'static str,
}

impl Status {
    pub fn build(app: &AppState) -> Self {
        let now = app.clock.now();
        let writable = setup::data_writable(&app.paths);
        let snapshot = app.snapshot();
        let state = &snapshot.state;
        let (ready, ready_reason) = readiness(state, now, writable);

        let frame = match state.frame.as_ref() {
            Some(frame) => FrameReport::Published {
                available: true,
                generated_at: timestamp(frame.generated_at),
                age_seconds: age_seconds(now, frame.generated_at),
                bytes: frame.bytes.len(),
                etag: frame.etag.clone(),
                mode: frame.mode.map(Mode::name),
                degraded: frame.degraded.clone(),
            },
            None => FrameReport::Missing { available: false },
        };

        Status {
            now,
            version: env!("CARGO_PKG_VERSION"),
            started_at: app.metrics.started_at().and_then(timestamp),
            uptime_seconds: app
                .metrics
                .started_at()
                .map(|started| age_seconds(now, started)),
            mode: state.mode.name(),
            mode_select: state.mode_select.name(),
            manual_til: state.manual_til.and_then(timestamp),
            last_updated: timestamp(state.last_updated),
            last_attempt: timestamp(state.last_attempt),
            next_attempt: timestamp(state.next_attempt),
            failures: state.failures,
            radar_status: state.radar_status.map(|status| RadarReport {
                up: status.up,
                vcp: status.vcp,
                checked_at: timestamp(state.status_checked_at),
                age_seconds: age_seconds(now, state.status_checked_at),
            }),
            radar_volume: RadarVolume {
                key: state.radar_key.clone(),
                last_updated: timestamp(state.radar_last_updated),
            },
            ready,
            ready_reason,
            data_writable: writable,
            frame,
            counters: Counters {
                requests_total: app.metrics.requests_total(),
                cycles_total: app.metrics.cycles_total(),
                cycles_failed: app.metrics.cycles_failed(),
                cycles_degraded: app.metrics.cycles_degraded(),
                upstream_total: app.metrics.upstream_total(),
                upstream_failed: app.metrics.upstream_failed(),
            },
            last_cycle: app.metrics.last_cycle().map(|sample| SampleReport {
                at: sample.at,
                age_seconds: age_seconds(now, sample.at),
                elapsed_ms: sample.elapsed_ms,
                outcome: sample.outcome.as_str(),
            }),
            last_upstream: app.metrics.last_upstream().map(|sample| SampleReport {
                at: sample.at,
                age_seconds: age_seconds(now, sample.at),
                elapsed_ms: sample.elapsed_ms,
                outcome: sample.outcome.as_str(),
            }),
        }
    }
}

pub async fn status(AxumState(app): AxumState<Arc<AppState>>) -> Response {
    let body = Status::build(&app);

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

fn timestamp(time: DateTime<Utc>) -> Option<DateTime<Utc>> {
    (time != DateTime::UNIX_EPOCH).then_some(time)
}

pub fn age_seconds(now: DateTime<Utc>, then: DateTime<Utc>) -> i64 {
    now.signed_duration_since(then).num_seconds().max(0)
}
