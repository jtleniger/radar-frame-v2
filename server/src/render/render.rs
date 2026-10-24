//! The radar frame pipeline: Archive II gates -> composite grid -> RGBA PNG.

use std::path::Path;

use crate::render::colors::ColorTable;
use crate::render::pngio;
use crate::render::raster::Transform;
use crate::render::{Error, Result};

/// Frame extent and pixel size.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bounds {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
    pub width: usize,
    pub height: usize,
}

/// Read the color table and optional street mask and build the geotransform.
///
/// `None` means the mask has not been built yet: the radar frame is rendered
/// without the overlay rather than failing the cycle. A fully opaque neutral
/// mask passes the colorized radar through unchanged.
pub(crate) fn load_frame(
    bounds: &Bounds,
    colors: Option<&Path>,
    streets_path: Option<&Path>,
) -> Result<(Transform, pngio::Gray8, ColorTable)> {
    let table = ColorTable::load(colors)?;

    let streets = match streets_path {
        Some(streets_path) => {
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
            streets
        }
        None => {
            let pixels = bounds
                .width
                .checked_mul(bounds.height)
                .ok_or_else(|| Error("frame size is too large".into()))?;
            pngio::Gray8 {
                width: bounds.width,
                height: bounds.height,
                samples: vec![255; pixels],
                transparent: None,
            }
        }
    };

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

/// Uncovered pixels stay 0: the old `gdal_rasterize` left them at its init
/// value without declaring a nodata value, so `gdal_calc.py` kept them at 0 and
/// the color table's `0 0 0 0 0` entry (alpha 0) made the gated-out background
/// transparent. Colorize, overlay the street mask, and write the RGBA PNG.
///
/// GDAL 3.10 changed `gdal_rasterize` to declare nodata 0, which made
/// `gdal_calc.py` rewrite every uncovered pixel to its Byte nodata default
/// (255) and `gdaldem` color it like the last entry. The port must not copy
/// that: 255 has no table entry of its own, so it would paint the background
/// opaque `(46, 11, 119)` instead of transparent.
pub(crate) fn write_composite(
    composite: &[u8],
    streets: &pngio::Gray8,
    table: &ColorTable,
    width: usize,
    height: usize,
    output: &Path,
) -> Result<()> {
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

    /// Gated-out and uncovered pixels stay 0, which the shipped table maps to
    /// its `0 0 0 0 0` entry: the background must flatten to black, not to the
    /// opaque dark blue of the table's last entry.
    #[test]
    fn uncovered_pixels_use_the_transparent_entry() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("frame.png");
        let streets = pngio::Gray8 {
            width: 2,
            height: 1,
            samples: vec![0, 0],
            transparent: None,
        };

        write_composite(
            &[0, 40],
            &streets,
            &ColorTable::builtin().unwrap(),
            2,
            1,
            &output,
        )
        .unwrap();

        let (width, height, rgba) = pngio::read_rgba(&output).unwrap();
        assert_eq!((width, height), (2, 1));
        assert_eq!(&rgba[..4], &[0, 0, 0, 255]);
        assert_eq!(&rgba[4..], &[244, 129, 37, 255]);
    }
}
