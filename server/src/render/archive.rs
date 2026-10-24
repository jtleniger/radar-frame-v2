//! Archive II volume -> radar frame PNG in one pass.
//!
//! Reads the volume with [`crate::nexrad::archive2::read_volume`], projects
//! each gate with [`for_each_bin`], and max-composites straight into the
//! output grid. This is the only radar renderer.
//!
//! Coordinates and values are rounded through
//! [`crate::nexrad::bins::round_coordinate`] /
//! [`crate::nexrad::bins::round_value`], preserving the precision of the
//! pre-fusion two-step output.

use std::path::PathBuf;

use crate::nexrad::archive2::{read_volume, Product, Radial, Volume};
use crate::nexrad::bins::{for_each_bin, round_coordinate, round_value};
use crate::nexrad::geo::{shortest_f32_degrees, Ortho};

use crate::render::raster::{self, Transform};
use crate::render::render;
use crate::render::{Error, Result};

/// Renderer inputs, one-to-one with the `radar-frame render archive` flags.
#[derive(Debug, Clone)]
pub struct Options {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
    pub width: usize,
    pub height: usize,
    /// NEXRAD Archive II volume, plain or gzip-compressed.
    pub archive: PathBuf,
    /// Elevation numbers to composite, ascending. Missing elevations are
    /// skipped; at least one must be present.
    pub elevations: Vec<u8>,
    pub minimum: Option<f32>,
    pub maximum: Option<f32>,
    pub colors: Option<PathBuf>,
    /// Street mask to overlay; `None` renders the radar frame alone.
    pub streets: Option<PathBuf>,
    pub output: PathBuf,
}

/// What the renderer actually composited, for the cycle provenance log.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Summary {
    pub elevations: Vec<u8>,
    pub radials: usize,
    pub gates: usize,
}

/// Rasterize the selected elevations straight from the volume, max-composite
/// them, colorize, overlay streets.
pub fn render_archive(options: &Options) -> Result<Summary> {
    let (transform, streets, table) = render::load_frame(
        &render::Bounds {
            min_lon: options.min_lon,
            min_lat: options.min_lat,
            max_lon: options.max_lon,
            max_lat: options.max_lat,
            width: options.width,
            height: options.height,
        },
        options.colors.as_deref(),
        options.streets.as_deref(),
    )?;

    let volume = read_volume(&options.archive)
        .map_err(|e| Error(format!("{}: {e}", options.archive.display())))?;

    // The projection origin comes from the first radial of the first scan that
    // has one; the earliest elevation is not always present in a volume.
    let first = origin_radial(&volume)
        .ok_or_else(|| Error(format!("{}: no radials present", options.archive.display())))?;
    let ortho = Ortho::new(
        shortest_f32_degrees(first.volume.lat),
        shortest_f32_degrees(first.volume.lon),
    );

    let mut composite = vec![0u8; options.width * options.height];
    let mut summary = Summary::default();
    for &elevation in &options.elevations {
        let Some(scan) = volume.scans.get(&elevation) else {
            tracing::warn!(
                archive = %options.archive.display(),
                elevation,
                "elevation not present; skipping"
            );
            continue;
        };

        summary.elevations.push(elevation);
        summary.radials += scan.len();

        for radial in scan {
            summary.gates += for_each_bin(
                &volume,
                radial,
                Product::Ref,
                &ortho,
                options.minimum,
                options.maximum,
                |ring, value| {
                    rasterize_bin(
                        &mut composite,
                        &transform,
                        options.width,
                        options.height,
                        ring,
                        value,
                    );
                },
            )
            .map_err(|e| Error(format!("{}: {e}", options.archive.display())))?;
        }
    }

    if summary.elevations.is_empty() {
        let available: Vec<u8> = volume.scans.keys().copied().collect();
        return Err(Error(format!(
            "{}: none of the requested elevations {:?} are present (available: {:?})",
            options.archive.display(),
            options.elevations,
            available
        )));
    }

    render::write_composite(
        &composite,
        &streets,
        &table,
        options.width,
        options.height,
        &options.output,
    )?;
    Ok(summary)
}

/// The radial that anchors the projection: the lowest elevation scan that
/// actually contains radials.
fn origin_radial(volume: &Volume) -> Option<&Radial> {
    volume.scans.values().find_map(|scan| scan.first())
}

/// One gate as an open four-corner ring, at the precision of the pre-fusion
/// serialized output.
fn rasterize_bin(
    composite: &mut [u8],
    transform: &Transform,
    width: usize,
    height: usize,
    ring: &[(f64, f64); 4],
    value: f32,
) {
    let mut pixel_ring = [(0.0, 0.0); 4];
    for (point, &(lon, lat)) in pixel_ring.iter_mut().zip(ring) {
        *point = transform.pixel(round_coordinate(lon), round_coordinate(lat));
    }

    raster::fill_ring_max(
        composite,
        width,
        height,
        &pixel_ring,
        raster::burn_value(round_value(value)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nexrad::archive2::{Moments, RadialHeader, VolumeBlock};
    use std::collections::BTreeMap;

    fn radial(lat: f32, lon: f32, elevation: u8) -> Radial {
        Radial {
            header: RadialHeader {
                azimuth_deg: 0.0,
                elevation_deg: 0.5,
                elevation_number: elevation,
                azimuth_resolution_deg: 1.0,
            },
            volume: VolumeBlock { lat, lon },
            moments: Moments::default(),
        }
    }

    #[test]
    fn origin_uses_the_first_scan_with_radials() {
        let volume = Volume::from_payload(
            BTreeMap::from([(1, Vec::new()), (2, vec![radial(35.5, -97.5, 2)])]),
            Vec::new(),
        );

        let origin = origin_radial(&volume).expect("elevation 2 anchors the frame");
        assert_eq!(origin.volume.lat, 35.5);
        assert_eq!(origin.volume.lon, -97.5);
    }

    #[test]
    fn no_radials_has_no_origin() {
        let volume = Volume::from_payload(BTreeMap::new(), Vec::new());
        assert!(origin_radial(&volume).is_none());
    }
}
