//! GDAL-compatible even-odd scanline polygon fill.

/// How a burned pixel is written.
#[derive(Clone, Copy)]
pub(super) enum Burn {
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

pub(super) fn fill_ring_with(
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
