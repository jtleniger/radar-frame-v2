//! Askama pages and the fields they share with the status document.

use askama::Template;
use axum::response::{Html, IntoResponse, Response};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::http::status::{age_seconds, FrameReport, SampleReport, Status};
use crate::state::State;
use crate::util::time::local;

pub struct Fields {
    pub mode: &'static str,
    pub mode_select: &'static str,
    pub last_updated: String,
    pub manual_til: String,
    pub has_manual_til: bool,
}

pub fn fields(state: &State, timezone: Tz) -> Fields {
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

/// One value in the home-page status grid.
pub struct Stat {
    pub label: &'static str,
    pub value: String,
    pub hint: String,
    /// Empty, `ok`, `warn`, or `error`; selects the value colour.
    pub tone: &'static str,
}

pub struct Group {
    pub title: &'static str,
    pub stats: Vec<Stat>,
}

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexPage {
    pub radar_id: String,
    pub version: &'static str,
    pub ready: bool,
    pub ready_reason: &'static str,
    pub mode: &'static str,
    pub mode_select: &'static str,
    pub manual_til: String,
    pub has_manual_til: bool,
    pub frame_available: bool,
    pub frame_caption: String,
    pub has_frame_degraded: bool,
    pub frame_degraded: String,
    pub groups: Vec<Group>,
}

impl IndexPage {
    pub fn new(radar_id: &str, status: Status, timezone: Tz) -> Self {
        let groups = groups(&status, timezone);
        let (frame_available, frame_caption, degraded) = frame_summary(&status, timezone);
        let manual_til = status
            .manual_til
            .and_then(|time| local(time, timezone))
            .unwrap_or_default();

        IndexPage {
            radar_id: radar_id.to_string(),
            version: status.version,
            ready: status.ready,
            ready_reason: status.ready_reason,
            mode: status.mode,
            mode_select: status.mode_select,
            has_manual_til: !manual_til.is_empty(),
            manual_til,
            frame_available,
            frame_caption,
            has_frame_degraded: degraded.is_some(),
            frame_degraded: degraded.unwrap_or_default(),
            groups,
        }
    }
}

#[derive(Template)]
#[template(path = "partials/settings.html")]
pub struct SettingsPage {
    pub mode: &'static str,
    pub mode_select: &'static str,
    pub manual_til: String,
    pub has_manual_til: bool,
}

pub fn html<T: Template>(page: T) -> Result<Response> {
    match page.render() {
        Ok(body) => Ok(Html(body).into_response()),
        Err(error) => {
            tracing::error!("template: {error}");
            Err(Error::from(error))
        }
    }
}

fn groups(status: &Status, timezone: Tz) -> Vec<Group> {
    vec![
        schedule_group(status, timezone),
        radar_group(status, timezone),
        service_group(status, timezone),
    ]
}

fn schedule_group(status: &Status, timezone: Tz) -> Group {
    let selection = match status.mode_select {
        "Manual" => status
            .manual_til
            .and_then(|time| local(time, timezone))
            .map(|time| format!("Manual until {time}"))
            .unwrap_or_else(|| "Manual".to_string()),
        _ => "Automatic".to_string(),
    };

    let updated = clock(status.last_updated, timezone);
    let updated_hint = status.last_updated.map(|time| {
        format!(
            "{} · {} ago",
            day(time, timezone),
            duration(age_seconds(status.now, time))
        )
    });
    let attempted = clock(status.last_attempt, timezone);
    let attempted_hint = status.last_attempt.map(|time| {
        format!(
            "{} · {} ago",
            day(time, timezone),
            duration(age_seconds(status.now, time))
        )
    });
    let next = clock(status.next_attempt, timezone);
    let countdown = status
        .next_attempt
        .map(|time| format!("{} · {}", day(time, timezone), countdown(status.now, time)));

    Group {
        title: "Schedule",
        stats: vec![
            stat("Mode", status.mode, selection, ""),
            stat(
                "Updated",
                updated.unwrap_or_else(|| "Never".to_string()),
                updated_hint.unwrap_or_default(),
                "",
            ),
            stat(
                "Attempted",
                attempted.unwrap_or_else(|| "Never".to_string()),
                attempted_hint.unwrap_or_default(),
                "",
            ),
            stat(
                "Next update",
                next.unwrap_or_else(|| "As soon as possible".to_string()),
                countdown.unwrap_or_default(),
                "",
            ),
            stat(
                "Failures",
                status.failures.to_string(),
                "In a row",
                if status.failures == 0 { "ok" } else { "error" },
            ),
        ],
    }
}

fn radar_group(status: &Status, timezone: Tz) -> Group {
    let (station, station_hint, station_tone) = match status.radar_status.as_ref() {
        Some(radar) => {
            let vcp = if radar.vcp < 0 {
                "VCP unknown".to_string()
            } else {
                format!("VCP {}", radar.vcp)
            };
            (
                if radar.up { "Online" } else { "Offline" },
                format!("{vcp} · checked {} ago", duration(radar.age_seconds)),
                if radar.up { "ok" } else { "error" },
            )
        }
        None => ("Unknown", "No status sample yet".to_string(), "warn"),
    };

    let volume = status
        .radar_volume
        .key
        .as_deref()
        .and_then(|key| key.rsplit('/').next())
        .unwrap_or("—");
    let volume_hint = match status.radar_volume.last_updated {
        Some(time) => format!(
            "Downloaded {} · {} ago",
            timestamp(Some(time), timezone).unwrap_or_default(),
            duration(age_seconds(status.now, time))
        ),
        None => "No volume downloaded yet".to_string(),
    };

    Group {
        title: "Radar",
        stats: vec![
            stat("Station", station, station_hint, station_tone),
            stat("Volume", volume, volume_hint, ""),
        ],
    }
}

fn service_group(status: &Status, timezone: Tz) -> Group {
    let counters = &status.counters;
    let uptime = status
        .uptime_seconds
        .map(duration)
        .unwrap_or_else(|| "—".to_string());
    let started = timestamp(status.started_at, timezone)
        .map(|time| format!("Since {time}"))
        .unwrap_or_default();
    let writable = if status.data_writable {
        "Writable"
    } else {
        "Read-only"
    };

    Group {
        title: "Service",
        stats: vec![
            stat("Uptime", uptime, started, ""),
            stat(
                "Data directory",
                writable,
                "Frame output location",
                if status.data_writable { "ok" } else { "error" },
            ),
            stat(
                "Requests",
                counters.requests_total.to_string(),
                "Since start",
                "",
            ),
            stat(
                "Cycles",
                counters.cycles_total.to_string(),
                format!(
                    "{} failed · {} degraded",
                    counters.cycles_failed, counters.cycles_degraded
                ),
                if counters.cycles_failed == 0 {
                    ""
                } else {
                    "warn"
                },
            ),
            stat(
                "Upstream",
                counters.upstream_total.to_string(),
                format!("{} failed", counters.upstream_failed),
                if counters.upstream_failed == 0 {
                    ""
                } else {
                    "warn"
                },
            ),
            sample_stat("Last cycle", status.last_cycle.as_ref(), status.now),
            sample_stat("Last upstream", status.last_upstream.as_ref(), status.now),
        ],
    }
}

fn sample_stat(label: &'static str, sample: Option<&SampleReport>, now: DateTime<Utc>) -> Stat {
    match sample {
        Some(sample) => stat(
            label,
            sample.outcome,
            format!(
                "{} ms · {} ago",
                sample.elapsed_ms,
                duration(age_seconds(now, sample.at))
            ),
            match sample.outcome {
                "ok" => "ok",
                "degraded" => "warn",
                _ => "error",
            },
        ),
        None => stat(label, "—", "No sample yet", ""),
    }
}

fn frame_summary(status: &Status, timezone: Tz) -> (bool, String, Option<String>) {
    match &status.frame {
        FrameReport::Published {
            mode,
            generated_at,
            age_seconds,
            bytes,
            degraded,
            ..
        } => {
            let mode = mode.map(|mode| format!("{mode} · ")).unwrap_or_default();
            let generated = generated_at
                .and_then(|time| local(time, timezone))
                .unwrap_or_else(|| "unknown time".to_string());
            (
                true,
                format!(
                    "{mode}{generated} · {} · {} ago",
                    size(*bytes),
                    duration(*age_seconds)
                ),
                degraded.clone(),
            )
        }
        FrameReport::Missing { .. } => (false, "Waiting for the first frame".to_string(), None),
    }
}

fn stat(
    label: &'static str,
    value: impl Into<String>,
    hint: impl Into<String>,
    tone: &'static str,
) -> Stat {
    Stat {
        label,
        value: value.into(),
        hint: hint.into(),
        tone,
    }
}

fn timestamp(time: Option<DateTime<Utc>>, timezone: Tz) -> Option<String> {
    time.and_then(|time| local(time, timezone))
}

/// Time of day for a compact stat value; the date goes in the hint.
fn clock(time: Option<DateTime<Utc>>, timezone: Tz) -> Option<String> {
    time.map(|time| time.with_timezone(&timezone).format("%I:%M %p").to_string())
}

fn day(time: DateTime<Utc>, timezone: Tz) -> String {
    time.with_timezone(&timezone).format("%b %d").to_string()
}

fn countdown(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    if then <= now {
        "Due now".to_string()
    } else {
        format!("In {}", duration((then - now).num_seconds()))
    }
}

/// Compact age, e.g. `12s`, `4m 05s`, `2h 03m`, `3d 04h`.
pub fn duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    let secs = seconds % 60;

    if days > 0 {
        format!("{days}d {hours:02}h")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m {secs:02}s")
    } else {
        format!("{secs}s")
    }
}

fn size(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;

    let bytes = bytes as f64;
    if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.0} KB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[derive(Deserialize)]
pub struct SettingsForm {
    #[serde(rename = "mode-select")]
    pub mode_select: String,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Deserialize)]
pub struct FrameQuery {
    pub debug: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{duration, size};

    #[test]
    fn ages_roll_up_to_the_largest_unit() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(59), "59s");
        assert_eq!(duration(60), "1m 00s");
        assert_eq!(duration(3_599), "59m 59s");
        assert_eq!(duration(3_600), "1h 00m");
        assert_eq!(duration(86_400), "1d 00h");
        assert_eq!(duration(-5), "0s");
    }

    #[test]
    fn sizes_use_readable_units() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(2_048), "2 KB");
        assert_eq!(size(1_572_864), "1.5 MB");
    }
}
