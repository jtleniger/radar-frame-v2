use chrono::{DateTime, FixedOffset, Utc};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CurrentConditions {
    pub temp_f: f64,
    pub code: i32,
    pub is_day: bool,
}

#[derive(Debug, Deserialize)]
pub struct ForecastHour {
    pub hour: String,
    pub temp_f: f64,
    pub code: i32,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
pub struct ClearInput {
    pub current: CurrentConditions,
    pub hourly: Vec<ForecastHour>,
    pub daily: Vec<ForecastDay>,
    #[serde(default)]
    pub alerts: Vec<Alert>,
}
