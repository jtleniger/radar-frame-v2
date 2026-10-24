//! Startup validation for `serve`.
//!
//! The service used to bind first and discover a bad bbox, a missing zone, or
//! an unwritable `data/` only when the first poll arrived. Validate everything
//! the first frame depends on up front so a misconfiguration fails in under a
//! second with a message naming the offending setting or artifact.
//!
//! The street mask is the one exception: the overlay is cosmetic, so a missing
//! or unreadable mask is reported and rebuilt in the background instead of
//! stopping the service (see `setup::retry_streets`).

use chrono_tz::Tz;

use crate::config::{Config, DEFAULT_USER_AGENT};
use crate::constants::{RADAR_HEIGHT, WIDTH};
use crate::engine::Bounds;
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::raster::Transform;
use crate::setup;
use crate::view::palette::{Palette, EINK_COLORS};

/// The configuration the service validated, reused for the startup summary.
#[derive(Clone, Debug)]
pub struct Validated {
    pub nexrad_id: String,
    pub bounds: Bounds,
    pub timezone: Tz,
    pub latitude: f64,
    pub longitude: f64,
    pub zone: String,
    pub fire_zone: String,
    pub user_agent: String,
    /// True when a correctly sized street mask is present. A missing mask is
    /// allowed; the background builder fills it in when Overpass is reachable.
    pub streets: bool,
}

/// Validate the settings and artifacts `serve` needs before it binds.
pub fn validate(config: &Config, paths: &Paths) -> Result<Validated> {
    let nexrad_id = required(config, "radar", "nexrad_id")?;
    if nexrad_id.len() != 4 || !nexrad_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error::new(format!(
            "config: [radar] nexrad_id must be four letters or digits, got {nexrad_id:?}"
        )));
    }

    let bounds = bounds(config)?;
    let timezone = config.timezone("forecast", "timezone")?;
    let latitude = coordinate(config, "forecast", "lat", 90.0)?;
    let longitude = coordinate(config, "forecast", "lon", 180.0)?;

    let zone = required(config, "nws", "zone")?;
    let fire_zone = required(config, "nws", "fire_zone")?;
    let user_agent = config.user_agent().trim();
    if user_agent.is_empty() || user_agent == DEFAULT_USER_AGENT {
        return Err(Error::new(
            "config: [nws] user_agent must identify a contact; the NWS API requires one",
        ));
    }

    validate_data_dir(paths)?;
    validate_palette(paths)?;
    let streets = validate_streets(paths);

    Ok(Validated {
        nexrad_id,
        bounds,
        timezone,
        latitude,
        longitude,
        zone,
        fire_zone,
        user_agent: user_agent.to_string(),
        streets,
    })
}

fn required(config: &Config, section: &str, key: &str) -> Result<String> {
    let value = config.get(section, key)?;
    if value.trim().is_empty() {
        return Err(Error::new(format!(
            "config: [{section}] {key} must not be empty"
        )));
    }
    Ok(value.trim().to_string())
}

