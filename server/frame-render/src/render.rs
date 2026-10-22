//! The radar frame pipeline: GeoJSON or Archive II gates -> composite grid ->
//! RGBA PNG.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::colors::ColorTable;
use crate::pngio;
use crate::raster::{self, Transform};
use crate::{Error, Result};

/// `gdal_calc.py` defaults an undefined output nodata to the Byte maximum, and
/// the rasterized gates carry nodata 0, so every pixel no elevation covers ends
/// up as 255 (which the color table then maps its last entry).
const COMPOSITE_NODATA: u8 = 255;

/// Renderer inputs, one-to-one with the `frame-render radar` flags.
#[derive(Debug, Clone)]
pub struct Options {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
    pub width: usize,
    pub height: usize,
    /// Base path of `{input}-REF-{1,2,3}.json`.
    pub input: PathBuf,
    pub colors: PathBuf,
    pub streets: PathBuf,
    pub output: PathBuf,
}

#[derive(Deserialize)]
struct FeatureCollection {
    features: Vec<Feature>,
}

#[derive(Deserialize)]
struct Feature {
    geometry: Geometry,
    properties: Properties,
}

#[derive(Deserialize)]
struct Geometry {
    /// Polygon rings; GDAL rasterizes them with an even-odd rule, so the ring
    /// orientation and interior rings need no special handling.
    coordinates: Vec<Vec<[f64; 2]>>,
}

#[derive(Deserialize)]
struct Properties {
    value: f64,
}

