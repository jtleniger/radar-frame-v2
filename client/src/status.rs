//! Typed subset of `GET /status` used for scheduling and diagnostics.

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServerStatus {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub uptime_seconds: Option<i64>,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub mode_select: String,
    #[serde(default)]
    pub manual_til: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_updated: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_attempt: Option<DateTime<Utc>>,
    #[serde(default)]
    pub next_attempt: Option<DateTime<Utc>>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub radar_status: Option<RadarStatus>,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub ready_reason: String,
    #[serde(default)]
    pub frame: FrameStatus,
    #[serde(default)]
    pub last_cycle: Option<SampleStatus>,
    #[serde(default)]
    pub last_upstream: Option<SampleStatus>,
    #[serde(default)]
    pub counters: Counters,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RadarStatus {
    #[serde(default)]
    pub up: bool,
    #[serde(default)]
    pub vcp: i32,
    #[serde(default)]
    pub age_seconds: i64,
}

/// The published-frame object is untagged: either `{available: false}` or the
/// full report with age, byte count, and ETag.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FrameStatus {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub generated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub age_seconds: Option<i64>,
    #[serde(default)]
    pub bytes: Option<usize>,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub degraded: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SampleStatus {
    #[serde(default)]
    pub at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub age_seconds: i64,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub outcome: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Counters {
    #[serde(default)]
    pub requests_total: u64,
    #[serde(default)]
    pub cycles_total: u64,
    #[serde(default)]
    pub cycles_failed: u64,
    #[serde(default)]
    pub cycles_degraded: u64,
    #[serde(default)]
    pub upstream_total: u64,
    #[serde(default)]
    pub upstream_failed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_status_contract() {
        let status: ServerStatus = serde_json::from_str(
            r#"{
                "version": "0.1.0",
                "mode": "Clear",
                "mode_select": "Auto",
                "ready": true,
                "ready_reason": "ready",
                "failures": 0,
                "next_attempt": "2026-10-24T12:05:00Z",
                "frame": {
                    "available": true,
                    "age_seconds": 12,
                    "bytes": 34567,
                    "etag": "\"abc\"",
                    "mode": "Clear",
                    "degraded": null
                },
                "radar_status": {"up": true, "vcp": 31, "age_seconds": 3},
                "counters": {"cycles_total": 2, "cycles_failed": 0},
                "last_cycle": {"outcome": "ok", "age_seconds": 12, "elapsed_ms": 900}
            }"#,
        )
        .unwrap();

        assert_eq!(status.mode, "Clear");
        assert!(status.ready);
        assert!(status.frame.available);
        assert_eq!(status.frame.etag.as_deref(), Some("\"abc\""));
        assert_eq!(status.failures, 0);
        assert!(status.next_attempt.is_some());
        assert_eq!(status.radar_status.unwrap().vcp, 31);
        assert_eq!(status.last_cycle.unwrap().outcome, "ok");
    }

    #[test]
    fn parses_a_missing_frame() {
        let status: ServerStatus =
            serde_json::from_str(r#"{"ready": false, "frame": {"available": false}}"#).unwrap();
        assert!(!status.frame.available);
        assert!(status.frame.etag.is_none());
        assert_eq!(status.failures, 0);
    }
}
