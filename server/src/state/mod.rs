//! Process state and the mode schedule, ported from `state/state.py`,
//! `modes/mode.py`, and `modes/mode_select.py`.

mod mode;

pub use mode::{mode_for, Mode, ModeSelect, MODE_MIN_DWELL, MODE_SWITCH_SAMPLES};

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::service::frame::PublishedFrame;
use crate::sources::{Forecast, RadarStatus};
use crate::util::{backoff, fsutil};
use crate::view::forecast::Alert;

/// How long a radar-status sample is reused. This is the interim single-flight
/// cache until scheduling moves out of the request path.
pub const STATUS_TTL: Duration = Duration::seconds(60);

/// On-disk schema version. A file from another version is ignored so a
/// downgrade cannot adopt fields it does not understand.
const STATE_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct State {
    pub last_updated: DateTime<Utc>,
    pub radar_last_updated: DateTime<Utc>,
    /// Archive key of [`Self::radar_last_updated`], kept for provenance and
    /// restored with it so a restart does not re-download the same volume.
    pub radar_key: Option<String>,
    pub mode: Mode,
    pub mode_select: ModeSelect,
    pub manual_til: Option<DateTime<Utc>>,
    /// When the current mode was accepted, for the minimum-dwell rule.
    pub mode_since: DateTime<Utc>,
    /// Candidate mode and how many consecutive status samples have named it.
    pub pending_mode: Option<Mode>,
    pub pending_samples: u32,
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
            radar_key: None,
            mode: Mode::Clear,
            mode_select: ModeSelect::Auto,
            manual_til: None,
            mode_since: DateTime::UNIX_EPOCH,
            pending_mode: None,
            pending_samples: 0,
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

    /// Apply one radar-status sample. Returns true when the mode changed.
    ///
    /// The first sample after startup establishes the mode immediately. Later
    /// changes need [`MODE_SWITCH_SAMPLES`] consecutive samples naming the new
    /// mode *and* [`MODE_MIN_DWELL`] since the last accepted change, so a
    /// station flapping between Operate and Standby cannot drive a render on
    /// every poll.
    pub fn observe_status(&mut self, status: RadarStatus, now: DateTime<Utc>) -> bool {
        let bootstrap = self.radar_status.is_none();
        self.radar_status = Some(status);
        let desired = mode_for(status);

        if desired == self.mode {
            self.pending_mode = None;
            self.pending_samples = 0;
            return false;
        }

        if self.pending_mode == Some(desired) {
            self.pending_samples = self.pending_samples.saturating_add(1);
        } else {
            self.pending_mode = Some(desired);
            self.pending_samples = 1;
        }

        let dwell_elapsed = now.signed_duration_since(self.mode_since) >= MODE_MIN_DWELL;
        if !bootstrap && (self.pending_samples < MODE_SWITCH_SAMPLES || !dwell_elapsed) {
            return false;
        }

        self.set_mode(desired, now);
        true
    }

    /// Apply a `/settings` change. Returns true when the next poll must run a
    /// cycle immediately because the mode changed.
    pub fn apply_settings(
        &mut self,
        mode_select: ModeSelect,
        requested: Option<Mode>,
        now: DateTime<Utc>,
    ) -> bool {
        let previous = self.mode;

        self.mode_select = mode_select;
        if mode_select == ModeSelect::Manual {
            self.manual_til = Some(now + Duration::days(1));
            if let Some(mode) = requested {
                self.set_mode(mode, now);
            }
        } else {
            self.manual_til = None;
        }

        // An operator decision supersedes any half-collected status evidence.
        self.pending_mode = None;
        self.pending_samples = 0;

        if previous != self.mode {
            self.reschedule_now();
            true
        } else {
            false
        }
    }

    /// Expire a manual selection after its day has passed, exactly as the
    /// Python state did (the old timestamp is left in place). Returns true when
    /// the selection expired.
    pub fn expire_manual(&mut self, now: DateTime<Utc>) -> bool {
        if self.mode_select != ModeSelect::Manual {
            return false;
        }
        if self.manual_til.is_some_and(|til| now <= til) {
            return false;
        }

        self.mode_select = ModeSelect::Auto;
        self.pending_mode = None;
        self.pending_samples = 0;
        true
    }

    /// Load the persisted schedule, mode, and radar provenance. A missing,
    /// unreadable, or incompatible file yields a fresh state; the next cycle
    /// rebuilds it.
    pub fn load(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return State::new(),
            Err(error) => {
                tracing::warn!(
                    "event=state error={error} path={} storage unreadable",
                    path.display()
                );
                return State::new();
            }
        };

        let persisted: Persisted = match serde_json::from_slice(&bytes) {
            Ok(persisted) => persisted,
            Err(error) => {
                tracing::warn!(
                    "event=state error={error} path={} ignoring persisted state",
                    path.display()
                );
                return State::new();
            }
        };
        if persisted.version != STATE_VERSION {
            tracing::warn!(
                "event=state version={} expected={} path={} ignoring persisted state",
                persisted.version,
                STATE_VERSION,
                path.display()
            );
            return State::new();
        }

        let mut state = State::new();
        state.mode = Mode::parse(&persisted.mode).unwrap_or_else(|| {
            tracing::warn!(
                "event=state mode={} unknown mode; using Clear",
                persisted.mode
            );
            Mode::Clear
        });
        state.mode_select = ModeSelect::parse(&persisted.mode_select).unwrap_or_else(|| {
            tracing::warn!(
                "event=state mode_select={} unknown selection; using Auto",
                persisted.mode_select
            );
            ModeSelect::Auto
        });
        state.manual_til = persisted.manual_til;
        state.last_updated = persisted.last_updated;
        state.last_attempt = persisted.last_attempt;
        state.next_attempt = persisted.next_attempt;
        state.failures = persisted.failures;
        state.radar_last_updated = persisted.radar_last_updated;
        state.radar_key = persisted.radar_key;
        state.mode_since = persisted.last_updated;

        tracing::info!(
            event = "state",
            path = %path.display(),
            mode = state.mode.name(),
            mode_select = state.mode_select.name(),
            next_attempt = %state.next_attempt,
            radar_key = state.radar_key.as_deref().unwrap_or("-"),
            "restored state"
        );
        state
    }

    /// Write the schedule, mode, and radar provenance atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let persisted = Persisted {
            version: STATE_VERSION,
            mode: self.mode.name().to_string(),
            mode_select: self.mode_select.name().to_string(),
            manual_til: self.manual_til,
            last_updated: self.last_updated,
            last_attempt: self.last_attempt,
            next_attempt: self.next_attempt,
            failures: self.failures,
            radar_last_updated: self.radar_last_updated,
            radar_key: self.radar_key.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&persisted)?;
        fsutil::atomic_write(path, &bytes)?;
        Ok(())
    }

    fn set_mode(&mut self, mode: Mode, now: DateTime<Utc>) {
        self.mode = mode;
        self.mode_since = now;
        self.pending_mode = None;
        self.pending_samples = 0;
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

/// The subset of [`State`] that survives a restart. Caches, the published
/// frame, and the radar-status sample are rebuilt from upstream or from the
/// files on disk.
#[derive(Debug, Serialize, Deserialize)]
struct Persisted {
    version: u32,
    mode: String,
    mode_select: String,
    manual_til: Option<DateTime<Utc>>,
    last_updated: DateTime<Utc>,
    last_attempt: DateTime<Utc>,
    next_attempt: DateTime<Utc>,
    failures: u32,
    radar_last_updated: DateTime<Utc>,
    #[serde(default)]
    radar_key: Option<String>,
}

fn local_string(time: DateTime<Utc>, timezone: Tz) -> Option<String> {
    crate::util::time::local(time, timezone)
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
            "Oct 21, 10:30 AM"
        );
        assert_eq!(
            state.manual_til_str(chrono_tz::America::Chicago).unwrap(),
            "Oct 22, 10:30 AM"
        );
    }

    #[test]
    fn intervals_match_the_python_schedule() {
        assert_eq!(Mode::Storm.interval(), Duration::minutes(10));
        assert_eq!(Mode::Clear.interval(), Duration::minutes(30));
    }

    fn status(up: bool, vcp: i32) -> RadarStatus {
        RadarStatus { up, vcp }
    }

    fn storm_status() -> RadarStatus {
        status(true, 212)
    }

    fn clear_status() -> RadarStatus {
        status(false, -1)
    }

    #[test]
    fn the_first_status_sample_establishes_the_mode() {
        let mut state = State::new();
        let now = at("2026-10-21T15:30:00Z");

        assert!(state.observe_status(storm_status(), now));
        assert_eq!(state.mode, Mode::Storm);
        assert_eq!(state.mode_since, now);
    }

    #[test]
    fn a_mode_change_needs_three_consecutive_samples() {
        let mut state = State::new();
        let start = at("2026-10-21T15:30:00Z");
        assert!(!state.observe_status(clear_status(), start));

        for minutes in 1..MODE_SWITCH_SAMPLES as i64 {
            assert!(
                !state.observe_status(storm_status(), start + Duration::minutes(minutes)),
                "sample {minutes} must not switch"
            );
        }
        assert_eq!(state.mode, Mode::Clear);

        assert!(state.observe_status(
            storm_status(),
            start + Duration::minutes(MODE_SWITCH_SAMPLES as i64)
        ));
        assert_eq!(state.mode, Mode::Storm);
        assert_eq!(state.pending_mode, None);
        assert_eq!(state.pending_samples, 0);
    }

    #[test]
    fn flapping_status_never_switches() {
        let mut state = State::new();
        let start = at("2026-10-21T15:30:00Z");
        assert!(!state.observe_status(clear_status(), start));

        for sample in 1..=10i64 {
            let status = if sample % 2 == 0 {
                clear_status()
            } else {
                storm_status()
            };
            assert!(!state.observe_status(status, start + Duration::minutes(sample)));
        }
        assert_eq!(state.mode, Mode::Clear);
    }

    #[test]
    fn a_mode_change_waits_for_the_minimum_dwell() {
        let mut state = State::new();
        let start = at("2026-10-21T15:30:00Z");
        assert!(!state.observe_status(clear_status(), start));
        state.mode_since = start;

        // The samples agree after three polls, but the current mode has not
        // been held for the dwell period yet.
        for minutes in 1..=3 {
            assert!(!state.observe_status(storm_status(), start + Duration::minutes(minutes)));
        }
        assert_eq!(state.mode, Mode::Clear);

        assert!(state.observe_status(storm_status(), start + MODE_MIN_DWELL));
        assert_eq!(state.mode, Mode::Storm);
    }

    #[test]
    fn manual_settings_reset_the_hysteresis_state() {
        let mut state = State::new();
        let start = at("2026-10-21T15:30:00Z");
        assert!(!state.observe_status(clear_status(), start));
        assert!(!state.observe_status(storm_status(), start + Duration::minutes(1)));

        assert!(state.apply_settings(ModeSelect::Manual, Some(Mode::Storm), start));
        assert_eq!(state.mode, Mode::Storm);
        assert_eq!(state.mode_since, start);
        assert_eq!(state.pending_mode, None);
        assert_eq!(state.manual_til, Some(start + Duration::days(1)));
    }

    #[test]
    fn manual_selection_expires_to_auto() {
        let mut state = State::new();
        let start = at("2026-10-21T15:30:00Z");
        state.apply_settings(ModeSelect::Manual, Some(Mode::Storm), start);

        assert!(!state.expire_manual(start + Duration::hours(23)));
        assert_eq!(state.mode_select, ModeSelect::Manual);

        assert!(state.expire_manual(start + Duration::days(1) + Duration::seconds(1)));
        assert_eq!(state.mode_select, ModeSelect::Auto);
        assert!(state.manual_til.is_some());
    }

    #[test]
    fn state_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        let mut state = State::new();
        state.mode = Mode::Storm;
        state.mode_select = ModeSelect::Manual;
        state.manual_til = Some(at("2026-10-22T15:30:00Z"));
        state.last_updated = at("2026-10-21T15:30:00Z");
        state.last_attempt = at("2026-10-21T15:31:00Z");
        state.next_attempt = at("2026-10-21T15:41:00Z");
        state.failures = 2;
        state.radar_last_updated = at("2026-10-21T15:20:00Z");
        state.radar_key = Some("2026/10/21/KTLX/KTLX20261021_152000_V06".into());
        state.save(&path).unwrap();

        let loaded = State::load(&path);
        assert_eq!(loaded.mode, state.mode);
        assert_eq!(loaded.mode_select, state.mode_select);
        assert_eq!(loaded.manual_til, state.manual_til);
        assert_eq!(loaded.last_updated, state.last_updated);
        assert_eq!(loaded.last_attempt, state.last_attempt);
        assert_eq!(loaded.next_attempt, state.next_attempt);
        assert_eq!(loaded.failures, state.failures);
        assert_eq!(loaded.radar_last_updated, state.radar_last_updated);
        assert_eq!(loaded.radar_key, state.radar_key);
        assert_eq!(loaded.mode_since, state.last_updated);
    }

    #[test]
    fn missing_or_corrupt_state_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        let missing = State::load(&path);
        assert_eq!(missing.mode_select, ModeSelect::Auto);
        assert_eq!(missing.last_updated, DateTime::UNIX_EPOCH);

        std::fs::write(&path, b"not json").unwrap();
        let corrupt = State::load(&path);
        assert_eq!(corrupt.mode_select, ModeSelect::Auto);
        assert_eq!(corrupt.last_updated, DateTime::UNIX_EPOCH);
    }

    #[test]
    fn an_unknown_state_version_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            br#"{"version":99,"mode":"Storm","mode_select":"Manual",
                "manual_til":null,"last_updated":"2026-10-21T15:30:00Z",
                "last_attempt":"2026-10-21T15:30:00Z",
                "next_attempt":"2026-10-21T15:40:00Z","failures":0,
                "radar_last_updated":"2026-10-21T15:20:00Z","radar_key":null}"#,
        )
        .unwrap();

        let state = State::load(&path);
        assert_eq!(state.mode_select, ModeSelect::Auto);
        assert_eq!(state.last_updated, DateTime::UNIX_EPOCH);
    }
}
