//! Retry pacing for failed update cycles.
//!
//! A failed cycle leaves the last good frame in place, but the next poll must
//! not immediately repeat every upstream request. The delay starts at one
//! minute, doubles per consecutive failure, and is capped at the mode interval
//! so a healthy schedule is never delayed beyond its own cadence. Up to a
//! quarter of the delay is added as jitter so several instances do not retry
//! in lockstep.

use chrono::Duration;

/// Delay before the second attempt after a failure.
const BASE_SECONDS: i64 = 60;

/// Doublings are bounded before the mode-interval cap takes over.
const MAX_DOUBLINGS: u32 = 8;

/// Exponential delay for `failures` consecutive failures, capped at
/// `interval`. `failures` counts the attempt that just failed, so the first
/// failure schedules the one-minute delay. `seed` varies the jitter; callers
/// pass the current time.
pub fn delay(failures: u32, interval: Duration, seed: u64) -> Duration {
    let interval_seconds = interval.num_seconds().max(1);
    let exponent = failures.saturating_sub(1).min(MAX_DOUBLINGS);
    let base = BASE_SECONDS
        .saturating_mul(1i64 << exponent)
        .min(interval_seconds);

    // At most 25% jitter, never beyond the cap.
    let spread = (base / 4).max(1) as u64;
    let jitter = (seed % spread) as i64;
    Duration::seconds((base + jitter).min(interval_seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minutes(value: i64) -> Duration {
        Duration::minutes(value)
    }

    #[test]
    fn doubles_from_one_minute_and_caps_at_the_interval() {
        let interval = minutes(10);
        assert_eq!(delay(1, interval, 0), minutes(1));
        assert_eq!(delay(2, interval, 0), minutes(2));
        assert_eq!(delay(3, interval, 0), minutes(4));
        assert_eq!(delay(4, interval, 0), minutes(8));
        assert_eq!(delay(5, interval, 0), minutes(10));
        assert_eq!(delay(100, interval, 0), minutes(10));
    }

    #[test]
    fn jitter_stays_below_the_cap() {
        let interval = minutes(10);
        for seed in 0..1000 {
            let wait = delay(1, interval, seed);
            assert!(wait >= minutes(1), "{wait}");
            assert!(wait <= minutes(1) + Duration::seconds(15), "{wait}");
        }
    }

    #[test]
    fn short_intervals_are_respected() {
        let interval = Duration::seconds(30);
        assert_eq!(delay(3, interval, 0), interval);
        assert_eq!(delay(3, interval, 99), interval);
    }
}
