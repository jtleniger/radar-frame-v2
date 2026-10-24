//! Forecast, alert, and radar-status sources and the two providers the server
//! selects between.
//!
//! `OnlineProvider` reads Open-Meteo and the NWS API; `MockProvider` mirrors
//! `sources/mock_forecast.py` for offline development and fixtures. Each
//! consumer takes only the source trait it needs, and [`Sources`] bundles the
//! three handles built from one provider. The data types are `frame-view`'s,
//! so the composed image consumes them directly.

pub mod http;
pub mod mock;
pub mod nexrad;
pub mod nws;
pub mod online;
pub mod open_meteo;
pub mod osm;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::Result;
use crate::view::forecast::{Alert, CurrentConditions, ForecastDay, ForecastHour};

/// One forecast cycle: what the clear view draws and nothing else.
#[derive(Clone, Debug, Deserialize)]
pub struct Forecast {
    pub current: CurrentConditions,
    pub hourly: Vec<ForecastHour>,
    pub daily: Vec<ForecastDay>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadarStatus {
    pub up: bool,
    pub vcp: i32,
}

impl RadarStatus {
    /// The volume coverage patterns the NWS uses when no precipitation mode is
    /// running (see `sources/forecast.py`).
    pub fn clear_air_mode(&self) -> bool {
        matches!(self.vcp, 31 | 32 | 35)
    }
}

/// One archive entry, with the modification time the schedule compares.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RadarObject {
    pub key: String,
    pub last_modified: DateTime<Utc>,
}

/// The forecast panel source used by Clear cycles.
#[async_trait]
pub trait ForecastSource: Send + Sync {
    /// Current conditions plus the hourly and daily panels. Called once per
    /// update cycle, so each Clear frame gets a fresh forecast; the online
    /// provider fetches all of it in one request.
    async fn forecast(&self) -> Result<Forecast>;
}

/// The alert overlay source, consumed by both modes.
#[async_trait]
pub trait AlertSource: Send + Sync {
    async fn alerts(&self) -> Result<Vec<Alert>>;
}

/// The NWS radar status used to choose the mode. Kept separate from the
/// forecast and alerts so a status outage cannot couple to those layers.
#[async_trait]
pub trait RadarStatusSource: Send + Sync {
    async fn radar_status(&self) -> Result<RadarStatus>;
}

/// The three independent source handles the server uses. Construct one from a
/// provider that implements every source so all calls share its client and
/// configuration, while each consumer depends only on the data it needs.
#[derive(Clone)]
pub struct Sources {
    pub forecast: Arc<dyn ForecastSource>,
    pub alerts: Arc<dyn AlertSource>,
    pub status: Arc<dyn RadarStatusSource>,
}

impl Sources {
    pub fn from_provider<P>(provider: Arc<P>) -> Self
    where
        P: ForecastSource + AlertSource + RadarStatusSource + 'static,
    {
        let forecast: Arc<dyn ForecastSource> = provider.clone();
        let alerts: Arc<dyn AlertSource> = provider.clone();
        let status: Arc<dyn RadarStatusSource> = provider;
        Sources {
            forecast,
            alerts,
            status,
        }
    }
}

#[async_trait]
pub trait RadarSource: Send + Sync {
    /// Newest archive object for `nexrad_id`, looking at today and then
    /// yesterday (UTC) as `sources/nexrad_level2.py` does.
    async fn latest(&self, nexrad_id: &str, now: DateTime<Utc>) -> Result<Option<RadarObject>>;

    /// Stream the object to `destination`, returning the number of bytes
    /// written. Implementations cap the transfer at
    /// [`crate::nexrad::archive2::MAX_VOLUME_BYTES`].
    async fn download(
        &self,
        radar_object: &RadarObject,
        destination: &std::path::Path,
    ) -> Result<u64>;
}
