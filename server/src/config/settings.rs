//! The configuration `serve` and the renderer both depend on, parsed once.
//!
//! The bbox and timezone were previously parsed separately by `AppState` and
//! `FrameRenderer::new`, so a startup could validate one set of values while the
//! renderer used another. [`Settings::from_config`] parses and validates the
//! values exactly once; [`crate::service::startup::validate`] adds the artifact checks
//! and returns the same value.

use std::path::PathBuf;

use chrono_tz::Tz;

use crate::config::constants::{RADAR_HEIGHT, WIDTH};
use crate::config::{Config, DEFAULT_USER_AGENT};
use crate::error::{Error, Result};
use crate::render::raster::Transform;
use crate::service::renderer::Bounds;

#[derive(Clone, Debug)]
pub struct Settings {
    pub nexrad_id: String,
    pub bounds: Bounds,
    pub timezone: Tz,
    pub latitude: f64,
    pub longitude: f64,
    pub zone: String,
    pub fire_zone: String,
    pub user_agent: String,
    /// `User-Agent` sent with Overpass street-data requests.
    pub osm_user_agent: String,
    /// Optional `[render] colors` override; the embedded table is the default.
    pub colors: Option<PathBuf>,
    /// True when a correctly sized street mask is present. A missing mask is
    /// allowed; the background builder fills it in when Overpass is reachable.
    pub streets: bool,
}

impl Settings {
    /// Parse and validate the settings `serve` needs. Artifacts are checked by
    /// [`crate::service::startup::validate`], which owns this value.
    pub fn from_config(config: &Config) -> Result<Self> {
        let nexrad_id = required(config, "radar", "nexrad_id")?;
        if nexrad_id.len() != 4 || !nexrad_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(Error::config(format!(
                "config: [radar] nexrad_id must be four letters or digits, got {nexrad_id:?}"
            )));
        }

        let bounds = bounds(config)?;
        let timezone = config.timezone("forecast", "timezone")?;
        let latitude = coordinate(config, "forecast", "lat", 90.0)?;
        let longitude = coordinate(config, "forecast", "lon", 180.0)?;

        let zone = required(config, "nws", "zone")?;
        let fire_zone = required(config, "nws", "fire_zone")?;
        let user_agent = contact_user_agent(config, "nws", "NWS")?;
        let osm_user_agent = contact_user_agent(config, "osm", "Overpass")?;

        Ok(Settings {
            nexrad_id,
            bounds,
            timezone,
            latitude,
            longitude,
            zone,
            fire_zone,
            user_agent,
            osm_user_agent,
            colors: config.colors(),
            streets: false,
        })
    }
}

/// A user agent has to identify a contact, not the generic fallback. Both the
/// NWS API and Overpass ask for one.
fn contact_user_agent(config: &Config, section: &str, provider: &str) -> Result<String> {
    let value = config
        .get_opt(section, "user_agent")
        .unwrap_or(DEFAULT_USER_AGENT)
        .trim();
    if value.is_empty() || value == DEFAULT_USER_AGENT {
        return Err(Error::config(format!(
            "config: [{section}] user_agent must identify a contact; the {provider} API requires one"
        )));
    }
    Ok(value.to_string())
}

fn required(config: &Config, section: &str, key: &str) -> Result<String> {
    let value = config.get(section, key)?;
    if value.trim().is_empty() {
        return Err(Error::config(format!(
            "config: [{section}] {key} must not be empty"
        )));
    }
    Ok(value.trim().to_string())
}

/// Parse a coordinate and reject values the projection cannot use.
fn coordinate(config: &Config, section: &str, key: &str, limit: f64) -> Result<f64> {
    let value = config.f64(section, key)?;
    if !value.is_finite() {
        return Err(Error::config(format!(
            "config: [{section}] {key} must be finite, got {value}"
        )));
    }
    if value.abs() > limit {
        return Err(Error::config(format!(
            "config: [{section}] {key} must be within +/-{limit}, got {value}"
        )));
    }
    Ok(value)
}

fn bounds(config: &Config) -> Result<Bounds> {
    let min_lon = coordinate(config, "bbox", "min_lon", 180.0)?;
    let min_lat = coordinate(config, "bbox", "min_lat", 90.0)?;
    let max_lon = coordinate(config, "bbox", "max_lon", 180.0)?;
    let max_lat = coordinate(config, "bbox", "max_lat", 90.0)?;

    // The renderer's own validation, so a bbox that would fail later fails
    // here with the same explanation.
    Transform::new(min_lon, min_lat, max_lon, max_lat, WIDTH, RADAR_HEIGHT)
        .map_err(|error| Error::config(format!("config: [bbox]: {error}")))?;

    Ok(Bounds {
        min_lon,
        min_lat,
        max_lon,
        max_lat,
    })
}
