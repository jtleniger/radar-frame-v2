//! The two render modes, ported from `modes/storm.py` and `modes/clear.py`.

use std::fs;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::engine::Renderer;
use crate::error::{Error, Result};
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
    pub now: DateTime<Utc>,
}

pub async fn run(mode: Mode, deps: &Deps<'_>, state: &mut State) -> Result<()> {
    match mode {
        Mode::Storm => storm(deps, state).await,
        Mode::Clear => clear(deps).await,
    }
}

async fn storm(deps: &Deps<'_>, state: &mut State) -> Result<()> {
    let nexrad_id = deps.config.get("radar", "nexrad_id")?;

    if let Some(object) = deps.radar.latest(nexrad_id, deps.now).await? {
        if object.last_modified > state.radar_last_updated {
            tracing::info!("downloading {}", object.key);
            let bytes = deps.radar.download(&object).await?;
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
    render(Arc::clone(deps.engine), move |engine| {
        engine.render_radar(&volume, MINIMUM_DBZ)
    })
    .await?;

    let alerts = deps.provider.alerts().await?;
    let now = deps.now;
    render(Arc::clone(deps.engine), move |engine| {
        engine.render_storm(&alerts, now)
    })
    .await
}

async fn clear(deps: &Deps<'_>) -> Result<()> {
    let forecast = (*deps.provider.forecast().await?).clone();
    let alerts = deps.provider.alerts().await?;
    let now = deps.now;

    render(Arc::clone(deps.engine), move |engine| {
        engine.render_clear(&forecast, alerts, now)
    })
    .await
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