/// Rasterize elevations 1-3, max-composite them, colorize, overlay streets.
pub fn render_radar(options: &Options) -> Result<()> {
    let (transform, streets, table) = load_frame(
        &Bounds {
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

    let mut composite = vec![0u8; options.width * options.height];
    for elevation in 1..=3 {
        let path = elevation_path(&options.input, elevation);
        let grid = rasterize_file(&path, &transform, options.width, options.height)?;
        merge(&mut composite, &grid);
    }

    write_composite(
        &mut composite,
        &streets,
        &table,
        options.width,
        options.height,
        &options.output,
    )
}

fn elevation_path(base: &Path, elevation: u8) -> PathBuf {
    let mut path = base.as_os_str().to_os_string();
    path.push(format!("-REF-{elevation}.json"));
    PathBuf::from(path)
}

fn rasterize_file(
    path: &Path,
    transform: &Transform,
    width: usize,
    height: usize,
) -> Result<Vec<u8>> {
    let text =
        std::fs::read_to_string(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    let collection: FeatureCollection =
        serde_json::from_str(&text).map_err(|e| Error(format!("{}: {e}", path.display())))?;

    let mut grid = vec![0u8; width * height];
    for feature in &collection.features {
        let parts: Vec<Vec<[f64; 2]>> = feature
            .geometry
            .coordinates
            .iter()
            .map(|ring| {
                ring.iter()
                    .map(|point| {
                        let (x, y) = transform.pixel(point[0], point[1]);
                        [x, y]
                    })
                    .collect()
            })
            .collect();
        raster::fill_polygon(
            &mut grid,
            width,
            height,
            &parts,
            raster::burn_value(feature.properties.value),
        );
    }
    Ok(grid)
}

/// Frame extent and pixel size, shared by both entry points.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bounds {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
    pub width: usize,
    pub height: usize,
}

/// Read the color table and street mask and build the geotransform; shared by
/// the GeoJSON (`radar`) and Archive II (`frame`) paths.
pub(crate) fn load_frame(
    bounds: &Bounds,
    colors: &Path,
    streets_path: &Path,
) -> Result<(Transform, pngio::Gray8, ColorTable)> {
    let colors_text =
        std::fs::read_to_string(colors).map_err(|e| Error(format!("{}: {e}", colors.display())))?;
    let table =
        ColorTable::parse(&colors_text).map_err(|e| Error(format!("{}: {e}", colors.display())))?;

    let streets = pngio::read_gray8(streets_path)?;
    if (streets.width, streets.height) != (bounds.width, bounds.height) {
        return Err(Error(format!(
            "{}: street mask is {}x{}, frame is {}x{}",
            streets_path.display(),
            streets.width,
            streets.height,
            bounds.width,
            bounds.height
        )));
    }

    let transform = Transform::new(
        bounds.min_lon,
        bounds.min_lat,
        bounds.max_lon,
        bounds.max_lat,
        bounds.width,
        bounds.height,
    )?;

    Ok((transform, streets, table))
}

/// `gdal_calc.py`'s `numpy.max((A, B, C), axis=0)` over the elevation grids.
pub(crate) fn merge(composite: &mut [u8], grid: &[u8]) {
    for (slot, value) in composite.iter_mut().zip(grid) {
        *slot = (*slot).max(*value);
    }
}

/// Uncovered pixels become the composite nodata, then colorize, overlay the
/// street mask, and write the RGBA PNG.
pub(crate) fn write_composite(
    composite: &mut [u8],
    streets: &pngio::Gray8,
    table: &ColorTable,
    width: usize,
    height: usize,
    output: &Path,
) -> Result<()> {
    for value in composite.iter_mut() {
        if *value == 0 {
            *value = COMPOSITE_NODATA;
        }
    }

    let rgba = compose(composite, streets, &table.lut());
    pngio::write_rgba(output, width, height, &rgba)
}

/// Pixels the way the old PIL sequence produced them: the colorized radar
/// pasted onto an opaque black RGBA image, then the street mask pasted over
/// that, both with Pillow's `(src * alpha + dst * (255 - alpha) + 127) / 255`.
fn compose(composite: &[u8], streets: &pngio::Gray8, lut: &[[u8; 4]; 256]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(composite.len() * 4);
    for (index, &value) in composite.iter().enumerate() {
        let [red, green, blue, alpha] = lut[usize::from(value)];
        let mask = streets.samples[index];
        let source_alpha = match streets.transparent {
            Some(transparent) if mask == transparent => 0,
            _ => 255,
        };
        rgba.push(blend(over_black(red, alpha), mask, mask));
        rgba.push(blend(over_black(green, alpha), mask, mask));
        rgba.push(blend(over_black(blue, alpha), mask, mask));
        rgba.push(blend(255, mask, source_alpha));
    }
    rgba
}

/// Colorized radar over opaque black: Pillow pastes the radar image with its
/// own alpha as the mask.
fn over_black(channel: u8, alpha: u8) -> u8 {
    blend(0, alpha, channel)
}

/// Pillow `Image.paste` blend for one channel: `src` over `dst` with `alpha`.
fn blend(dst: u8, alpha: u8, src: u8) -> u8 {
    let numerator = u32::from(src) * u32::from(alpha) + u32::from(dst) * (255 - u32::from(alpha));
    ((numerator + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_alpha_radar_is_black_and_opaque_alpha_keeps_the_color() {
        assert_eq!(over_black(200, 0), 0);
        assert_eq!(over_black(200, 255), 200);
    }

    #[test]
    fn blend_rounds_like_pillow() {
        // Pillow: paste an L sample m with itself as mask.
        assert_eq!(blend(200, 1, 1), 199);
        assert_eq!(blend(100, 1, 1), 100);
        assert_eq!(blend(46, 128, 128), 87);
        assert_eq!(blend(255, 1, 1), 254);
        assert_eq!(blend(17, 255, 255), 255);
        assert_eq!(blend(17, 0, 0), 17);
    }

    #[test]
    fn merge_keeps_the_highest_elevation_value() {
        let mut composite = vec![0, 20, 10, 30];
        merge(&mut composite, &[41, 10, 30, 30]);
        assert_eq!(composite, vec![41, 20, 30, 30]);
    }
}
