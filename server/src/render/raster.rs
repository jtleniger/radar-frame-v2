//! Affine geotransform, burn conversion, and the GDAL polygon scanline fill.
//!
//! Ported from GDAL 3.10.3 `alg/llrasterize.cpp` (`GDALdllImageFilledPolygon`)
//! and `alg/gdaltransformer.cpp` (`GDALInvGeoTransform`), keeping the exact
//! floating point operation order so the resulting byte grids are identical.

use crate::render::{Error, Result};

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

/// How a burned pixel is written.
#[derive(Clone, Copy)]
enum Burn {
    Set,
    Max,
}

/// `GDALdllImageFilledPolygon` for one open four-corner ring: even-odd
/// scanline fill with the caller's value.
///
/// The ring is implicitly closed, so callers pass four corners rather than
/// repeating the first one.
pub fn fill_ring(grid: &mut [u8], width: usize, height: usize, ring: &[(f64, f64); 4], value: u8) {
    fill_ring_with(grid, width, height, ring, value, Burn::Set);
}

/// Same fill as [`fill_ring`], but keeps the higher value already present.
pub fn fill_ring_max(
    grid: &mut [u8],
    width: usize,
    height: usize,
    ring: &[(f64, f64); 4],
    value: u8,
) {
    fill_ring_with(grid, width, height, ring, value, Burn::Max);
}

fn fill_ring_with(
    grid: &mut [u8],
    width: usize,
    height: usize,
    ring: &[(f64, f64); 4],
    value: u8,
    burn: Burn,
) {
    if width == 0 || height == 0 || grid.len() != width * height {
        return;
    }

    let mut xs = [0.0f64; 4];
    let mut ys = [0.0f64; 4];
    for (index, &(x, y)) in ring.iter().enumerate() {
        xs[index] = x;
        ys[index] = y;
    }

    let mut dminy = ys[0];
    let mut dmaxy = ys[0];
    for &y in &ys[1..] {
        if y < dminy {
            dminy = y;
        }
        if y > dmaxy {
            dmaxy = y;
        }
    }

    // C casts truncate toward zero; saturated casts keep infinities harmless.
    let miny = (dminy as i64).max(0);
    let maxy = (dmaxy as i64).min(height as i64 - 1);
    let maxx = width as i64 - 1;

    // A quad crosses one scanline at most four times.
    let mut intersects = [0.0f64; 4];

    for y in miny..=maxy {
        let dy = y as f64 + 0.5;
        let row = y as usize * width;

        let mut count = 0usize;
        for i in 0..4 {
            let (ind1, ind2) = if i == 0 { (3, 0) } else { (i - 1, i) };

            let (mut dy1, mut dy2) = (ys[ind1], ys[ind2]);
            if (dy1 < dy && dy2 < dy) || (dy1 > dy && dy2 > dy) {
                continue;
            }

            let (dx1, dx2);
            if dy1 < dy2 {
                dx1 = xs[ind1];
                dx2 = xs[ind2];
            } else if dy1 > dy2 {
                std::mem::swap(&mut dy1, &mut dy2);
                dx2 = xs[ind1];
                dx1 = xs[ind2];
            } else {
                // Horizontal edge: burn it only when it runs right to left.
                if xs[ind1] > xs[ind2] {
                    let x1 = (xs[ind2] + 0.5).floor() as i64;
                    let x2 = (xs[ind1] + 0.5).floor() as i64;
                    if x1 <= maxx && x2 > 0 {
                        burn_scanline(grid, row, width, x1, x2 - 1, value, burn);
                    }
                }
                continue;
            }

            if dy < dy2 && dy >= dy1 {
                let intersect = (dy - dy1) * (dx2 - dx1) / (dy2 - dy1) + dx1;
                intersects[count] = (intersect + 0.5).floor();
                count += 1;
            }
        }

        intersects[..count].sort_unstable_by(|a, b| a.total_cmp(b));
        let (pairs, _) = intersects[..count].as_chunks::<2>();
        for pair in pairs {
            let (x1, x2) = (pair[0] as i64, pair[1] as i64);
            if x1 <= maxx && x2 > 0 {
                burn_scanline(grid, row, width, x1, x2 - 1, value, burn);
            }
        }
    }
}

/// `GDALdllImageLine` for a single-band byte grid: Bresenham burn of each
/// polyline segment with the caller's value. Coordinates are pixel
/// coordinates; each point is floored to obtain the integer pixel index,
/// matching GDAL's convention.
pub fn draw_lines(
    grid: &mut [u8],
    width: usize,
    height: usize,
    parts: &[Vec<[f64; 2]>],
    value: u8,
) {
    if width == 0 || height == 0 || grid.len() != width * height {
        return;
    }

    for part in parts {
        if part.len() < 2 {
            continue;
        }
        let last_index = part.len() - 1;
        for j in 1..=last_index {
            let mut i_x = part[j - 1][0].floor() as i64;
            let mut i_y = part[j - 1][1].floor() as i64;
            let i_x1 = part[j][0].floor() as i64;
            let i_y1 = part[j][1].floor() as i64;

            let mut n_delta_x = (i_x1 - i_x).abs();
            let mut n_delta_y = (i_y1 - i_y).abs();
            let n_x_step = if i_x > i_x1 { -1 } else { 1 };
            let n_y_step = if i_y > i_y1 { -1 } else { 1 };

            if n_delta_x >= n_delta_y {
                let n_x_error = n_delta_y << 1;
                let n_y_error = n_x_error - (n_delta_x << 1);
                let mut n_error = n_x_error - n_delta_x;

                if j != last_index {
                    n_delta_x -= 1;
                }

                loop {
                    if n_delta_x < 0 {
                        break;
                    }
                    if 0 <= i_x && i_x < width as i64 && 0 <= i_y && i_y < height as i64 {
                        grid[i_y as usize * width + i_x as usize] = value;
                    }

                    i_x += n_x_step;
                    if n_error > 0 {
                        i_y += n_y_step;
                        n_error += n_y_error;
                    } else {
                        n_error += n_x_error;
                    }
                    n_delta_x -= 1;
                }
            } else {
                let n_x_error = n_delta_x << 1;
                let n_y_error = n_x_error - (n_delta_y << 1);
                let mut n_error = n_x_error - n_delta_y;

                if j != last_index {
                    n_delta_y -= 1;
                }

                loop {
                    if n_delta_y < 0 {
                        break;
                    }
                    if 0 <= i_x && i_x < width as i64 && 0 <= i_y && i_y < height as i64 {
                        grid[i_y as usize * width + i_x as usize] = value;
                    }

                    i_y += n_y_step;
                    if n_error > 0 {
                        i_x += n_x_step;
                        n_error += n_y_error;
                    } else {
                        n_error += n_x_error;
                    }
                    n_delta_y -= 1;
                }
            }
        }
    }
}

/// Inclusive horizontal span burn, clipped to the row.
fn burn_scanline(
    grid: &mut [u8],
    row: usize,
    width: usize,
    start: i64,
    end: i64,
    value: u8,
    burn: Burn,
) {
    if start > end {
        return;
    }
    let start = start.max(0) as usize;
    let end = (end.min(width as i64 - 1)) as usize;
    if start >= width || start > end {
        return;
    }
    match burn {
        Burn::Set => grid[row + start..=row + end].fill(value),
        Burn::Max => {
            for slot in &mut grid[row + start..=row + end] {
                if value > *slot {
                    *slot = value;
                }
            }
        }
    }
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
