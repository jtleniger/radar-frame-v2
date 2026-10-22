//! One-time and per-bbox setup: data directory, e-ink palette, street mask.

use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::constants::{RADAR_HEIGHT, WIDTH};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::sources::osm::{self, OsmFetcher};

pub fn create_data_dir(paths: &Paths) -> Result<()> {
    let data = paths.data_dir();
    if data.is_dir() {
        tracing::info!("{} exists", data.display());
        return Ok(());
    }

    tracing::info!("{} missing, creating it", data.display());
    fs::create_dir(&data).map_err(|e| Error(format!("{}: {e}", data.display())))
}

/// Write the seven-colour palette PNG. `setup/palette.py` ran the `frame-view
/// palette` subcommand; this calls the same library code.
pub fn create_palette(paths: &Paths) -> Result<()> {
    frame_view::palette::write_eink_png(&paths.palette_img())?;
    Ok(())
}

pub fn bbox_stamp(config: &Config) -> Result<String> {
    let mut stamp = String::new();
    for key in ["min_lat", "max_lat", "min_lon", "max_lon"] {
        stamp.push_str(key);
        stamp.push('=');
        stamp.push_str(config.get("bbox", key)?);
        stamp.push('\n');
    }
    Ok(stamp)
}

fn read_stamp(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn write_stamp(path: &Path, stamp: &str) -> Result<()> {
    fs::write(path, stamp).map_err(|e| Error(format!("{}: {e}", path.display())))
}

/// Build the street mask for the configured bbox, fetching the OSM file only
/// when it is missing or was fetched for a different bbox.
pub async fn create_streets(
    paths: &Paths,
    config: &Config,
    fetcher: &dyn OsmFetcher,
    force: bool,
) -> Result<()> {
    let stamp = bbox_stamp(config)?;
    let mask = paths.streets_img();
    let osm = paths.streets_osm();

    let mask_stamp = read_stamp(&paths.streets_bbox());
    let mut data_stamp = read_stamp(&paths.streets_osm_bbox());

    if mask.exists() && !force {
        match mask_stamp.as_deref() {
            Some(current) if current == stamp => {
                tracing::info!("{} is current for [bbox], doing nothing", mask.display());
                return Ok(());
            }
            None => {
                // Masks generated before the bbox stamp existed: adopt them
                // rather than refetching on every upgrade.
                tracing::warn!(
                    "{} has no bbox stamp, assuming it matches the current [bbox]; use --force-streets if the bbox has changed",
                    mask.display()
                );
                write_stamp(&paths.streets_bbox(), &stamp)?;
                return Ok(());
            }
            Some(_) => {
                tracing::info!(
                    "{} was generated for a different [bbox], regenerating",
                    mask.display()
                );
            }
        }
    }

    if osm.exists() && data_stamp.is_none() && !mask.exists() {
        // First run with a hand-written streets.osm: use it for the current
        // bbox instead of overwriting it.
        tracing::warn!(
            "{} has no bbox stamp, assuming it matches the current [bbox]",
            osm.display()
        );
        write_stamp(&paths.streets_osm_bbox(), &stamp)?;
        data_stamp = Some(stamp.clone());
    }

    if force || !osm.exists() || data_stamp.as_deref() != Some(stamp.as_str()) {
        tracing::info!(
            "{} missing or generated for a different [bbox], downloading",
            osm.display()
        );
        let query = osm::query(
            config.get("bbox", "min_lat")?,
            config.get("bbox", "min_lon")?,
            config.get("bbox", "max_lat")?,
            config.get("bbox", "max_lon")?,
        );
        let body = fetcher.fetch(&query).await?;
        fs::write(&osm, body).map_err(|e| Error(format!("{}: {e}", osm.display())))?;
        write_stamp(&paths.streets_osm_bbox(), &stamp)?;
    }

    generate_streets_png(paths, config)?;
    write_stamp(&paths.streets_bbox(), &stamp)?;
    Ok(())
}

fn generate_streets_png(paths: &Paths, config: &Config) -> Result<()> {
    frame_render::streets::render_streets(&frame_render::streets::Options {
        min_lon: config.f64("bbox", "min_lon")?,
        min_lat: config.f64("bbox", "min_lat")?,
        max_lon: config.f64("bbox", "max_lon")?,
        max_lat: config.f64("bbox", "max_lat")?,
        width: WIDTH,
        height: RADAR_HEIGHT,
        input: paths.streets_osm(),
        output: paths.streets_img(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CONFIG: &str = "\
[bbox]
min_lat = 35.35
max_lat = 35.39
min_lon = -97.26
max_lon = -97.21
";

    const OSM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<osm version="0.6">
  <node id="1" lat="35.355" lon="-97.245"/>
  <node id="2" lat="35.385" lon="-97.225"/>
  <way id="10"><nd ref="1"/><nd ref="2"/><tag k="highway" v="primary"/></way>
</osm>
"#;

    struct FakeOsm {
        calls: AtomicUsize,
        body: String,
    }

    #[async_trait]
    impl OsmFetcher for FakeOsm {
        async fn fetch(&self, _query: &str) -> Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.body.clone())
        }
    }

    fn root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        dir
    }

    #[tokio::test]
    async fn adopts_hand_written_osm_and_stamps_both_files() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        std::fs::write(paths.streets_osm(), OSM).unwrap();
        let fetcher = FakeOsm {
            calls: AtomicUsize::new(0),
            body: String::new(),
        };

        create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 0);
        assert!(paths.streets_img().exists());
        assert_eq!(
            std::fs::read_to_string(paths.streets_bbox()).unwrap(),
            bbox_stamp(&config).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(paths.streets_osm_bbox()).unwrap(),
            bbox_stamp(&config).unwrap()
        );
    }

    #[tokio::test]
    async fn current_mask_is_a_no_op() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        std::fs::write(paths.streets_osm(), OSM).unwrap();
        let fetcher = FakeOsm {
            calls: AtomicUsize::new(0),
            body: String::new(),
        };

        create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap();
        let mask = std::fs::read(paths.streets_img()).unwrap();
        create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(paths.streets_img()).unwrap(), mask);
    }

    #[tokio::test]
    async fn force_refetches_even_when_stamps_match() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        std::fs::write(paths.streets_osm(), OSM).unwrap();
        let fetcher = FakeOsm {
            calls: AtomicUsize::new(0),
            body: OSM.to_string(),
        };

        create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap();
        create_streets(&paths, &config, &fetcher, true)
            .await
            .unwrap();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_different_bbox_refetches_the_osm() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        std::fs::write(paths.streets_osm(), OSM).unwrap();
        let fetcher = FakeOsm {
            calls: AtomicUsize::new(0),
            body: OSM.to_string(),
        };

        create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap();

        let moved = Config::parse(&CONFIG.replace("min_lon = -97.26", "min_lon = -97.30")).unwrap();
        create_streets(&paths, &moved, &fetcher, false)
            .await
            .unwrap();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
        assert!(std::fs::read_to_string(paths.streets_bbox())
            .unwrap()
            .contains("min_lon=-97.30"));
    }
}
