//! The single background task that owns [`State`].

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot, watch};

use crate::config::{Paths, Settings};
use crate::error::{Error, Result};
use crate::service::clock::Clock;
use crate::service::frame::PublishedFrame;
use crate::service::modes::{self, Services};
use crate::service::renderer::Renderer;
use crate::sources::{RadarSource, Sources};
use crate::state::{Mode, ModeSelect, State, STATUS_TTL};
use crate::telemetry::metrics::{elapsed_ms, Metrics, Outcome};

use super::{CYCLE_DEADLINE, TICK};

/// Immutable schedule and frame published by the updater. Handlers clone the
/// handle, never the mutable state.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: State,
}

pub enum Command {
    Update {
        reply: oneshot::Sender<Update>,
    },
    Settings {
        mode_select: ModeSelect,
        mode: Option<Mode>,
        reply: oneshot::Sender<()>,
    },
}

/// What one [`Updater::update`] pass did.
pub enum Update {
    /// Nothing was due; the last published frame still stands.
    Skipped,
    /// A cycle rendered and validated a new frame.
    Published(Arc<PublishedFrame>),
    /// A cycle was due and failed; the last good frame is still published.
    Failed(Error),
}

/// Owns [`State`] and is the only task that mutates it.
pub struct Updater {
    pub(super) settings: Settings,
    pub(super) paths: Paths,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) sources: Sources,
    pub(super) radar: Arc<dyn RadarSource>,
    pub(super) renderer: Arc<dyn Renderer>,
    pub(super) metrics: Arc<Metrics>,
    pub(super) state: State,
    pub(super) snapshots: watch::Sender<Arc<Snapshot>>,
}

