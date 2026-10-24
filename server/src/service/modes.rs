//! The two render modes, ported from `modes/storm.py` and `modes/clear.py`.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::config::paths::Paths;
use crate::config::settings::Settings;
use crate::error::{Error, Result};
use crate::service::engine::Renderer;
use crate::sources::{AlertSource, Forecast, ForecastSource, RadarSource};
use crate::state::{Mode, State};
use crate::telemetry::metrics::{elapsed_ms, Metrics};
use crate::view::forecast::Alert;

/// The default gate cutoff used by the frame path (`render()` in
/// `components/radar/render.py`).
const MINIMUM_DBZ: f64 = 10.0;

pub struct Deps<'a> {
    pub paths: &'a Paths,
    pub settings: &'a Settings,
    pub engine: &'a Arc<dyn Renderer>,
    pub forecast: &'a dyn ForecastSource,
    pub alerts: &'a dyn AlertSource,
    pub radar: &'a dyn RadarSource,
    pub metrics: &'a Metrics,
    pub now: DateTime<Utc>,
}

/// What a cycle managed to produce. `degraded` names any layer that failed
/// but did not prevent a frame from being rendered; the remaining fields are
/// provenance for the per-cycle log.
#[derive(Clone, Debug, Default)]
pub struct Cycle {
    pub degraded: Option<String>,
    pub volume_key: Option<String>,
    pub elevations: Vec<u8>,
    pub radials: usize,
    pub gates: usize,
    pub alerts: usize,
    pub render_ms: u64,
}

pub async fn run(mode: Mode, deps: &Deps<'_>, state: &mut State) -> Result<Cycle> {
    match mode {
        Mode::Storm => storm(deps, state).await,
        Mode::Clear => clear(deps, state).await,
    }
}

async fn storm(deps: &Deps<'_>, state: &mut State) -> Result<Cycle> {
    let nexrad_id = deps.settings.nexrad_id.as_str();
    let mut cycle = Cycle::default();

    let started = Instant::now();
    let latest = deps.radar.latest(nexrad_id, deps.now).await;
    deps.metrics
        .upstream(deps.now, started.elapsed(), latest.is_ok());

    // A failed listing falls back to the volume already on disk. The cycle
    // only fails when there is no last good volume to render.
    let latest = match latest {
        Ok(latest) => latest,
        Err(error) => {
            if !deps.paths.radar_raw().exists() {
                return Err(error);
            }
            tracing::warn!("radar listing unavailable, using the last good volume: {error}");
            degrade(&mut cycle, format!("radar: {error}"));
            None
        }
    };

    let mut radar_summary = None;
    if let Some(object) = latest {
        // A restarted process restores `radar_last_updated`, so the volume
        // already on disk is not downloaded again. The file itself must still
        // exist; a cleared data directory forces a fresh download.
        if object.last_modified > state.radar_last_updated || !deps.paths.radar_raw().exists() {
            tracing::info!("downloading {}", object.key);
            let part = deps.paths.radar_part();
            let started = Instant::now();
            let downloaded = deps.radar.download(&object, &part).await;
            deps.metrics
                .upstream(deps.now, started.elapsed(), downloaded.is_ok());

            match downloaded {
                Ok(_) => {
                    // Parse and render the staged volume before it can replace
                    // the last good pair. A bad volume leaves
                    // `nexrad-latest.ar2` and `nexrad-latest.png` untouched.
                    let png_part = deps.paths.radar_img_part();
                    let volume = part.clone();
                    let output = png_part.clone();
                    let started = Instant::now();
                    let rendered = render(Arc::clone(deps.engine), move |engine| {
                        engine.render_radar(&volume, MINIMUM_DBZ, &output)
                    })
                    .await;
                    cycle.render_ms += elapsed_ms(started.elapsed());

                    match rendered {
                        Ok(summary) => {
                            let raw = deps.paths.radar_raw();
                            let radar_img = deps.paths.radar_img();
                            match commit(&part, &raw).and_then(|()| commit(&png_part, &radar_img)) {
                                Ok(()) => {
                                    cycle.volume_key = Some(object.key.clone());
                                    state.radar_last_updated = object.last_modified;
                                    state.radar_key = Some(object.key.clone());
                                    radar_summary = Some(summary);
                                }
                                Err(error) => {
                                    remove(&part);
                                    remove(&png_part);
                                    degrade(&mut cycle, format!("radar: {error}"));
                                }
                            }
                        }
                        Err(error) => {
                            remove(&part);
                            remove(&png_part);
                            degrade(&mut cycle, format!("radar: {error}"));
                        }
                    }
                }
                Err(error) => {
                    remove(&part);
                    degrade(&mut cycle, format!("radar: {error}"));
                }
            }
        } else {
            tracing::info!("using old radar data");
            cycle.volume_key = Some(
                state
                    .radar_key
                    .clone()
                    .unwrap_or_else(|| object.key.clone()),
            );
        }
    }

    // The Python mode rendered the radar frame on every update, including when
    // the volume was unchanged; keep that so a missing or deleted frame is
    // rebuilt. `radar_img` was already written atomically for a new volume.
    let summary = match radar_summary {
        Some(summary) => summary,
        None => {
            let volume = deps.paths.radar_raw();
            let output = deps.paths.radar_img();
            let started = Instant::now();
            let summary = render(Arc::clone(deps.engine), move |engine| {
                engine.render_radar(&volume, MINIMUM_DBZ, &output)
            })
            .await?;
            cycle.render_ms += elapsed_ms(started.elapsed());
            summary
        }
    };
    cycle.elevations = summary.elevations;
    cycle.radials = summary.radials;
    cycle.gates = summary.gates;

    let alerts = alerts(deps, state, &mut cycle).await;
    let now = deps.now;
    let started = Instant::now();
    render(Arc::clone(deps.engine), move |engine| {
        engine.render_storm(&alerts, now)
    })
    .await?;
    cycle.render_ms += elapsed_ms(started.elapsed());
    Ok(cycle)
}

