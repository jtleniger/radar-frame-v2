//! The two render modes, ported from `modes/storm.py` and `modes/clear.py`.

use std::fs;
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

    if let Some(object) = latest? {
        cycle.volume_key = Some(object.key.clone());
        if object.last_modified > state.radar_last_updated {
            tracing::info!("downloading {}", object.key);
            let started = Instant::now();
            let bytes = deps.radar.download(&object).await;
            deps.metrics
                .upstream(deps.now, started.elapsed(), bytes.is_ok());
            let bytes = bytes?;
            let raw = deps.paths.radar_raw();
            fs::write(&raw, bytes).map_err(|e| Error(format!("{}: {e}", raw.display())))?;
            state.radar_last_updated = object.last_modified;
        } else {
            tracing::info!("using old radar data");
        }
    }

    // The Python mode rendered the radar frame on every update, including when
    // the volume was unchanged; keep that so a missing or deleted frame is
    // rebuilt.
    let volume = deps.paths.radar_raw();
    let started = Instant::now();
    let summary = render(Arc::clone(deps.engine), move |engine| {
        engine.render_radar(&volume, MINIMUM_DBZ)
    })
    .await?;
    cycle.render_ms += elapsed_ms(started.elapsed());
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
