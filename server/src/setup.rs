//! One-time and per-bbox setup: data directory, e-ink palette, street mask.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::config::Config;
use crate::constants::{RADAR_HEIGHT, WIDTH};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::pngio;
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

/// Write and remove a probe file in `data/`. Used by readiness and the
/// startup summary so a read-only mount is reported before a cycle fails.
pub fn data_writable(paths: &Paths) -> bool {
    static PROBE: AtomicU64 = AtomicU64::new(0);

    let name = format!(
        ".readyz-{}-{}",
        std::process::id(),
        PROBE.fetch_add(1, Ordering::Relaxed)
    );
    let path = paths.data_dir().join(name);
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => {
            drop(file);
            if let Err(error) = fs::remove_file(&path) {
                tracing::warn!("{}: {error}", path.display());
            }
            true
        }
        Err(error) => {
            tracing::debug!("{}: {error}", path.display());
            false
        }
    }
}

/// Write the seven-colour palette PNG. `setup/palette.py` ran the `frame-view
/// palette` subcommand; this calls the same library code.
pub fn create_palette(paths: &Paths) -> Result<()> {
    crate::view::palette::write_eink_png(&paths.palette_img())?;
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

fn read_stamp(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(stamp) => Ok(Some(stamp)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error(format!("{}: {error}", path.display()))),
    }
}

fn write_stamp(path: &Path, stamp: &str) -> Result<()> {
    crate::fsutil::atomic_write(path, stamp.as_bytes())
        .map_err(|e| Error(format!("{}: {e}", path.display())))
}

/// FNV-1a fingerprint of the OSM body. This is corruption detection, not
/// authentication: the mask is regenerated when the body no longer parses.
fn content_hash(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}\n")
}

fn validate_osm(xml: &str) -> Result<()> {
    crate::render::osm::validate_root(xml)?;
    crate::render::osm::parse_polylines(xml)?;
    Ok(())
}

fn validate_mask(path: &Path) -> Result<()> {
    let mask = pngio::read_gray8(path)?;
    if (mask.width, mask.height) != (WIDTH, RADAR_HEIGHT) {
        return Err(Error(format!(
            "{}: street mask is {}x{}, expected {WIDTH}x{RADAR_HEIGHT}",
            path.display(),
            mask.width,
            mask.height
        )));
    }
    Ok(())
}

/// Build the street mask for the configured bbox, fetching the OSM file only
/// when it is missing or was fetched for a different bbox.
///
/// The fetched body is parsed and rasterized to a temporary mask before any
/// artifact is replaced; a malformed response leaves the previous OSM file,
/// mask, and stamps untouched.
pub async fn create_streets(
    paths: &Paths,
    config: &Config,
    fetcher: &dyn OsmFetcher,
    force: bool,
) -> Result<()> {
    let stamp = bbox_stamp(config)?;
    let mask = paths.streets_img();
    let osm = paths.streets_osm();

    let (mask_stamp, mask_stamp_unreadable) = match read_stamp(&paths.streets_bbox()) {
        Ok(stamp) => (stamp, false),
        Err(error) => {
            tracing::warn!(
                event = "streets_stamp",
                error = %error,
                "regenerating the street mask because its [bbox] stamp is unreadable"
            );
            (None, true)
        }
    };

    if mask.exists() && !force {
        match mask_stamp.as_deref() {
            Some(current) if current == stamp.as_str() => match validate_mask(&mask) {
                Ok(()) => {
                    tracing::info!("{} is current for [bbox], doing nothing", mask.display());
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(
                        event = "streets_mask",
                        error = %error,
                        "regenerating the invalid street mask"
                    );
                }
            },
            None if !mask_stamp_unreadable => {
                // Masks generated before the bbox stamp existed: adopt them
                // rather than refetching on every upgrade.
                match validate_mask(&mask) {
                    Ok(()) => {
                        tracing::warn!(
                            "{} has no bbox stamp, assuming it matches the current [bbox]; use --force-streets if the bbox has changed",
                            mask.display()
                        );
                        write_stamp(&paths.streets_bbox(), &stamp)?;
                        return Ok(());
                    }
                    Err(error) => {
                        tracing::warn!(
                            event = "streets_mask",
                            error = %error,
                            "regenerating the invalid unstamped street mask"
                        );
                    }
                }
            }
            _ => {
                tracing::info!(
                    "{} was generated for a different [bbox], regenerating",
                    mask.display()
                );
            }
        }
    }

    let (osm_stamp, osm_stamp_unreadable) = match read_stamp(&paths.streets_osm_bbox()) {
        Ok(stamp) => (stamp, false),
        Err(error) => {
            tracing::warn!(
                event = "streets_stamp",
                error = %error,
                "re-downloading the OSM file because its [bbox] stamp is unreadable"
            );
            (None, true)
        }
    };

    let local_osm = match fs::read_to_string(&osm) {
        Ok(body) => Some(body),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            tracing::warn!(event = "streets_osm", error = %error, "ignoring the unreadable OSM file");
            None
        }
    };
    let local_valid = match local_osm.as_deref() {
        Some(body) => match validate_osm(body) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    event = "streets_osm",
                    path = %osm.display(),
                    error = %error,
                    "ignoring the invalid OSM file"
                );
                false
            }
        },
        None => false,
    };

    // A hand-written OSM file is adopted only on a first run or when it is
    // already stamped for this bbox; both paths require a successful parse.
    let adopt_local = local_valid
        && !force
        && (osm_stamp.as_deref() == Some(stamp.as_str())
            || (osm_stamp.is_none() && !osm_stamp_unreadable && !mask.exists()));

    let body = if adopt_local {
        if osm_stamp.is_none() {
            tracing::warn!(
                "{} has no bbox stamp, assuming it matches the current [bbox]",
                osm.display()
            );
        }
        local_osm.ok_or_else(|| Error(format!("{}: unreadable OSM file", osm.display())))?
    } else {
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
        fetcher.fetch(&query).await?
    };

    let part = paths.streets_img_part();
    let _ = fs::remove_file(&part);
    generate_streets_png(paths, config, &body, &part)?;
    if let Err(error) = validate_mask(&part) {
        let _ = fs::remove_file(&part);
        return Err(error);
    }

    // The mask is validated; only now replace the OSM file and its stamps.
    if !adopt_local {
        crate::fsutil::atomic_write(&osm, body.as_bytes())
            .map_err(|e| Error(format!("{}: {e}", osm.display())))?;
        write_stamp(&paths.streets_osm_bbox(), &stamp)?;
    } else {
        if osm_stamp.is_none() {
            write_stamp(&paths.streets_osm_bbox(), &stamp)?;
        }
    }
    crate::fsutil::atomic_write(
        &paths.streets_osm_hash(),
        content_hash(body.as_bytes()).as_bytes(),
    )
    .map_err(|e| Error(format!("{}: {e}", paths.streets_osm_hash().display())))?;

    fs::rename(&part, &mask).map_err(|error| {
        let _ = fs::remove_file(&part);
        Error(format!("{}: {error}", mask.display()))
    })?;
    write_stamp(&paths.streets_bbox(), &stamp)?;
    Ok(())
}

