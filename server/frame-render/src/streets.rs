//! OSM street mask rasterization.
//!
//! Replaces the `gdal_rasterize` / `gdal_translate` sequence in
//! `server/setup/streets.py`: parse Overpass OSM XML, rasterize highway
//! polylines into an 8-bit grayscale mask, and write a PNG with the same
//! `tRNS` nodata level the radar renderer already honours.

use std::path::PathBuf;

use crate::osm;
use crate::pngio;
use crate::raster::{self, Transform};
use crate::{Error, Result};

/// Renderer inputs, one-to-one with the `frame-render streets` flags.
#[derive(Debug, Clone)]
pub struct Options {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
    pub width: usize,
    pub height: usize,
    /// Overpass OSM XML file.
    pub input: PathBuf,
    /// Output grayscale PNG path.
    pub output: PathBuf,
}

/// GDAL-compatible defaults used by `setup/streets.py`.
const BURN_VALUE: u8 = 255;
const NODATA_VALUE: u8 = 1;

/// Rasterize OSM highway ways into an 8-bit grayscale street mask PNG.
pub fn render_streets(options: &Options) -> Result<()> {
    let xml = std::fs::read_to_string(&options.input)
        .map_err(|e| Error(format!("{}: {e}", options.input.display())))?;
    let parts = osm::parse_polylines(&xml)?;

    let transform = Transform::new(
        options.min_lon,
        options.min_lat,
        options.max_lon,
        options.max_lat,
        options.width,
        options.height,
    )?;

    // GDAL's gdal_rasterize initializes the raster to the nodata value when
    // `-a_nodata` is supplied, then burns lines with the burn value.
    let mut grid = vec![NODATA_VALUE; options.width * options.height];
    for part in &parts {
        let pixels: Vec<[f64; 2]> = part
            .iter()
            .map(|point| {
                let (x, y) = transform.pixel(point[0], point[1]);
                [x, y]
            })
            .collect();
        raster::draw_lines(
            &mut grid,
            options.width,
            options.height,
            &[pixels],
            BURN_VALUE,
        );
    }

    pngio::write_gray8(
        &options.output,
        options.width,
        options.height,
        &grid,
        Some(NODATA_VALUE),
    )
}
