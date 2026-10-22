use std::fs::File;
use std::io::BufReader;
use std::io::BufWriter;
use std::path::Path;

use crate::error::{Error, Result};

/// An RGBA raster. All drawing is clipped to the canvas.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    data: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, color: [u8; 4]) -> Self {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            data.extend_from_slice(&color);
        }
        Image {
            width,
            height,
            data,
        }
    }

    pub fn from_png(path: &Path) -> Result<Self> {
        let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        let mut reader = decoder.read_info()?;
        let mut buf = vec![
            0;
            reader
                .output_buffer_size()
                .ok_or_else(|| Error::Image("PNG too large".into()))?
        ];
        let info = reader.next_frame(&mut buf)?;
        let bytes = &buf[..info.buffer_size()];
        let width = info.width;
        let height = info.height;

        let mut data = Vec::with_capacity((width * height * 4) as usize);
        match info.color_type {
            png::ColorType::Rgba => data.extend_from_slice(bytes),
            png::ColorType::Rgb => {
                for px in bytes.as_chunks::<3>().0 {
                    data.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for px in bytes.as_chunks::<2>().0 {
                    data.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
                }
            }
            png::ColorType::Grayscale => {
                for &g in bytes {
                    data.extend_from_slice(&[g, g, g, 255]);
                }
            }
            png::ColorType::Indexed => {
                let palette = reader
                    .info()
                    .palette
                    .as_ref()
                    .ok_or_else(|| Error::Image("indexed PNG without a palette".into()))?
                    .to_vec();
                let index = |i: u8| -> Result<[u8; 3]> {
                    let offset = i as usize * 3;
                    palette
                        .get(offset..offset + 3)
                        .map(|c| [c[0], c[1], c[2]])
                        .ok_or_else(|| Error::Image("palette index out of range".into()))
                };

                match info.bit_depth {
                    png::BitDepth::Eight => {
                        for &i in bytes {
                            let color = index(i)?;
                            data.extend_from_slice(&[color[0], color[1], color[2], 255]);
                        }
                    }
                    // Pillow writes small palettes packed at 4, 2, or 1 bit.
                    depth => {
                        let per_byte = 8 / depth as usize;
                        let mask = (1u16 << depth as u16) - 1;
                        let shift = depth as u16;
                        for &byte in bytes {
                            for slot in 0..per_byte {
                                let value = (byte as u16 >> (8 - shift * (slot as u16 + 1))) & mask;
                                let color = index(value as u8)?;
                                data.extend_from_slice(&[color[0], color[1], color[2], 255]);
                            }
                        }
                    }
                }
            }
        }

        Ok(Image {
            width,
            height,
            data,
        })
    }

    pub fn write_png(&self, path: &Path) -> Result<()> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;

        let mut rgb = Vec::with_capacity((self.width * self.height * 3) as usize);
        for px in self.data.as_chunks::<4>().0 {
            rgb.extend_from_slice(&px[..3]);
        }
        writer.write_image_data(&rgb)?;
        writer.finish()?;
        Ok(())
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    pub fn put_pixel(&mut self, x: i64, y: i64, color: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let i = ((y as u32 * self.width + x as u32) * 4) as usize;
        self.data[i..i + 4].copy_from_slice(&color);
    }

    /// Copy the source's colour channels, ignoring its alpha, like Pillow's
    /// `paste` when the source has no mask.
    pub fn paste(&mut self, source: &Image, x: i64, y: i64) {
        for sy in 0..source.height as i64 {
            let dy = y + sy;
            if dy < 0 || dy >= self.height as i64 {
                continue;
            }
            for sx in 0..source.width as i64 {
                let dx = x + sx;
                if dx < 0 || dx >= self.width as i64 {
                    continue;
                }
                let color = source.pixel(sx as u32, sy as u32);
                self.put_pixel(dx, dy, [color[0], color[1], color[2], 255]);
            }
        }
    }

    /// Copy the source through its alpha channel, like Pillow's
    /// `paste(source, box, source)`.
    pub fn paste_masked(&mut self, source: &Image, x: i64, y: i64) {
        for sy in 0..source.height as i64 {
            let dy = y + sy;
            if dy < 0 || dy >= self.height as i64 {
                continue;
            }
            for sx in 0..source.width as i64 {
                let dx = x + sx;
                if dx < 0 || dx >= self.width as i64 {
                    continue;
                }
                let src = source.pixel(sx as u32, sy as u32);
                let a = src[3] as u32;
                if a == 0 {
                    continue;
                }
                let dst = self.pixel(dx as u32, dy as u32);
                let blend = |s: u8, d: u8| -> u8 {
                    ((s as u32 * a + d as u32 * (255 - a) + 127) / 255) as u8
                };
                self.put_pixel(
                    dx,
                    dy,
                    [
                        blend(src[0], dst[0]),
                        blend(src[1], dst[1]),
                        blend(src[2], dst[2]),
                        255,
                    ],
                );
            }
        }
    }

    /// Fill the inclusive rectangle, matching `ImageDraw.rectangle`.
    pub fn fill_rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, color: [u8; 4]) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.put_pixel(x, y, color);
            }
        }
    }

    /// Draw a vertical line `width` pixels wide, starting at `x` and extending
    /// to the right, matching `ImageDraw.line` for even widths.
    pub fn vertical_line(&mut self, x: i64, y0: i64, y1: i64, width: u32, color: [u8; 4]) {
        for y in y0.min(y1)..=y0.max(y1) {
            for dx in 0..width as i64 {
                self.put_pixel(x + dx, y, color);
            }
        }
    }

    /// The palette entries of an indexed PNG, in table order.
    pub fn png_palette(path: &Path) -> Result<Vec<[u8; 3]>> {
        let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        let reader = decoder.read_info()?;
        let palette = reader
            .info()
            .palette
            .as_ref()
            .ok_or_else(|| Error::Image("palette image has no PLTE chunk".into()))?;

        Ok(palette
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect())
    }
}

impl From<resvg::tiny_skia::Pixmap> for Image {
    fn from(pixmap: resvg::tiny_skia::Pixmap) -> Self {
        Image {
            width: pixmap.width(),
            height: pixmap.height(),
            data: pixmap.data().to_vec(),
        }
    }
}
