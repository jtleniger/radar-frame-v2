//! Offline data, ported value for value from `sources/mock_forecast.py`.

use async_trait::async_trait;

use super::{Forecast, ForecastProvider, RadarStatus};
use crate::error::Result;
use frame_view::forecast::{Alert, CurrentConditions, ForecastDay, ForecastHour};

pub struct MockProvider {
    forecast: Forecast,
}

impl MockProvider {
    pub fn new() -> Self {
        MockProvider {
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
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ForecastProvider for MockProvider {
    async fn forecast(&self) -> Result<&Forecast> {
        Ok(&self.forecast)
    }

    async fn alerts(&self) -> Result<Vec<Alert>> {
        Ok(Vec::new())
    }

    async fn radar_status(&self) -> Result<RadarStatus> {
        Ok(RadarStatus { up: true, vcp: 31 })
    }
}
