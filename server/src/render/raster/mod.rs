//! Affine geotransform, burn conversion, and the GDAL polygon scanline fill.
//!
//! Ported from GDAL 3.10.3 `alg/llrasterize.cpp` (`GDALdllImageFilledPolygon`)
//! and `alg/gdaltransformer.cpp` (`GDALInvGeoTransform`), keeping the exact
//! floating point operation order so the resulting byte grids are identical.

use crate::render::{Error, Result};

mod line;
mod polygon;

pub use line::draw_lines;
pub use polygon::{fill_ring, fill_ring_max};

/// Maps WGS84 lon/lat to pixel coordinates, pixel origin at the top-left
/// corner of the top-left pixel (GDAL's inverse geotransform convention).
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    inv0: f64,
    inv1: f64,
    inv3: f64,
    inv5: f64,
}

impl Transform {
    pub fn new(
        min_lon: f64,
        min_lat: f64,
        max_lon: f64,
        max_lat: f64,
        width: usize,
        height: usize,
    ) -> Result<Self> {
        if !min_lon.is_finite()
            || !min_lat.is_finite()
            || !max_lon.is_finite()
            || !max_lat.is_finite()
        {
            return Err(Error(format!(
                "non-finite bbox: {min_lon} {min_lat} {max_lon} {max_lat}"
            )));
        }
        if max_lon <= min_lon {
            return Err(Error(format!(
                "max_lon {max_lon} must be greater than min_lon {min_lon}"
            )));
        }
        if max_lat <= min_lat {
            return Err(Error(format!(
                "max_lat {max_lat} must be greater than min_lat {min_lat}"
            )));
        }
        if width == 0 || height == 0 {
            return Err(Error(format!("frame size {width}x{height} is empty")));
        }

        // GDAL geotransform: gt0 = min_lon, gt1 = width of a pixel,
        // gt2 = 0, gt3 = max_lat, gt4 = 0, gt5 = -height of a pixel.
        let gt1 = (max_lon - min_lon) / width as f64;
        let gt5 = -(max_lat - min_lat) / height as f64;

        Ok(Transform {
            inv0: -min_lon / gt1,
            inv1: 1.0 / gt1,
            inv3: -max_lat / gt5,
            inv5: 1.0 / gt5,
        })
    }

    /// Pixel coordinates of a lon/lat point.
    pub fn pixel(&self, lon: f64, lat: f64) -> (f64, f64) {
        (self.inv0 + lon * self.inv1, self.inv3 + lat * self.inv5)
    }
}

