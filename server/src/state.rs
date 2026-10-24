//! Process state and the mode schedule, ported from `state/state.py`,
//! `modes/mode.py`, and `modes/mode_select.py`.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;

use crate::backoff;
use crate::frame::PublishedFrame;
use crate::sources::{Forecast, RadarStatus};
use crate::view::forecast::Alert;

/// How long a radar-status sample is reused. This is the interim single-flight
/// cache until scheduling moves out of the request path.
pub const STATUS_TTL: Duration = Duration::seconds(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Storm,
    Clear,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Storm => "Storm",
            Mode::Clear => "Clear",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Storm" => Some(Mode::Storm),
            "Clear" => Some(Mode::Clear),
            _ => None,
        }
    }

    /// Storm every 10 minutes, clear every 30.
    pub fn interval(self) -> Duration {
        match self {
            Mode::Storm => Duration::minutes(10),
            Mode::Clear => Duration::minutes(30),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModeSelect {
    Auto,
    Manual,
}

impl ModeSelect {
    pub fn name(self) -> &'static str {
        match self {
            ModeSelect::Auto => "Auto",
            ModeSelect::Manual => "Manual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Auto" => Some(ModeSelect::Auto),
            "Manual" => Some(ModeSelect::Manual),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct State {
    pub last_updated: DateTime<Utc>,
    pub radar_last_updated: DateTime<Utc>,
    pub mode: Mode,
    pub mode_select: ModeSelect,
    pub manual_til: Option<DateTime<Utc>>,
    /// Last frame that rendered and validated successfully. Kept across
    /// upstream failures so `/frame` can serve it instead of `503`.
    pub frame: Option<Arc<PublishedFrame>>,
    /// Last good upstream payloads, served while a source is failing. `Arc`
    /// keeps the cache from cloning forecasts and alerts on every frame.
    pub forecast: Option<Arc<Forecast>>,
    pub alerts: Option<Arc<[Alert]>>,
    pub radar_status: Option<RadarStatus>,
    /// When the radar status was last requested, and when it may be requested
    /// again. The request path holds the state lock across the call, so the
    /// second of two concurrent polls finds the refreshed timestamp and reuses
    /// the cached value.
    pub status_checked_at: DateTime<Utc>,
    pub status_next_attempt: DateTime<Utc>,
    /// When the last update cycle was attempted, successful or not, and when
    /// the next one is due. A failed cycle backs off instead of retrying on
    /// every poll.
    pub last_attempt: DateTime<Utc>,
    pub next_attempt: DateTime<Utc>,
    pub failures: u32,
}

impl State {
    pub fn new() -> Self {
        State {
            last_updated: DateTime::UNIX_EPOCH,
            radar_last_updated: DateTime::UNIX_EPOCH,
            mode: Mode::Clear,
            mode_select: ModeSelect::Auto,
            manual_til: None,
            frame: None,
            forecast: None,
            alerts: None,
            radar_status: None,
            status_checked_at: DateTime::UNIX_EPOCH,
            status_next_attempt: DateTime::UNIX_EPOCH,
            last_attempt: DateTime::UNIX_EPOCH,
            next_attempt: DateTime::UNIX_EPOCH,
            failures: 0,
        }
    }

    /// True when the schedule allows the next update cycle.
    pub fn cycle_due(&self, now: DateTime<Utc>) -> bool {
        now >= self.next_attempt
    }

    /// Record a cycle that published a frame. The next one is due after the
    /// mode's normal interval and the failure streak is cleared.
    pub fn cycle_succeeded(&mut self, now: DateTime<Utc>, mode: Mode) {
        self.last_attempt = now;
        self.last_updated = now;
        self.failures = 0;
        self.next_attempt = now + mode.interval();
    }

    /// Record a failed cycle and schedule a retry with exponential backoff.
    pub fn cycle_failed(&mut self, now: DateTime<Utc>, mode: Mode) {
        self.last_attempt = now;
        self.failures = self.failures.saturating_add(1);
        self.next_attempt = now
            + backoff::delay(
                self.failures,
                mode.interval(),
                now.timestamp_millis() as u64,
            );
    }

    /// Make the next poll run a cycle immediately, e.g. after a mode change.
    pub fn reschedule_now(&mut self) {
        self.last_updated = DateTime::UNIX_EPOCH;
        self.last_attempt = DateTime::UNIX_EPOCH;
        self.next_attempt = DateTime::UNIX_EPOCH;
        self.failures = 0;
    }

    pub fn last_updated_str(&self, timezone: Tz) -> Option<String> {
        local_string(self.last_updated, timezone)
    }

    pub fn manual_til_str(&self, timezone: Tz) -> Option<String> {
        self.manual_til
            .and_then(|time| local_string(time, timezone))
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

fn local_string(time: DateTime<Utc>, timezone: Tz) -> Option<String> {
    if time == DateTime::UNIX_EPOCH {
        return None;
    }

    Some(
        time.with_timezone(&timezone)
            .format("%Y-%m-%d %I:%M%p")
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn epoch_formats_as_none() {
        let state = State::new();
        assert_eq!(state.last_updated_str(chrono_tz::America::Chicago), None);
        assert_eq!(state.manual_til_str(chrono_tz::America::Chicago), None);
    }

    #[test]
    fn times_are_formatted_in_the_configured_zone() {
        let mut state = State::new();
        state.last_updated = at("2026-10-21T15:30:00Z");
        state.manual_til = Some(at("2026-10-22T15:30:00Z"));

        assert_eq!(
            state.last_updated_str(chrono_tz::America::Chicago).unwrap(),
            "2026-10-21 10:30AM"
        );
        assert_eq!(
            state.manual_til_str(chrono_tz::America::Chicago).unwrap(),
            "2026-10-22 10:30AM"
        );
    }

    #[test]
    fn intervals_match_the_python_schedule() {
        assert_eq!(Mode::Storm.interval(), Duration::minutes(10));
        assert_eq!(Mode::Clear.interval(), Duration::minutes(30));
    }
}
