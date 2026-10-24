//! Offline data for the mock providers and the demo commands. The forecast is
//! ported value for value from `sources/mock_forecast.py`; the radar volume is
//! a trimmed real KTLX archive embedded at build time.

use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use super::{
    AlertSource, Forecast, ForecastSource, RadarObject, RadarSource, RadarStatus, RadarStatusSource,
};
use crate::error::{Error, Result};
use crate::view::forecast::{Alert, AlertLevel, CurrentConditions, ForecastDay, ForecastHour};

pub struct MockProvider {
    forecast: Forecast,
    alerts: Vec<Alert>,
}

impl MockProvider {
    /// The frozen forecast with no alerts, matching the fixture goldens.
    pub fn new() -> Self {
        Self::with_alerts(Vec::new())
    }

    /// The frozen forecast plus three alerts that are active right now, for
    /// checking the warning bar and the alert pipeline offline.
    pub fn demo() -> Self {
        let now = Utc::now();
        let alert = |event: &str, level: AlertLevel, expires_minutes: i64| Alert {
            event: event.into(),
            level,
            status: "actual".into(),
            effective: (now - Duration::minutes(15)).fixed_offset(),
            expires: (now + Duration::minutes(expires_minutes)).fixed_offset(),
        };

        Self::with_alerts(vec![
            alert("tornado warning", AlertLevel::Emergency, 30),
            alert("severe thunderstorm warning", AlertLevel::Warning, 45),
            alert("flash flood watch", AlertLevel::Watch, 180),
        ])
    }

    pub fn with_alerts(alerts: Vec<Alert>) -> Self {
        MockProvider {
            alerts,
            forecast: Forecast {
                current: CurrentConditions {
                    temp_f: 101.0,
                    code: 57,
                    is_day: false,
                },
                hourly: vec![
                    ForecastHour {
                        hour: "9pm".into(),
                        temp_f: 24.0,
                        code: 3,
                    },
                    ForecastHour {
                        hour: "10pm".into(),
                        temp_f: 24.0,
                        code: 50,
                    },
                    ForecastHour {
                        hour: "11pm".into(),
                        temp_f: 24.0,
                        code: 53,
                    },
                    ForecastHour {
                        hour: "12am".into(),
                        temp_f: 24.0,
                        code: 95,
                    },
                    ForecastHour {
                        hour: "1am".into(),
                        temp_f: 124.0,
                        code: 77,
                    },
                    ForecastHour {
                        hour: "2am".into(),
                        temp_f: 24.0,
                        code: 95,
                    },
                    ForecastHour {
                        hour: "3am".into(),
                        temp_f: 124.0,
                        code: 77,
                    },
                    ForecastHour {
                        hour: "4am".into(),
                        temp_f: 24.0,
                        code: 95,
                    },
                    ForecastHour {
                        hour: "5am".into(),
                        temp_f: 124.0,
                        code: 77,
                    },
                ],
                daily: vec![
                    ForecastDay {
                        day: "tue".into(),
                        high_f: 38.8,
                        low_f: 123.2,
                        code: 1,
                    },
                    ForecastDay {
                        day: "wed".into(),
                        high_f: 112.3,
                        low_f: -25.0,
                        code: 2,
                    },
                    ForecastDay {
                        day: "thu".into(),
                        high_f: 38.8,
                        low_f: 12.2,
                        code: 3,
                    },
                    ForecastDay {
                        day: "fri".into(),
                        high_f: 38.8,
                        low_f: 123.2,
                        code: 45,
                    },
                    ForecastDay {
                        day: "tue".into(),
                        high_f: 38.8,
                        low_f: 123.2,
                        code: 53,
                    },
                    ForecastDay {
                        day: "tue".into(),
                        high_f: 38.8,
                        low_f: 123.2,
                        code: 66,
                    },
                    ForecastDay {
                        day: "tue".into(),
                        high_f: 38.8,
                        low_f: 123.2,
                        code: 77,
                    },
                    ForecastDay {
                        day: "tue".into(),
                        high_f: 38.8,
                        low_f: 123.2,
                        code: 66,
                    },
                    ForecastDay {
                        day: "tue".into(),
                        high_f: 38.8,
                        low_f: 123.2,
                        code: 77,
                    },
                ],
            },
        }
    }
}

impl MockProvider {
    /// The frozen data, for fixture generation and tests.
    pub fn forecast_data(&self) -> &Forecast {
        &self.forecast
    }

    /// The configured alerts, for `check warnings --mock`.
    pub fn alerts_data(&self) -> &[Alert] {
        &self.alerts
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ForecastSource for MockProvider {
    async fn forecast(&self) -> Result<Forecast> {
        Ok(self.forecast.clone())
    }
}

#[async_trait]
impl AlertSource for MockProvider {
    async fn alerts(&self) -> Result<Vec<Alert>> {
        Ok(self.alerts.clone())
    }
}

#[async_trait]
impl RadarStatusSource for MockProvider {
    async fn radar_status(&self) -> Result<RadarStatus> {
        Ok(RadarStatus { up: true, vcp: 31 })
    }
}

/// A real but tiny KTLX volume compiled into the binary. It is small enough to
/// embed and still exercises the full download, parse, and rasterize path.
const RADAR_VOLUME: &[u8] = include_bytes!("../../assets/ktlx-mini.ar2");

/// Radar data for `demo radar --mock`: no network calls and deterministic
/// bytes. The synthetic key keeps the printed provenance distinct from S3.
pub struct MockRadar;

#[async_trait]
impl RadarSource for MockRadar {
    async fn latest(&self, nexrad_id: &str, now: DateTime<Utc>) -> Result<Option<RadarObject>> {
        Ok(Some(RadarObject {
            key: format!("mock/{nexrad_id}-mini.ar2"),
            last_modified: now,
        }))
    }

    async fn download(&self, _radar_object: &RadarObject, destination: &Path) -> Result<u64> {
        tokio::fs::write(destination, RADAR_VOLUME)
            .await
            .map_err(|error| Error::io_path(destination, &error))?;
        Ok(RADAR_VOLUME.len() as u64)
    }
}