/// `GDALCopyWord(double -> GByte)`: NaN becomes 0, otherwise round half up
/// and clamp.
pub fn burn_value(value: f64) -> u8 {
    if value.is_nan() {
        return 0;
    }
    (value + 0.5).clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_maps_corners_to_pixel_edges() {
        let t = Transform::new(0.0, 0.0, 8.0, 4.0, 8, 4).unwrap();
        assert_eq!(t.pixel(0.0, 4.0), (0.0, 0.0));
        assert_eq!(t.pixel(8.0, 0.0), (8.0, 4.0));
        assert_eq!(t.pixel(4.0, 2.0), (4.0, 2.0));
    }

    #[test]
    fn transform_rejects_degenerate_input() {
        assert!(Transform::new(0.0, 0.0, 0.0, 4.0, 8, 4).is_err());
        assert!(Transform::new(0.0, 0.0, 8.0, 0.0, 8, 4).is_err());
        assert!(Transform::new(f64::NAN, 0.0, 8.0, 4.0, 8, 4).is_err());
        assert!(Transform::new(0.0, 0.0, 8.0, 4.0, 0, 4).is_err());
    }

    #[test]
    fn fills_interior_scanlines_only() {
        let (width, height) = (8, 8);
        let mut grid = vec![0u8; width * height];
        let ring = [(2.0, 2.0), (2.0, 5.0), (6.0, 5.0), (6.0, 2.0)];
        fill_ring(&mut grid, width, height, &ring, 7);

        for y in 2..=4 {
            for x in 0..width {
                let expected = if (2..=5).contains(&x) { 7 } else { 0 };
                assert_eq!(grid[y * width + x], expected, "pixel ({x},{y})");
            }
        }
        // The bottom edge sits at y = 5, whose pixel center is 5.5.
        for x in 0..width {
            assert_eq!(grid[5 * width + x], 0, "row 5 pixel {x}");
        }
    }

    #[test]
    fn clips_to_the_grid_without_panicking() {
        let (width, height) = (8, 8);
        let mut grid = vec![0u8; width * height];
        let ring = [(-2.0, -2.0), (-2.0, 1.0), (2.0, 1.0), (2.0, -2.0)];
        fill_ring(&mut grid, width, height, &ring, 9);

        for (x, &value) in grid[..width].iter().enumerate() {
            let expected = if x <= 1 { 9 } else { 0 };
            assert_eq!(value, expected, "row 0 pixel {x}");
        }
        for y in 1..height {
            for x in 0..width {
                assert_eq!(grid[y * width + x], 0, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn degenerate_ring_is_a_no_op() {
        let (width, height) = (4, 4);
        let mut grid = vec![0u8; width * height];
        fill_ring(&mut grid, width, height, &[(1.0, 1.0); 4], 5);
        assert!(grid.iter().all(|&v| v == 0));
    }

    #[test]
    fn max_fill_keeps_higher_existing_values() {
        let (width, height) = (4, 1);
        let mut grid = vec![10u8, 40, 0, 30];
        fill_ring_max(
            &mut grid,
            width,
            height,
            &[(0.0, 0.0), (0.0, 1.0), (3.0, 1.0), (3.0, 0.0)],
            20,
        );
        assert_eq!(grid, vec![20, 40, 20, 30]);
    }

    #[test]
    fn burn_conversion_matches_gdal_copy_word() {
        assert_eq!(burn_value(10.5), 11);
        assert_eq!(burn_value(12.5), 13);
        assert_eq!(burn_value(41.4), 41);
        assert_eq!(burn_value(-0.4), 0);
        assert_eq!(burn_value(254.6), 255);
        assert_eq!(burn_value(300.0), 255);
        assert_eq!(burn_value(f64::NAN), 0);
    }

    #[test]
    fn draws_horizontal_line_without_duplicating_shared_endpoints() {
        let (width, height) = (8, 4);
        let mut grid = vec![0u8; width * height];
        // Pixel centers are at integer + 0.5; y = 1.5 maps to row 1.
        draw_lines(
            &mut grid,
            width,
            height,
            &[vec![[0.5, 1.5], [3.5, 1.5], [6.5, 1.5]]],
            255,
        );
        for x in 0..width {
            let expected = if (0..=6).contains(&x) { 255 } else { 0 };
            assert_eq!(grid[width + x], expected, "row 1 pixel {x}");
        }
        assert!(grid[..width].iter().all(|&v| v == 0));
        assert!(grid[2 * width..].iter().all(|&v| v == 0));
    }

    #[test]
    fn draws_vertical_line() {
        let (width, height) = (4, 8);
        let mut grid = vec![0u8; width * height];
        draw_lines(
            &mut grid,
            width,
            height,
            &[vec![[1.5, 0.5], [1.5, 5.5]]],
            255,
        );
        for y in 0..height {
            let expected = if (0..=5).contains(&y) { 255 } else { 0 };
            assert_eq!(grid[y * width + 1], expected, "column 1 pixel {y}");
        }
    }

    #[test]
    fn draws_diagonal_line() {
        let (width, height) = (8, 8);
        let mut grid = vec![0u8; width * height];
        draw_lines(
            &mut grid,
            width,
            height,
            &[vec![[0.5, 0.5], [5.5, 4.5]]],
            255,
        );
        let lit: Vec<(usize, usize)> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&(x, y)| grid[y * width + x] == 255)
            .collect();
        assert_eq!(lit, vec![(0, 0), (1, 1), (2, 2), (3, 2), (4, 3), (5, 4)]);
    }

    #[test]
    fn clips_lines_to_the_grid() {
        let (width, height) = (4, 4);
        let mut grid = vec![0u8; width * height];
        draw_lines(
            &mut grid,
            width,
            height,
            &[vec![[-2.0, 2.5], [6.0, 2.5]]],
            255,
        );
        assert_eq!(&grid[2 * width..3 * width], &[255, 255, 255, 255]);
    }
}
