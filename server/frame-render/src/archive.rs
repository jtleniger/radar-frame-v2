//! Archive II volume -> radar frame PNG in one pass.
//!
//! This is the fused path of unit 2d: it reads the volume with
//! [`nexrad_geojson::archive2::read_volume`], projects each gate with the same
//! [`build_bins`] geometry the GeoJSON path uses, and rasterizes straight into
//! the per-elevation byte grids. The GeoJSON artifacts are only a serialization
//! of those bins; the frame path never writes or reparses them.
//!
//! Coordinates and values are rounded through
//! [`nexrad_geojson::geojson::geojson_coordinate`] /
//! [`nexrad_geojson::geojson::geojson_value`], so the rasterized numbers are
//! exactly the ones a GeoJSON round trip would produce.

use std::path::PathBuf;

use nexrad_geojson::archive2::{read_volume, Product};
use nexrad_geojson::geo::{shortest_f32_degrees, Ortho};
use nexrad_geojson::geojson::{build_bins, geojson_coordinate, geojson_value};

use crate::raster::{self, Transform};
use crate::render;
use crate::{Error, Result};

/// Renderer inputs, one-to-one with the `frame-render frame` flags.
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
    pub colors: PathBuf,
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
        &options.colors,
        &options.streets,
    )?;

    let volume = read_volume(&options.archive)
        .map_err(|e| Error(format!("{}: {e}", options.archive.display())))?;

    // The projection origin comes from the first radial of the first sweep,
    // exactly as `nexrad-geojson` chooses it.
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
        &mut composite,
        &streets,
        &table,
        options.width,
        options.height,
        &options.output,
    )
}

/// One gate as a single-ring polygon, with the GeoJSON precision the serialized
/// path feeds the rasterizer.
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
        let (x, y) = transform.pixel(geojson_coordinate(lon), geojson_coordinate(lat));
        pixel_ring.push([x, y]);
    }

    raster::fill_polygon(
        grid,
        width,
        height,
        std::slice::from_ref(pixel_ring),
        raster::burn_value(geojson_value(value)),
    );
}