/// Parse a coordinate and reject values the projection cannot use.
fn coordinate(config: &Config, section: &str, key: &str, limit: f64) -> Result<f64> {
    let value = config.f64(section, key)?;
    if !value.is_finite() {
        return Err(Error::new(format!(
            "config: [{section}] {key} must be finite, got {value}"
        )));
    }
    if value.abs() > limit {
        return Err(Error::new(format!(
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
        .map_err(|error| Error::new(format!("config: [bbox]: {error}")))?;

    Ok(Bounds {
        min_lon,
        min_lat,
        max_lon,
        max_lat,
    })
}

fn validate_data_dir(paths: &Paths) -> Result<()> {
    let data = paths.data_dir();
    if !data.is_dir() {
        return Err(Error::new(format!(
            "startup: data directory {} does not exist; create it or run `radar-frame setup`",
            data.display()
        )));
    }
    if !setup::data_writable(paths) {
        return Err(Error::new(format!(
            "startup: data directory {} is not writable",
            data.display()
        )));
    }
    Ok(())
}

fn validate_palette(paths: &Paths) -> Result<()> {
    let path = paths.palette_img();
    let palette = Palette::from_png(&path)
        .map_err(|error| Error::new(format!("startup: {}: {error}", path.display())))?;
    if palette.colors.len() != EINK_COLORS.len() {
        return Err(Error::new(format!(
            "startup: {} has {} colours, expected the {}-colour e-ink palette",
            path.display(),
            palette.colors.len(),
            EINK_COLORS.len()
        )));
    }
    Ok(())
}

/// Check a present mask and report whether it can be used. Problems are
/// warnings, not startup failures: `setup::create_streets` regenerates an
/// invalid mask and the frame renders without the overlay until it succeeds.
fn validate_streets(paths: &Paths) -> bool {
    let path = paths.streets_img();
    if !path.is_file() {
        tracing::warn!(
            event = "startup",
            path = %path.display(),
            "street mask missing; rendering without the overlay until it is built"
        );
        return false;
    }

    match crate::render::pngio::read_gray8(&path) {
        Ok(mask) if (mask.width, mask.height) == (WIDTH, RADAR_HEIGHT) => true,
        Ok(mask) => {
            tracing::warn!(
                event = "startup",
                path = %path.display(),
                width = mask.width,
                height = mask.height,
                "street mask is the wrong size; the background builder will replace it"
            );
            false
        }
        Err(error) => {
            tracing::warn!(
                event = "startup",
                path = %path.display(),
                error = %error,
                "street mask is unreadable; the background builder will replace it"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "\
[logging]
level = INFO

[radar]
nexrad_id = KTLX

[bbox]
min_lat = 35.35
max_lat = 35.39
min_lon = -97.26
max_lon = -97.21

[forecast]
lat = 35.37
lon = -97.23
timezone = America/Chicago

[nws]
user_agent = radar-frame (test@example.com)
zone = OKZ025
fire_zone = OKZ025
";

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: Paths,
        config: Config,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let paths = Paths::new(dir.path());
            std::fs::create_dir_all(paths.data_dir()).unwrap();
            crate::view::palette::write_eink_png(&paths.palette_img()).unwrap();
            Fixture {
                _dir: dir,
                paths,
                config: Config::parse(VALID).unwrap(),
            }
        }

        fn write_streets(&self, width: usize, height: usize) {
            crate::render::pngio::write_gray8(
                &self.paths.streets_img(),
                width,
                height,
                &vec![1; width * height],
                None,
            )
            .unwrap();
        }

        fn error(&self) -> String {
            validate(&self.config, &self.paths)
                .expect_err("expected validation to fail")
                .to_string()
        }
    }

    #[test]
    fn valid_configuration_is_accepted() {
        let fixture = Fixture::new();
        let validated = validate(&fixture.config, &fixture.paths).unwrap();
        assert_eq!(validated.nexrad_id, "KTLX");
        assert_eq!(validated.bounds.min_lon, -97.26);
        assert_eq!(validated.timezone, chrono_tz::America::Chicago);
        assert_eq!((validated.latitude, validated.longitude), (35.37, -97.23));
        assert_eq!(validated.zone, "OKZ025");
        assert_eq!(validated.fire_zone, "OKZ025");
        assert!(validated.user_agent.starts_with("radar-frame"));
        assert!(!validated.streets);
    }

    #[test]
    fn valid_street_mask_is_reported() {
        let fixture = Fixture::new();
        fixture.write_streets(WIDTH, RADAR_HEIGHT);
        assert!(validate(&fixture.config, &fixture.paths).unwrap().streets);
    }

    #[test]
    fn wrong_sized_street_mask_does_not_block_startup() {
        let fixture = Fixture::new();
        fixture.write_streets(10, 10);
        let validated = validate(&fixture.config, &fixture.paths).unwrap();
        assert!(!validated.streets);
    }

    #[test]
    fn degenerate_bbox_is_rejected() {
        let fixture = Fixture::new();
        let config = Config::parse(&VALID.replace("max_lon = -97.21", "max_lon = -97.26")).unwrap();
        let error = validate(&config, &fixture.paths).err().unwrap().to_string();
        assert!(error.contains("max_lon"), "{error}");
    }

    #[test]
    fn non_finite_bbox_is_rejected() {
        let fixture = Fixture::new();
        let config = Config::parse(&VALID.replace("min_lat = 35.35", "min_lat = NaN")).unwrap();
        let error = validate(&config, &fixture.paths).err().unwrap().to_string();
        assert!(error.contains("finite"), "{error}");
    }

    #[test]
    fn out_of_range_forecast_point_is_rejected() {
        let fixture = Fixture::new();
        let config = Config::parse(&VALID.replace("lat = 35.37", "lat = 91")).unwrap();
        let error = validate(&config, &fixture.paths).err().unwrap().to_string();
        assert!(error.contains("[forecast] lat"), "{error}");
    }

    #[test]
    fn unrecognized_station_id_is_rejected() {
        let fixture = Fixture::new();
        let config =
            Config::parse(&VALID.replace("nexrad_id = KTLX", "nexrad_id = KTLX1")).unwrap();
        let error = validate(&config, &fixture.paths).err().unwrap().to_string();
        assert!(error.contains("nexrad_id"), "{error}");
    }

    #[test]
    fn missing_zone_is_rejected() {
        let fixture = Fixture::new();
        let config = Config::parse(&VALID.replace("\nzone = OKZ025", "")).unwrap();
        let error = validate(&config, &fixture.paths).err().unwrap().to_string();
        assert!(error.contains("[nws] zone"), "{error}");
    }

    #[test]
    fn default_user_agent_is_rejected() {
        let fixture = Fixture::new();
        let config =
            Config::parse(&VALID.replace("user_agent = radar-frame (test@example.com)\n", ""))
                .unwrap();
        let error = validate(&config, &fixture.paths).err().unwrap().to_string();
        assert!(error.contains("user_agent"), "{error}");
    }

    #[test]
    fn bad_timezone_is_rejected() {
        let fixture = Fixture::new();
        let config =
            Config::parse(&VALID.replace("timezone = America/Chicago", "timezone = Mars/Olympus"))
                .unwrap();
        let error = validate(&config, &fixture.paths).err().unwrap().to_string();
        assert!(error.contains("time zone"), "{error}");
    }

    #[test]
    fn missing_palette_is_rejected() {
        let fixture = Fixture::new();
        std::fs::remove_file(fixture.paths.palette_img()).unwrap();
        assert!(fixture.error().contains("palette.png"));
    }

    #[test]
    fn palette_must_have_seven_colours() {
        let fixture = Fixture::new();
        write_indexed_png(
            &fixture.paths.palette_img(),
            &[[0, 0, 0], [255, 255, 255], [255, 0, 0]],
        );
        let error = fixture.error();
        assert!(error.contains("3 colours"), "{error}");
    }

    #[test]
    fn missing_data_directory_is_rejected() {
        let fixture = Fixture::new();
        std::fs::remove_dir_all(fixture.paths.data_dir()).unwrap();
        let error = fixture.error();
        assert!(error.contains("data directory"), "{error}");
    }

    fn write_indexed_png(path: &std::path::Path, colors: &[[u8; 3]]) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), colors.len() as u32, 1);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(colors.concat());
        let mut writer = encoder.write_header().unwrap();
        let indices: Vec<u8> = (0..colors.len() as u8).collect();
        writer.write_image_data(&indices).unwrap();
        writer.finish().unwrap();
    }
}
