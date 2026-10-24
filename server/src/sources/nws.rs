//! NWS API: active alerts and radar station status.
//!
//! Both requests carry the configured `User-Agent` contact string; the client
//! is built once with it (see `http_client`). Responses are parsed separately
//! from the HTTP calls so fixtures drive the tests.

use chrono::{DateTime, FixedOffset};
use serde::Deserialize;

use super::RadarStatus;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::view::forecast::{Alert, AlertLevel};

pub const BASE_URL: &str = "https://api.weather.gov/";

#[derive(Deserialize)]
struct AlertsResponse {
    features: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Feature {
    properties: Properties,
}

#[derive(Deserialize)]
struct Properties {
    event: String,
    urgency: String,
    severity: String,
    certainty: String,
    status: String,
    effective: String,
    expires: String,
}

pub async fn alerts(client: &reqwest::Client, config: &Config) -> Result<Vec<Alert>> {
    let zones = format!(
        "{},{}",
        config.get("nws", "zone")?,
        config.get("nws", "fire_zone")?
    );
    let url = reqwest::Url::parse_with_params(
        &format!("{BASE_URL}alerts/active"),
        &[("zone", zones.as_str())],
    )
    .map_err(|e| Error::new(format!("nws alerts url: {e}")))?;

    let body = super::http::get(
        client,
        url,
        "application/geo+json, application/json",
        "nws-alerts",
    )
    .await?;
    parse_alerts(&body)
}

pub async fn radar_status(client: &reqwest::Client, config: &Config) -> Result<RadarStatus> {
    let nexrad_id = config.get("radar", "nexrad_id")?;
    let url = reqwest::Url::parse(&format!("{BASE_URL}radar/stations/{nexrad_id}"))
        .map_err(|e| Error::new(format!("nws radar url: {e}")))?;

    let body = super::http::get(
        client,
        url,
        "application/geo+json, application/json",
        "nws-radar-status",
    )
    .await?;
    parse_radar_status(&body)
}

pub fn parse_alerts(body: &str) -> Result<Vec<Alert>> {
    let response: AlertsResponse = serde_json::from_str(body)?;
    let mut alerts = Vec::with_capacity(response.features.len());

    // One malformed feature must not discard the alerts around it.
    for feature in response.features {
        let parsed = serde_json::from_value::<Feature>(feature)
            .map_err(Error::from)
            .and_then(|feature| alert(&feature.properties));
        match parsed {
            Ok(alert) => alerts.push(alert),
            Err(error) => tracing::warn!("skipping malformed nws alert: {error}"),
        }
    }

    Ok(alerts)
}

fn alert(properties: &Properties) -> Result<Alert> {
    let urgency = properties.urgency.to_lowercase();
    let severity = properties.severity.to_lowercase();
    let certainty = properties.certainty.to_lowercase();

    // The level rules are `Alert.from_dict` in `sources/forecast.py`.
    let level = if urgency == "future"
        && matches!(severity.as_str(), "extreme" | "severe" | "moderate")
        && certainty == "possible"
    {
        AlertLevel::Watch
    } else if matches!(urgency.as_str(), "immediate" | "expected")
        && matches!(certainty.as_str(), "likely" | "observed")
        && matches!(severity.as_str(), "extreme" | "severe")
    {
        if severity == "extreme" {
            AlertLevel::Emergency
        } else {
            AlertLevel::Warning
        }
    } else {
        AlertLevel::Info
    };

    Ok(Alert {
        event: properties.event.to_lowercase(),
        level,
        status: properties.status.to_lowercase(),
        effective: timestamp("effective", &properties.effective)?,
        expires: timestamp("expires", &properties.expires)?,
    })
}

fn timestamp(field: &str, value: &str) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value)
        .map_err(|e| Error::parse(format!("nws alert {field} {value}: {e}")))
}

#[derive(Deserialize)]
struct Station {
    properties: Option<StationProperties>,
}

#[derive(Deserialize)]
struct StationProperties {
    rda: Option<Rda>,
}

#[derive(Deserialize)]
struct Rda {
    properties: Option<RdaProperties>,
}

#[derive(Deserialize)]
struct RdaProperties {
    #[serde(rename = "volumeCoveragePattern")]
    volume_coverage_pattern: Option<String>,
    status: Option<String>,
}

