//! Bresenham line burn matching GDAL's `GDALdllImageLine`.

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
