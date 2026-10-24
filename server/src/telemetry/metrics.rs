//! Process counters and timestamps for `/status`.
//!
//! Every field is a single atomic, so recording a request or a cycle never
//! takes the state mutex. Ages are computed from the stored timestamps at
//! snapshot time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Ok,
    Degraded,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Degraded => "degraded",
            Outcome::Failed => "failed",
        }
    }

    fn code(self) -> u64 {
        match self {
            Outcome::Ok => 0,
            Outcome::Degraded => 1,
            Outcome::Failed => 2,
        }
    }

    fn from_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(Outcome::Ok),
            1 => Some(Outcome::Degraded),
            2 => Some(Outcome::Failed),
            _ => None,
        }
    }
}

/// One recorded event, as exposed in `/status`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sample {
    pub at: DateTime<Utc>,
    pub elapsed_ms: u64,
    pub outcome: Outcome,
}

#[derive(Debug)]
pub struct Metrics {
    started_at: AtomicU64,
    requests_total: AtomicU64,
    cycles_total: AtomicU64,
    cycles_failed: AtomicU64,
    cycles_degraded: AtomicU64,
    upstream_total: AtomicU64,
    upstream_failed: AtomicU64,
    last_cycle_at: AtomicU64,
    last_cycle_ms: AtomicU64,
    last_cycle_outcome: AtomicU64,
    last_upstream_at: AtomicU64,
    last_upstream_ms: AtomicU64,
    last_upstream_outcome: AtomicU64,
}

impl Metrics {
    pub fn new(started_at: DateTime<Utc>) -> Self {
        let metrics = Metrics {
            started_at: AtomicU64::new(0),
            requests_total: AtomicU64::new(0),
            cycles_total: AtomicU64::new(0),
            cycles_failed: AtomicU64::new(0),
            cycles_degraded: AtomicU64::new(0),
            upstream_total: AtomicU64::new(0),
            upstream_failed: AtomicU64::new(0),
            last_cycle_at: AtomicU64::new(0),
            last_cycle_ms: AtomicU64::new(0),
            last_cycle_outcome: AtomicU64::new(u64::MAX),
            last_upstream_at: AtomicU64::new(0),
            last_upstream_ms: AtomicU64::new(0),
            last_upstream_outcome: AtomicU64::new(u64::MAX),
        };
        metrics
            .started_at
            .store(millis(started_at), Ordering::Relaxed);
        metrics
    }

    pub fn request(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn cycle(&self, at: DateTime<Utc>, elapsed: Duration, outcome: Outcome) {
        self.cycles_total.fetch_add(1, Ordering::Relaxed);
        match outcome {
            Outcome::Failed => self.cycles_failed.fetch_add(1, Ordering::Relaxed),
            Outcome::Degraded => self.cycles_degraded.fetch_add(1, Ordering::Relaxed),
            Outcome::Ok => 0,
        };
        self.last_cycle_at.store(millis(at), Ordering::Relaxed);
        self.last_cycle_ms
            .store(elapsed_ms(elapsed), Ordering::Relaxed);
        self.last_cycle_outcome
            .store(outcome.code(), Ordering::Relaxed);
    }

    pub fn upstream(&self, at: DateTime<Utc>, elapsed: Duration, ok: bool) {
        self.upstream_total.fetch_add(1, Ordering::Relaxed);
        if !ok {
            self.upstream_failed.fetch_add(1, Ordering::Relaxed);
        }
        self.last_upstream_at.store(millis(at), Ordering::Relaxed);
        self.last_upstream_ms
            .store(elapsed_ms(elapsed), Ordering::Relaxed);
        self.last_upstream_outcome.store(
            if ok {
                Outcome::Ok.code()
            } else {
                Outcome::Failed.code()
            },
            Ordering::Relaxed,
        );
    }

    pub fn started_at(&self) -> Option<DateTime<Utc>> {
        time(self.started_at.load(Ordering::Relaxed))
    }

    pub fn requests_total(&self) -> u64 {
        self.requests_total.load(Ordering::Relaxed)
    }

    pub fn cycles_total(&self) -> u64 {
        self.cycles_total.load(Ordering::Relaxed)
    }

    pub fn cycles_failed(&self) -> u64 {
        self.cycles_failed.load(Ordering::Relaxed)
    }

    pub fn cycles_degraded(&self) -> u64 {
        self.cycles_degraded.load(Ordering::Relaxed)
    }

    pub fn upstream_total(&self) -> u64 {
        self.upstream_total.load(Ordering::Relaxed)
    }

    pub fn upstream_failed(&self) -> u64 {
        self.upstream_failed.load(Ordering::Relaxed)
    }

    pub fn last_cycle(&self) -> Option<Sample> {
        let at = time(self.last_cycle_at.load(Ordering::Relaxed))?;
        let outcome = Outcome::from_code(self.last_cycle_outcome.load(Ordering::Relaxed))?;
        Some(Sample {
            at,
            elapsed_ms: self.last_cycle_ms.load(Ordering::Relaxed),
            outcome,
        })
    }

    pub fn last_upstream(&self) -> Option<Sample> {
        let at = time(self.last_upstream_at.load(Ordering::Relaxed))?;
        let outcome = Outcome::from_code(self.last_upstream_outcome.load(Ordering::Relaxed))?;
        Some(Sample {
            at,
            elapsed_ms: self.last_upstream_ms.load(Ordering::Relaxed),
            outcome,
        })
    }
}

fn millis(time: DateTime<Utc>) -> u64 {
    u64::try_from(time.timestamp_millis()).unwrap_or(0)
}

fn time(millis: u64) -> Option<DateTime<Utc>> {
    if millis == 0 {
        return None;
    }
    DateTime::from_timestamp_millis(i64::try_from(millis).ok()?)
}

pub fn elapsed_ms(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
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
    fn records_cycles_and_upstream_calls() {
        let metrics = Metrics::new(at("2026-10-21T15:00:00Z"));
        metrics.request();
        metrics.cycle(
            at("2026-10-21T15:30:00Z"),
            Duration::from_millis(125),
            Outcome::Degraded,
        );
        metrics.upstream(at("2026-10-21T15:29:58Z"), Duration::from_millis(80), false);

        assert_eq!(metrics.started_at(), Some(at("2026-10-21T15:00:00Z")));
        assert_eq!(metrics.requests_total(), 1);
        assert_eq!(metrics.cycles_total(), 1);
        assert_eq!(metrics.cycles_degraded(), 1);
        assert_eq!(metrics.cycles_failed(), 0);
        assert_eq!(
            metrics.last_cycle(),
            Some(Sample {
                at: at("2026-10-21T15:30:00Z"),
                elapsed_ms: 125,
                outcome: Outcome::Degraded,
            })
        );
        assert_eq!(metrics.upstream_total(), 1);
        assert_eq!(metrics.upstream_failed(), 1);
        assert_eq!(
            metrics.last_upstream(),
            Some(Sample {
                at: at("2026-10-21T15:29:58Z"),
                elapsed_ms: 80,
                outcome: Outcome::Failed,
            })
        );
    }

    #[test]
    fn empty_samples_are_absent() {
        let metrics = Metrics::new(at("2026-10-21T15:00:00Z"));
        assert_eq!(metrics.last_cycle(), None);
        assert_eq!(metrics.last_upstream(), None);
    }
}