fn generate_streets_png(paths: &Paths, config: &Config, body: &str, output: &Path) -> Result<()> {
    crate::render::streets::render_streets_from_xml(
        body,
        &crate::render::streets::Options {
            min_lon: config.f64("bbox", "min_lon")?,
            min_lat: config.f64("bbox", "min_lat")?,
            max_lon: config.f64("bbox", "max_lon")?,
            max_lat: config.f64("bbox", "max_lat")?,
            width: WIDTH,
            height: RADAR_HEIGHT,
            input: paths.streets_osm(),
            output: output.to_path_buf(),
        },
    )?;
    Ok(())
}

/// Retry pacing for a failed street-mask build.
#[derive(Clone, Copy, Debug)]
pub struct StreetsRetry {
    pub base: Duration,
    pub max: Duration,
    /// `None` retries until the mask is built.
    pub max_attempts: Option<u32>,
}

impl Default for StreetsRetry {
    fn default() -> Self {
        StreetsRetry {
            base: Duration::from_secs(30),
            max: Duration::from_secs(30 * 60),
            max_attempts: None,
        }
    }
}

/// Delay after `failures` consecutive failed builds, capped at `policy.max`.
pub fn streets_retry_delay(failures: u32, policy: StreetsRetry) -> Duration {
    let exponent = failures.saturating_sub(1).min(20);
    let base_ms = policy.base.as_millis().max(1) as u64;
    let max_ms = policy.max.as_millis().max(1) as u64;
    let millis = base_ms.saturating_mul(1u64 << exponent).min(max_ms);
    Duration::from_millis(millis)
}

