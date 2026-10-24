use chrono::{DateTime, FixedOffset, Utc};
use serde::Deserialize;

use crate::sources::Forecast;

#[derive(Clone, Debug, Deserialize)]
pub struct CurrentConditions {
    pub temp_f: f64,
    pub code: i32,
    pub is_day: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ForecastHour {
    pub hour: String,
    pub temp_f: f64,
    pub code: i32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ForecastDay {
    pub day: String,
    pub high_f: f64,
    pub low_f: f64,
    pub code: i32,
}

/// Severity order, matching `AlertLevel`'s `auto()` values in the Python
/// source (Info is least severe).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
pub enum AlertLevel {
    Info,
    Watch,
    Warning,
    Emergency,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Alert {
    pub event: String,
    pub level: AlertLevel,
    pub status: String,
    pub effective: DateTime<FixedOffset>,
    pub expires: DateTime<FixedOffset>,
}

impl Alert {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.status == "actual"
            && now > self.effective.with_timezone(&Utc)
            && now < self.expires.with_timezone(&Utc)
    }
}

/// JSON input for `view clear` and the view fixtures. The panels come from
/// [`Forecast`]; alerts travel alongside it.
#[derive(Clone, Debug, Deserialize)]
pub struct ClearInput {
    #[serde(flatten)]
    pub forecast: Forecast,
    #[serde(default)]
    pub alerts: Vec<Alert>,
}
