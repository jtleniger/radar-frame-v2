//! The two render modes, ported from `modes/storm.py` and `modes/clear.py`.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::engine::Renderer;
use crate::error::{Error, Result};
use crate::metrics::{elapsed_ms, Metrics};
use crate::paths::Paths;
use crate::sources::{ForecastProvider, RadarSource};
use crate::state::{Mode, State};

/// The default gate cutoff used by the frame path (`render()` in
/// `components/radar/render.py`).
const MINIMUM_DBZ: f64 = 10.0;

pub struct Deps<'a> {
    pub paths: &'a Paths,
    pub config: &'a Config,
    pub engine: &'a Arc<dyn Renderer>,
    pub provider: &'a dyn ForecastProvider,
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
        Mode::Clear => clear(deps).await,
    }
}

async fn storm(deps: &Deps<'_>, state: &mut State) -> Result<Cycle> {
    let nexrad_id = deps.config.get("radar", "nexrad_id")?;
    let mut cycle = Cycle::default();

    let started = Instant::now();
    let latest = deps.radar.latest(nexrad_id, deps.now).await;
    deps.metrics
        .upstream(deps.now, started.elapsed(), latest.is_ok());

    let mut radar_summary = None;
    if let Some(object) = latest? {
        cycle.volume_key = Some(object.key.clone());
        if object.last_modified > state.radar_last_updated {
            tracing::info!("downloading {}", object.key);
            let part = deps.paths.radar_part();
            let started = Instant::now();
            let downloaded = deps.radar.download(&object, &part).await;
            deps.metrics
                .upstream(deps.now, started.elapsed(), downloaded.is_ok());
            if let Err(error) = downloaded {
                remove(&part);
                return Err(error);
            }

            // Parse and render the staged volume before it can replace the
            // last good pair. A bad volume leaves `nexrad-latest.ar2` and
            // `nexrad-latest.png` untouched.
            let png_part = deps.paths.radar_img_part();
            let volume = part.clone();
            let output = png_part.clone();
            let started = Instant::now();
            let summary = match render(Arc::clone(deps.engine), move |engine| {
                engine.render_radar(&volume, MINIMUM_DBZ, &output)
            })
            .await
            {
                Ok(summary) => summary,
                Err(error) => {
                    remove(&part);
                    remove(&png_part);
                    return Err(error);
                }
            };
            cycle.render_ms += elapsed_ms(started.elapsed());

            let raw = deps.paths.radar_raw();
            let radar_img = deps.paths.radar_img();
            if let Err(error) = commit(&part, &raw).and_then(|()| commit(&png_part, &radar_img)) {
                remove(&part);
                remove(&png_part);
                return Err(error);
            }
            state.radar_last_updated = object.last_modified;
            radar_summary = Some(summary);
        } else {
            tracing::info!("using old radar data");
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

    let started = Instant::now();
    let alerts = deps.provider.alerts().await;
    deps.metrics
        .upstream(deps.now, started.elapsed(), alerts.is_ok());
    let (alerts, degraded) = match alerts {
        Ok(alerts) => (alerts, None),
        Err(error) => {
            tracing::warn!("alerts unavailable, rendering without them: {error}");
            (Vec::new(), Some(format!("alerts: {error}")))
        }
    };
    cycle.alerts = alerts.len();
    let now = deps.now;
    let started = Instant::now();
    render(Arc::clone(deps.engine), move |engine| {
        engine.render_storm(&alerts, now)
    })
    .await?;
    cycle.render_ms += elapsed_ms(started.elapsed());
    cycle.degraded = degraded;
    Ok(cycle)
}

async fn clear(deps: &Deps<'_>) -> Result<Cycle> {
    let mut cycle = Cycle::default();

    let started = Instant::now();
    let forecast = deps.provider.forecast().await;
    deps.metrics
        .upstream(deps.now, started.elapsed(), forecast.is_ok());
    let forecast = forecast?;

    let started = Instant::now();
    let alerts = deps.provider.alerts().await;
    deps.metrics
        .upstream(deps.now, started.elapsed(), alerts.is_ok());
    let (alerts, degraded) = match alerts {
        Ok(alerts) => (alerts, None),
        Err(error) => {
            tracing::warn!("alerts unavailable, rendering without them: {error}");
            (Vec::new(), Some(format!("alerts: {error}")))
        }
    };
    cycle.alerts = alerts.len();
    let now = deps.now;

    let started = Instant::now();
    render(Arc::clone(deps.engine), move |engine| {
        engine.render_clear(&forecast, &alerts, now)
    })
    .await?;
    cycle.render_ms += elapsed_ms(started.elapsed());
    cycle.degraded = degraded;
    Ok(cycle)
}

async fn render<T, F>(engine: Arc<dyn Renderer>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&dyn Renderer) -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || f(engine.as_ref()))
        .await
        .map_err(|e| Error(format!("render task: {e}")))?
}

/// Rename a validated staging artifact over its published name.
fn commit(temp: &Path, destination: &Path) -> Result<()> {
    fs::rename(temp, destination).map_err(|error| {
        Error(format!(
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