/// Build the mask, retrying failures with exponential backoff.
///
/// The caller runs this in the background so a missing or stale mask never
/// blocks server startup or fails a request.
pub async fn retry_streets(
    paths: &Paths,
    config: &Config,
    fetcher: &dyn OsmFetcher,
    policy: StreetsRetry,
) -> Result<()> {
    let mut failures = 0u32;
    loop {
        match create_streets(paths, config, fetcher, false).await {
            Ok(()) => {
                tracing::info!(
                    event = "streets_ready",
                    attempts = failures + 1,
                    "street overlay is current"
                );
                return Ok(());
            }
            Err(error) => {
                failures += 1;
                if policy.max_attempts.is_some_and(|max| failures >= max) {
                    return Err(Error(format!(
                        "street overlay unavailable after {failures} attempts: {error}"
                    )));
                }
                let delay = streets_retry_delay(failures, policy);
                tracing::warn!(
                    event = "streets_retry",
                    attempt = failures,
                    delay_ms = delay.as_millis() as u64,
                    error = %error,
                    "street overlay unavailable; retrying"
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
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

    struct FlakyOsm {
        calls: AtomicUsize,
        failures_before_success: usize,
        body: String,
    }

    #[async_trait]
    impl OsmFetcher for FlakyOsm {
        async fn fetch(&self, _query: &str) -> Result<String> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call <= self.failures_before_success {
                Err(Error::new("overpass unavailable"))
            } else {
                Ok(self.body.clone())
            }
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
        assert_eq!(
            std::fs::read_to_string(paths.streets_osm_hash()).unwrap(),
            content_hash(OSM.as_bytes())
        );
    }

    #[tokio::test]
    async fn an_error_page_leaves_every_artifact_untouched() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        let fetcher = FakeOsm {
            calls: AtomicUsize::new(0),
            body: "<html><body>Overpass is overloaded</body></html>".into(),
        };

        let error = create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("<osm>"), "{error}");
        assert!(!paths.streets_osm().exists());
        assert!(!paths.streets_osm_hash().exists());
        assert!(!paths.streets_img().exists());
        assert!(!paths.streets_img_part().exists());
        assert!(!paths.streets_bbox().exists());
        assert!(!paths.streets_osm_bbox().exists());
    }

    #[tokio::test]
    async fn an_invalid_hand_written_osm_is_not_adopted() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        std::fs::write(paths.streets_osm(), "<html>error</html>").unwrap();
        let fetcher = FakeOsm {
            calls: AtomicUsize::new(0),
            body: OSM.to_string(),
        };

        create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
        assert_eq!(std::fs::read_to_string(paths.streets_osm()).unwrap(), OSM);
        assert!(paths.streets_img().exists());
    }

    #[tokio::test]
    async fn a_failed_forced_fetch_keeps_the_previous_mask() {
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
        let mask = std::fs::read(paths.streets_img()).unwrap();
        let bbox = std::fs::read(paths.streets_bbox()).unwrap();

        let failing = FlakyOsm {
            calls: AtomicUsize::new(0),
            failures_before_success: 1,
            body: OSM.to_string(),
        };
        create_streets(&paths, &config, &failing, true)
            .await
            .unwrap_err();

        assert_eq!(std::fs::read(paths.streets_img()).unwrap(), mask);
        assert_eq!(std::fs::read(paths.streets_bbox()).unwrap(), bbox);
        assert!(validate_mask(&paths.streets_img()).is_ok());
    }

    #[tokio::test]
    async fn an_unreadable_mask_stamp_forces_a_fetch_instead_of_adoption() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        std::fs::write(paths.streets_osm(), OSM).unwrap();
        let adopting = FakeOsm {
            calls: AtomicUsize::new(0),
            body: String::new(),
        };
        create_streets(&paths, &config, &adopting, false)
            .await
            .unwrap();

        std::fs::remove_file(paths.streets_osm()).unwrap();
        std::fs::remove_file(paths.streets_osm_bbox()).unwrap();
        std::fs::remove_file(paths.streets_osm_hash()).unwrap();
        std::fs::write(paths.streets_bbox(), [0xff, 0xfe]).unwrap();

        let fetcher = FakeOsm {
            calls: AtomicUsize::new(0),
            body: OSM.to_string(),
        };
        create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(paths.streets_bbox()).unwrap(),
            bbox_stamp(&config).unwrap()
        );
    }

    #[tokio::test]
    async fn an_unreadable_osm_stamp_forces_a_fetch() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        std::fs::write(paths.streets_osm(), OSM).unwrap();
        std::fs::write(paths.streets_osm_bbox(), [0xff, 0xfe]).unwrap();
        let fetcher = FakeOsm {
            calls: AtomicUsize::new(0),
            body: OSM.to_string(),
        };

        create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(paths.streets_osm_bbox()).unwrap(),
            bbox_stamp(&config).unwrap()
        );
    }

    #[tokio::test]
    async fn a_corrupt_mask_with_a_current_stamp_is_regenerated() {
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
        pngio::write_gray8(&paths.streets_img(), 1, 1, &[1], Some(1)).unwrap();

        create_streets(&paths, &config, &fetcher, false)
            .await
            .unwrap();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 0);
        assert!(validate_mask(&paths.streets_img()).is_ok());
    }

    #[tokio::test]
    async fn retry_rebuilds_the_mask_after_transient_failures() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        let fetcher = FlakyOsm {
            calls: AtomicUsize::new(0),
            failures_before_success: 2,
            body: OSM.to_string(),
        };
        let policy = StreetsRetry {
            base: Duration::from_millis(1),
            max: Duration::from_millis(2),
            max_attempts: Some(5),
        };

        retry_streets(&paths, &config, &fetcher, policy)
            .await
            .unwrap();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 3);
        assert!(validate_mask(&paths.streets_img()).is_ok());
    }

    #[tokio::test]
    async fn retry_gives_up_at_the_attempt_cap() {
        let dir = root();
        let paths = Paths::new(dir.path());
        let config = Config::parse(CONFIG).unwrap();
        let fetcher = FlakyOsm {
            calls: AtomicUsize::new(0),
            failures_before_success: 10,
            body: OSM.to_string(),
        };
        let policy = StreetsRetry {
            base: Duration::from_millis(1),
            max: Duration::from_millis(1),
            max_attempts: Some(3),
        };

        let error = retry_streets(&paths, &config, &fetcher, policy)
            .await
            .unwrap_err();

        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 3);
        assert!(error.to_string().contains("3 attempts"), "{error}");
        assert!(!paths.streets_img().exists());
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
