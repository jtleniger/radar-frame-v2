//! Forecast data and the two providers the server selects between.
//!
//! `OnlineProvider` reads Open-Meteo and the NWS API; `MockProvider` mirrors
//! `sources/mock_forecast.py` for offline development and fixtures. The data
//! types are `frame-view`'s, so the composed image consumes them directly.

pub mod mock;
pub mod nexrad;
pub mod nws;
pub mod online;
pub mod open_meteo;
pub mod osm;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::Result;
use crate::view::forecast::{Alert, CurrentConditions, ForecastDay, ForecastHour};

/// One forecast cycle: what the clear view draws and nothing else.
#[derive(Clone, Debug)]
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

/// One archive object, with the modification time the schedule compares.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Object {
    pub key: String,
    pub last_modified: DateTime<Utc>,
}

#[async_trait]
pub trait ForecastProvider: Send + Sync {
    /// Current conditions plus the hourly and daily panels. Called at most once
    /// per update cycle; the online provider fetches all of it in one request.
    async fn forecast(&self) -> Result<&Forecast>;

    async fn alerts(&self) -> Result<Vec<Alert>>;

    async fn radar_status(&self) -> Result<RadarStatus>;
}

#[async_trait]
pub trait RadarSource: Send + Sync {
    /// Newest archive object for `nexrad_id`, looking at today and then
    /// yesterday (UTC) as `sources/nexrad_level2.py` does.
    async fn latest(&self, nexrad_id: &str, now: DateTime<Utc>) -> Result<Option<Object>>;

    /// The object's bytes. The caller writes them to `data/nexrad-latest.ar2`.
    async fn download(&self, object: &Object) -> Result<Vec<u8>>;
}