async fn clear(deps: &Deps<'_>, state: &mut State) -> Result<Cycle> {
    let mut cycle = Cycle::default();

    let forecast = forecast(deps, state, &mut cycle).await?;
    let alerts = alerts(deps, state, &mut cycle).await;
    let now = deps.now;

    let started = Instant::now();
    render(Arc::clone(deps.engine), move |engine| {
        engine.render_clear(&forecast, &alerts, now)
    })
    .await?;
    cycle.render_ms += elapsed_ms(started.elapsed());
    Ok(cycle)
}

/// Fetch the forecast, falling back to the last good one when the upstream
/// call fails. The cycle fails only before a forecast has ever been cached.
async fn forecast(deps: &Deps<'_>, state: &mut State, cycle: &mut Cycle) -> Result<Arc<Forecast>> {
    let started = Instant::now();
    let forecast = deps.forecast.forecast().await;
    deps.metrics
        .upstream(deps.now, started.elapsed(), forecast.is_ok());

    match forecast {
        Ok(forecast) => {
            let forecast = Arc::new(forecast);
            state.forecast = Some(Arc::clone(&forecast));
            Ok(forecast)
        }
        Err(error) => match state.forecast.clone() {
            Some(cached) => {
                let reason = format!("forecast: {error} (using last good)");
                tracing::warn!("{reason}");
                degrade(cycle, reason);
                Ok(cached)
            }
            None => Err(error),
        },
    }
}

/// Fetch alerts, falling back to the last good list and then to an empty list.
/// A missing alert layer never fails the frame.
async fn alerts(deps: &Deps<'_>, state: &mut State, cycle: &mut Cycle) -> Arc<[Alert]> {
    let started = Instant::now();
    let alerts = deps.alerts.alerts().await;
    deps.metrics
        .upstream(deps.now, started.elapsed(), alerts.is_ok());

    let alerts = match alerts {
        Ok(alerts) => {
            let alerts: Arc<[Alert]> = alerts.into();
            state.alerts = Some(Arc::clone(&alerts));
            alerts
        }
        Err(error) => match state.alerts.clone() {
            Some(cached) => {
                let reason = format!("alerts: {error} (using last good)");
                tracing::warn!("{reason}");
                degrade(cycle, reason);
                cached
            }
            None => {
                let reason = format!("alerts: {error}");
                tracing::warn!("alerts unavailable, rendering without them: {error}");
                degrade(cycle, reason);
                Arc::from(Vec::new())
            }
        },
    };
    cycle.alerts = alerts.len();
    alerts
}

/// Append a failed layer to the cycle's degraded reason.
fn degrade(cycle: &mut Cycle, reason: String) {
    cycle.degraded = Some(match cycle.degraded.take() {
        Some(existing) => format!("{existing}; {reason}"),
        None => reason,
    });
}

async fn render<T, F>(engine: Arc<dyn Renderer>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&dyn Renderer) -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || f(engine.as_ref()))
        .await
        .map_err(|e| Error::render(format!("render task: {e}")))?
}

/// Rename a validated staging artifact over its published name.
fn commit(temp: &Path, destination: &Path) -> Result<()> {
    fs::rename(temp, destination).map_err(|error| {
        Error::io(format!(
            "{} -> {}: {error}",
            temp.display(),
            destination.display()
        ))
    })
}

fn remove(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!("{}: {error}", path.display()),
    }
}
