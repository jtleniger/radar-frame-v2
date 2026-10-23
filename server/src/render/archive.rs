//! Archive II volume -> radar frame PNG in one pass.
//!
//! Reads the volume with [`crate::nexrad::archive2::read_volume`], projects
//! each gate with [`build_bins`], and rasterizes straight into the
//! per-elevation byte grids. This is the only radar renderer.
//!
//! Coordinates and values are rounded through
//! [`crate::nexrad::bins::round_coordinate`] /
//! [`crate::nexrad::bins::round_value`], preserving the precision of the
//! pre-fusion two-step output.

use std::path::PathBuf;

use crate::nexrad::archive2::{read_volume, Product};
use crate::nexrad::bins::{build_bins, round_coordinate, round_value};
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
    /// Elevation numbers to composite, ascending.
    pub elevations: Vec<u8>,
    pub minimum: Option<f32>,
    pub maximum: Option<f32>,
    pub colors: Option<PathBuf>,
    pub streets: PathBuf,
    pub output: PathBuf,
}

/// Rasterize the selected elevations straight from the volume, max-composite
/// them, colorize, overlay streets.
pub fn render_archive(options: &Options) -> Result<()> {
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
        &options.streets,
    )?;

    let volume = read_volume(&options.archive)
        .map_err(|e| Error(format!("{}: {e}", options.archive.display())))?;

    // The projection origin comes from the first radial of the first sweep,
    // exactly as the pre-fusion converter chose it.
    let first = volume
        .scans
        .get(&1)
        .and_then(|scan| scan.first())
        .ok_or_else(|| {
            Error(format!(
                "{}: elevation 1 not present",
                options.archive.display()
            ))
        })?;
    let ortho = Ortho::new(
        shortest_f32_degrees(first.volume.lat),
        shortest_f32_degrees(first.volume.lon),
    );

    let mut composite = vec![0u8; options.width * options.height];
    let mut pixel_ring = Vec::with_capacity(5);
    for &elevation in &options.elevations {
        let scan = volume.scans.get(&elevation).ok_or_else(|| {
            Error(format!(
                "{}: elevation {elevation} not present",
                options.archive.display()
            ))
        })?;

        let mut grid = vec![0u8; options.width * options.height];
        for radial in scan {
            let bins = build_bins(
                radial,
                Product::Ref,
                &ortho,
                options.minimum,
                options.maximum,
            )
            .map_err(|e| Error(format!("{}: {e}", options.archive.display())))?;
            for bin in bins {
                rasterize_bin(
                    &mut grid,
                    &transform,
                    options.width,
                    options.height,
                    &mut pixel_ring,
                    &bin.ring,
                    bin.value,
                );
            }
        }
        render::merge(&mut composite, &grid);
    }

    render::write_composite(
        &composite,
        &streets,
        &table,
        options.width,
        options.height,
        &options.output,
    )
}

/// One gate as a single-ring polygon, with the precision of the pre-fusion
/// serialized output.
fn rasterize_bin(
    grid: &mut [u8],
    transform: &Transform,
    width: usize,
    height: usize,
    pixel_ring: &mut Vec<[f64; 2]>,
    ring: &[(f64, f64); 5],
    value: f32,
) {
    pixel_ring.clear();
    for &(lon, lat) in ring {
        let (x, y) = transform.pixel(round_coordinate(lon), round_coordinate(lat));
        pixel_ring.push([x, y]);
    }

    raster::fill_polygon(
        grid,
        width,
        height,
        std::slice::from_ref(pixel_ring),
        raster::burn_value(round_value(value)),
    );
}