pub fn parse_radar_status(body: &str) -> Result<RadarStatus> {
    let station: Station = serde_json::from_str(body)?;
    let Some(rda) = station
        .properties
        .and_then(|properties| properties.rda)
        .and_then(|rda| rda.properties)
    else {
        // A station without RDA response cannot be reporting precipitation, so
        // treat it as down and let the mode fall back to Clear.
        tracing::warn!("nws station response has no rda properties; treating radar as down");
        return Ok(RadarStatus { up: false, vcp: -1 });
    };

    let vcp = rda
        .volume_coverage_pattern
        .as_deref()
        .and_then(first_number)
        .unwrap_or(-1);

    Ok(RadarStatus {
        up: rda.status.as_deref() == Some("Operate"),
        vcp,
    })
}

/// First run of ASCII digits, the equivalent of `re.search(r'[0-9]+', ...)`.
fn first_number(value: &str) -> Option<i32> {
    let digits: String = value
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/api/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn maps_nws_severity_to_alert_levels() {
        let alerts = parse_alerts(&fixture("nws-alerts.json")).unwrap();

        assert_eq!(alerts.len(), 4);
        assert_eq!(alerts[0].event, "tornado warning");
        assert_eq!(alerts[0].level, AlertLevel::Emergency);
        assert_eq!(alerts[0].status, "actual");
        assert_eq!(alerts[1].level, AlertLevel::Warning);
        assert_eq!(alerts[2].level, AlertLevel::Watch);
        assert_eq!(alerts[3].level, AlertLevel::Info);
        assert_eq!(
            alerts[2].effective.to_string(),
            "2026-10-21 10:24:56 -05:00"
        );
    }

    #[test]
    fn reads_vcp_and_status() {
        let status = parse_radar_status(
            r#"{"properties":{"rda":{"properties":{"volumeCoveragePattern":"VCP 212","status":"Operate"}}}}"#,
        )
        .unwrap();
        assert!(status.up);
        assert_eq!(status.vcp, 212);
        assert!(!status.clear_air_mode());
    }

    #[test]
    fn skips_malformed_alert_features() {
        let body = r#"{
            "features": [
                {"properties": {"event": "Tornado Warning", "urgency": "Immediate", "severity": "Extreme", "certainty": "Observed", "status": "Actual", "effective": "2026-10-21T10:24:56-05:00", "expires": "2026-10-21T13:34:56-05:00"}},
                {"properties": {"event": "Bad timestamp", "urgency": "Immediate", "severity": "Extreme", "certainty": "Observed", "status": "Actual", "effective": "yesterday", "expires": "2026-10-21T13:34:56-05:00"}},
                {"properties": {"event": "Missing fields"}}
            ]
        }"#;

        let alerts = parse_alerts(body).unwrap();

        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].event, "tornado warning");
        assert_eq!(alerts[0].level, AlertLevel::Emergency);
    }

    #[test]
    fn missing_rda_is_down_with_unknown_vcp() {
        for body in [
            r#"{}"#,
            r#"{"properties": {}}"#,
            r#"{"properties": {"rda": null}}"#,
            r#"{"properties": {"rda": {}}}"#,
        ] {
            let status = parse_radar_status(body).unwrap();
            assert!(!status.up, "{body}");
            assert_eq!(status.vcp, -1, "{body}");
        }
    }

    #[test]
    fn invalid_station_json_is_an_upstream_error() {
        assert!(parse_radar_status("not json").is_err());
    }

    #[test]
    fn missing_vcp_is_minus_one() {
        let status = parse_radar_status(
            r#"{"properties":{"rda":{"properties":{"volumeCoveragePattern":"unknown","status":"Standby"}}}}"#,
        )
        .unwrap();
        assert!(!status.up);
        assert_eq!(status.vcp, -1);
    }

    #[test]
    fn clear_air_patterns() {
        for vcp in [31, 32, 35] {
            assert!(RadarStatus { up: true, vcp }.clear_air_mode());
        }
        for vcp in [11, 12, 212] {
            assert!(!RadarStatus { up: true, vcp }.clear_air_mode());
        }
    }
}