impl Updater {
    pub(super) async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        // The first tick is one period out: the initial update is driven by
        // the first poll (or the next tick), never by task startup.
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + TICK, TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let now = self.clock.now();
                    let _ = self.update(now).await;
                }
                command = commands.recv() => {
                    let Some(command) = command else { break };
                    match command {
                        Command::Update { reply } => {
                            let now = self.clock.now();
                            let update = self.update(now).await;
                            let _ = reply.send(update);
                        }
                        Command::Settings { mode_select, mode, reply } => {
                            self.set_mode(mode_select, mode);
                            let _ = reply.send(());
                        }
                    }
                }
            }
        }
    }

    /// One pass: expire a manual selection, refresh the status sample when its
    /// TTL has passed, and run a cycle when the schedule allows it.
    async fn update(&mut self, now: DateTime<Utc>) -> Update {
        let mut changed = self.state.expire_manual(now);
        if changed {
            tracing::info!("manual mode expired");
        }

        if self.state.mode_select == ModeSelect::Auto {
            match update_mode(&mut self.state, &self.sources, &self.metrics, now).await {
                Ok(true) => {
                    self.state.reschedule_now();
                    changed = true;
                }
                Ok(false) => {}
                Err(error) => {
                    // A status outage must not block rendering or change the
                    // last known mode; the cycle below still runs on schedule.
                    tracing::error!("radar status: {error}");
                    changed = true;
                }
            }
        }

        if changed {
            self.persist();
            self.publish();
        }

        if !self.state.cycle_due(now) {
            return Update::Skipped;
        }

        let update = self.run_cycle(now).await;
        self.persist();
        self.publish();
        update
    }

    fn set_mode(&mut self, mode_select: ModeSelect, mode: Option<Mode>) {
        tracing::info!("Set mode select to {}", mode_select.name());
        let now = self.clock.now();
        self.state.apply_settings(mode_select, mode, now);
        self.persist();
        self.publish();
    }

    async fn run_cycle(&mut self, now: DateTime<Utc>) -> Update {
        let mode = self.state.mode;
        let services = Services {
            paths: &self.paths,
            settings: &self.settings,
            renderer: &self.renderer,
            forecast: self.sources.forecast.as_ref(),
            alerts: self.sources.alerts.as_ref(),
            radar: self.radar.as_ref(),
            metrics: self.metrics.as_ref(),
            now,
        };

        let cycle_started = Instant::now();
        let cycle = match tokio::time::timeout(
            CYCLE_DEADLINE,
            modes::run(mode, &services, &mut self.state),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(Error::new(format!(
                "cycle exceeded the {} s deadline",
                CYCLE_DEADLINE.as_secs()
            ))),
        };
        let cycle = match cycle {
            Ok(cycle) => cycle,
            Err(error) => return self.fail(now, mode, cycle_started, error),
        };

        let bytes = match std::fs::read(self.paths.output_img()) {
            Ok(bytes) => bytes,
            Err(error) => {
                return self.fail(
                    now,
                    mode,
                    cycle_started,
                    Error::new(format!("{}: {error}", self.paths.output_img().display())),
                );
            }
        };
        let frame = match PublishedFrame::new(bytes, now, Some(mode), cycle.degraded.clone()) {
            Ok(frame) => frame,
            Err(error) => return self.fail(now, mode, cycle_started, error),
        };

        let frame = Arc::new(frame);
        self.state.cycle_succeeded(now, mode);
        self.state.frame = Some(Arc::clone(&frame));

        let outcome = if cycle.degraded.is_some() {
            Outcome::Degraded
        } else {
            Outcome::Ok
        };
        let elapsed = cycle_started.elapsed();
        self.metrics.cycle(self.clock.now(), elapsed, outcome);
        tracing::info!(
            event = "cycle",
            mode = mode.name(),
            outcome = outcome.as_str(),
            elapsed_ms = elapsed_ms(elapsed),
            volume_key = cycle.volume_key.as_deref().unwrap_or("-"),
            elevations = ?cycle.elevations,
            radials = cycle.radials,
            gates = cycle.gates,
            alerts = cycle.alerts,
            render_ms = cycle.render_ms,
            frame_bytes = frame.bytes.len(),
            frame_sha = frame.etag.trim_matches('"'),
            degraded = cycle.degraded.as_deref().unwrap_or("-"),
            "cycle complete"
        );
        Update::Published(frame)
    }

    fn fail(&mut self, now: DateTime<Utc>, mode: Mode, started: Instant, error: Error) -> Update {
        let elapsed = started.elapsed();
        self.metrics
            .cycle(self.clock.now(), elapsed, Outcome::Failed);
        tracing::warn!(
            event = "cycle",
            mode = mode.name(),
            outcome = "failed",
            elapsed_ms = elapsed_ms(elapsed),
            error = %error,
            "cycle failed"
        );
        tracing::error!("{} mode failed: {error}", mode.name());
        self.state.cycle_failed(now, mode);
        Update::Failed(error)
    }

    /// Publish the state for handlers. A failure means no handler is
    /// listening, which is fine: the updater is shutting down.
    fn publish(&self) {
        self.snapshots.send_replace(Arc::new(Snapshot {
            state: self.state.clone(),
        }));
    }

    fn persist(&self) {
        if let Err(error) = self.state.save(&self.paths.state_file()) {
            tracing::warn!(event = "state", error = %error, "could not persist state");
        }
    }
}

/// Refresh the radar-status sample when its TTL has passed. True when the mode
/// changed.
///
/// The timestamps are recorded even when the request fails, so an outage is
/// retried once per TTL instead of on every poll. A failure keeps the
/// last-known-good status and the current mode.
async fn update_mode(
    state: &mut State,
    sources: &Sources,
    metrics: &Metrics,
    now: DateTime<Utc>,
) -> Result<bool> {
    if now < state.status_next_attempt {
        return Ok(false);
    }

    let started = Instant::now();
    let status = sources.status.radar_status().await;
    metrics.upstream(now, started.elapsed(), status.is_ok());

    state.status_checked_at = now;
    state.status_next_attempt = now + STATUS_TTL;

    let status = status?;
    tracing::info!("radar status: up={} vcp={}", status.up, status.vcp);
    if !status.up {
        tracing::error!("radar down");
    } else if status.vcp == -1 {
        tracing::error!("some issue with nws api");
    }

    // A change is accepted only after consecutive agreeing samples and a
    // minimum dwell; the first sample after startup bootstraps the mode.
    let changed = state.observe_status(status, now);
    if changed {
        tracing::info!(event = "mode", mode = state.mode.name(), "mode switched");
    }
    Ok(changed)
}
