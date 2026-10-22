use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use crate::error::{Error, Result};
use crate::image::Image;

/// The seven colours of the Waveshare 7.5" e-ink panel, in the order
/// `setup/palette.py` wrote them.
pub const EINK_COLORS: [[u8; 3]; 7] = [
    [0, 0, 0],
    [255, 255, 255],
    [255, 0, 0],
    [255, 128, 0],
    [255, 255, 0],
    [0, 255, 0],
    [0, 0, 255],
];

#[derive(Clone)]
pub struct Palette {
    pub colors: Vec<[u8; 3]>,
}

impl Palette {
    pub fn new(colors: Vec<[u8; 3]>) -> Self {
        Palette { colors }
    }

    pub fn eink() -> Self {
        Palette::new(EINK_COLORS.to_vec())
    }

    pub fn from_png(path: &Path) -> Result<Self> {
        let colors = Image::png_palette(path)?;
        if colors.is_empty() {
            return Err(Error::Image("palette image has no colours".into()));
        }
        Ok(Palette::new(colors))
    }

    fn nearest(&self, color: [f32; 3]) -> [u8; 3] {
        let mut best = self.colors[0];
        let mut best_distance = f32::INFINITY;

        for &candidate in &self.colors {
            let dr = color[0] - candidate[0] as f32;
            let dg = color[1] - candidate[1] as f32;
            let db = color[2] - candidate[2] as f32;
            let distance = dr * dr + dg * dg + db * db;
            if distance < best_distance {
                best_distance = distance;
                best = candidate;
            }
        }

        best
    }

    /// Map every pixel to the palette. With `dither`, diffuse the error the
    /// way `Image.quantize(..., dither=FLOYDSTEINBERG)` does.
    pub fn quantize(&self, image: &mut Image, dither: bool) {
        if !dither {
            for y in 0..image.height {
                for x in 0..image.width {
                    let px = image.pixel(x, y);
                    let color = self.nearest([px[0] as f32, px[1] as f32, px[2] as f32]);
                    image.put_pixel(x as i64, y as i64, [color[0], color[1], color[2], 255]);
                }
            }
            return;
        }

        let width = image.width as usize;
        let height = image.height as usize;
        let mut current = vec![[0f32; 3]; width + 2];
        let mut next = vec![[0f32; 3]; width + 2];

        for y in 0..height {
            for x in 0..width {
                let px = image.pixel(x as u32, y as u32);
                let mut value = [
                    px[0] as f32 + current[x + 1][0],
                    px[1] as f32 + current[x + 1][1],
                    px[2] as f32 + current[x + 1][2],
                ];
                for channel in &mut value {
                    *channel = channel.clamp(0.0, 255.0);
                }

                let chosen = self.nearest(value);
                image.put_pixel(x as i64, y as i64, [chosen[0], chosen[1], chosen[2], 255]);

                let error = [
                    value[0] - chosen[0] as f32,
                    value[1] - chosen[1] as f32,
                    value[2] - chosen[2] as f32,
                ];

                for (i, e) in error.iter().enumerate() {
                    current[x + 2][i] += e * 7.0 / 16.0;
                    next[x][i] += e * 3.0 / 16.0;
                    next[x + 1][i] += e * 5.0 / 16.0;
                    next[x + 2][i] += e / 16.0;
                }
            }

            std::mem::swap(&mut current, &mut next);
            for value in next.iter_mut() {
                *value = [0.0; 3];
            }
        }
    }
}

/// Write the e-ink palette as a 7x1 indexed PNG, which is what
/// `setup/palette.py` used to produce.
pub fn write_eink_png(path: &Path) -> Result<()> {
    let file = File::create(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), EINK_COLORS.len() as u32, 1);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(EINK_COLORS.concat());
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&[0, 1, 2, 3, 4, 5, 6])?;
    writer.finish()?;
    Ok(())
}
