//! Open-Meteo forecast source.
//!
//! The Python source issued three requests per cycle (current, hourly, daily).
//! Their fields are disjoint parts of the same endpoint, so this source fetches
//! all three in one request and keeps the original URL parameters, including
//! `temperature_unit=fahrenheit` and the configured time zone.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use reqwest::Url;
use serde::Deserialize;

use super::Forecast;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::view::forecast::{CurrentConditions, ForecastDay, ForecastHour};

pub const BASE_URL: &str = "https://api.open-meteo.com/v1/forecast";

#[derive(Deserialize)]
struct Response {
    current_weather: Current,
    hourly: Hourly,
    daily: Daily,
}

#[derive(Deserialize)]
struct Current {
    temperature: f64,
    weathercode: i64,
    is_day: i64,
}

#[derive(Deserialize)]
struct Hourly {
    time: Vec<String>,
    weathercode: Vec<i64>,
    temperature_2m: Vec<f64>,
}

#[derive(Deserialize)]
struct Daily {
    time: Vec<String>,
    weathercode: Vec<i64>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
}

pub async fn fetch(
    client: &reqwest::Client,
    config: &Config,
    now: DateTime<Utc>,
) -> Result<Forecast> {
    let latitude = config.get("forecast", "lat")?;
    let longitude = config.get("forecast", "lon")?;
    let timezone: Tz = config.timezone("forecast", "timezone")?;

    let url = Url::parse_with_params(
        BASE_URL,
        &[
            ("latitude", latitude),
            ("longitude", longitude),
            ("timezone", config.get("forecast", "timezone")?),
            ("current_weather", "true"),
            ("hourly", "weathercode,temperature_2m"),
            ("daily", "weathercode,temperature_2m_max,temperature_2m_min"),
            ("temperature_unit", "fahrenheit"),
            ("forecast_days", "7"),
        ],
    )
    .map_err(|e| Error::new(format!("open-meteo url: {e}")))?;

    let body = super::http::get(client, url, "application/json", "open-meteo").await?;
    parse(&body, now, timezone)
}

/// Parse a response into the panels the clear view draws.
///
/// Hourly entries before the current local hour are dropped and labelled in
/// the configured zone (`%-I%p`, lowercased), and daily entries are labelled
/// with the lowercased English weekday, matching `sources/open_meteo.py`.
pub fn parse(body: &str, now: DateTime<Utc>, timezone: Tz) -> Result<Forecast> {
    let response: Response = serde_json::from_str(body)?;

    if response.hourly.time.len() != response.hourly.weathercode.len()
        || response.hourly.time.len() != response.hourly.temperature_2m.len()
    {
        return Err(Error::parse(
            "open-meteo: hourly arrays have different lengths",
        ));
    }
    if response.daily.time.len() != response.daily.weathercode.len()
        || response.daily.time.len() != response.daily.temperature_2m_max.len()
        || response.daily.time.len() != response.daily.temperature_2m_min.len()
    {
        return Err(Error::parse(
            "open-meteo: daily arrays have different lengths",
        ));
    }

    let now_local = now.with_timezone(&timezone);
    let now_local_hour = now_local
        .with_minute(0)
        .and_then(|time| time.with_second(0))
        .and_then(|time| time.with_nanosecond(0))
        .ok_or_else(|| {
            Error::new(format!(
                "open-meteo: cannot truncate {now_local} to the hour"
            ))
        })?;

    let mut hours = Vec::new();
    for (index, raw) in response.hourly.time.iter().enumerate() {
        let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
            .map_err(|e| Error::parse(format!("open-meteo hourly time {raw}: {e}")))?;

        // pytz's `localize` without `is_dst` resolves ambiguous wall times to
        // standard time, which is chrono's `latest()`.
        let Some(time) = timezone.from_local_datetime(&naive).latest() else {
            tracing::warn!("open-meteo: skipping nonexistent local time {naive}");
            continue;
        };

        if time < now_local_hour {
            continue;
        }

        hours.push(ForecastHour {
            hour: time.format("%-I%P").to_string().to_lowercase(),
            temp_f: response.hourly.temperature_2m[index],
            code: response.hourly.weathercode[index] as i32,
        });
    }

    let mut days = Vec::new();
    for (index, raw) in response.daily.time.iter().enumerate() {
        let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .map_err(|e| Error::parse(format!("open-meteo daily time {raw}: {e}")))?;
        days.push(ForecastDay {
            day: date.format("%a").to_string().to_lowercase(),
            high_f: response.daily.temperature_2m_max[index],
            low_f: response.daily.temperature_2m_min[index],
            code: response.daily.weathercode[index] as i32,
        });
    }

    Ok(Forecast {
        current: CurrentConditions {
            temp_f: response.current_weather.temperature,
            code: response.current_weather.weathercode as i32,
            is_day: response.current_weather.is_day == 1,
        },
        hourly: hours,
        daily: days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time_in_chicago(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn sample() -> String {
        sample_from(10)
    }

    fn sample_from(start: u32) -> String {
        let hours: Vec<String> = (start..start + 6)
            .map(|hour| format!("\"2026-10-21T{hour:02}:00\""))
            .collect();
        format!(
            r#"{{
              "current_weather": {{"temperature": 71.6, "weathercode": 3, "is_day": 1}},
              "hourly": {{
                "time": [{}],
                "weathercode": [0, 1, 2, 3, 4, 5],
                "temperature_2m": [50.0, 51.0, 52.0, 53.0, 54.0, 55.0]
              }},
              "daily": {{
                "time": ["2026-10-20", "2026-10-21", "2026-10-22"],
                "weathercode": [1, 2, 3],
                "temperature_2m_max": [60.0, 61.0, 62.0],
                "temperature_2m_min": [40.0, 41.0, 42.0]
              }}
            }}"#,
            hours.join(",")
        )
    }

    #[test]
    fn drops_hours_before_the_current_local_hour() {
        // 15:30Z is 10:30 CDT on 2026-10-21.
        let forecast = parse(
            &sample(),
            time_in_chicago("2026-10-21T15:30:00Z"),
            chrono_tz::America::Chicago,
        )
        .unwrap();

        assert_eq!(forecast.current.temp_f, 71.6);
        assert_eq!(forecast.current.code, 3);
        assert!(forecast.current.is_day);

        let labels: Vec<&str> = forecast.hourly.iter().map(|h| h.hour.as_str()).collect();
        assert_eq!(labels, ["10am", "11am", "12pm", "1pm", "2pm", "3pm"]);
        assert_eq!(forecast.hourly[0].temp_f, 50.0);

        let days: Vec<&str> = forecast.daily.iter().map(|d| d.day.as_str()).collect();
        assert_eq!(days, ["tue", "wed", "thu"]);
        assert_eq!(forecast.daily[1].low_f, 41.0);
    }

    #[test]
    fn rejects_mismatched_arrays() {
        let broken = sample().replace(
            "\"temperature_2m\": [50.0, 51.0, 52.0, 53.0, 54.0, 55.0]",
            "\"temperature_2m\": [50.0]",
        );
        let error = parse(
            &broken,
            time_in_chicago("2026-10-21T15:30:00Z"),
            chrono_tz::America::Chicago,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("hourly arrays"), "{error}");
    }

    #[test]
    fn midnight_is_labelled_without_a_leading_zero() {
        let forecast = parse(
            &sample_from(0),
            time_in_chicago("2026-10-21T05:05:00Z"), // 00:05 CDT
            chrono_tz::America::Chicago,
        )
        .unwrap();
        assert_eq!(forecast.hourly[0].hour, "12am");
    }
}
