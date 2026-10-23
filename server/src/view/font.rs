use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

use crate::view::image::Image;

const SPLEEN_16X32: &[u8] = include_bytes!("../../assets/spleen-16x32.otf");
const SPLEEN_32X64: &[u8] = include_bytes!("../../assets/spleen-32x64.otf");

/// The three text sizes the views use. Pillow loaded the 16x32 font at 32px,
/// the 32x64 font at 64px, and the same font doubled to 128px for the current
/// temperature.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Size {
    Small,
    Large,
    XLarge,
}

impl Size {
    fn face(self) -> Face {
        match self {
            Size::Small => Face::Spleen16x32,
            Size::Large | Size::XLarge => Face::Spleen32x64,
        }
    }

    fn pixels(self) -> f32 {
        match self {
            Size::Small => 32.0,
            Size::Large => 64.0,
            Size::XLarge => 128.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Face {
    Spleen16x32,
    Spleen32x64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Anchor {
    /// Left, ascender: Pillow's default (`anchor=None`).
    LeftAscender,
    /// Middle, top of the glyph ink (Pillow's "mt").
    MiddleTop,
    /// Left, middle of the ascender..descender line (Pillow's "lm").
    LeftMiddle,
    /// Right, middle of the ascender..descender line (Pillow's "rm").
    RightMiddle,
}

struct Glyph {
    advance: f32,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    mask: Vec<u8>,
}

pub struct Fonts {
    spleen16: FontVec,
    spleen32: FontVec,
    glyphs: Mutex<HashMap<(Size, char), Arc<Glyph>>>,
}

impl Fonts {
    pub fn new() -> Self {
        Fonts {
            spleen16: FontVec::try_from_vec(SPLEEN_16X32.to_vec())
                .expect("embedded spleen-16x32 font is valid"),
            spleen32: FontVec::try_from_vec(SPLEEN_32X64.to_vec())
                .expect("embedded spleen-32x64 font is valid"),
            glyphs: Mutex::new(HashMap::new()),
        }
    }

    fn font(&self, face: Face) -> &FontVec {
        match face {
            Face::Spleen16x32 => &self.spleen16,
            Face::Spleen32x64 => &self.spleen32,
        }
    }

    fn glyph(&self, size: Size, ch: char) -> Arc<Glyph> {
        if let Some(glyph) = self.glyphs.lock().unwrap().get(&(size, ch)) {
            return Arc::clone(glyph);
        }

        let face = self.font(size.face());
        let scale = PxScale::from(size.pixels());
        let scaled = face.as_scaled(scale);
        let id = face.glyph_id(ch);
        let advance = scaled.h_advance(id);

        let (left, top, width, height, mask) = match face.outline_glyph(id.with_scale(scale)) {
            Some(outlined) => {
                let bounds = outlined.px_bounds();
                let left = bounds.min.x.floor() as i32;
                let top = bounds.min.y.floor() as i32;
                let width = (bounds.max.x.ceil() as i32 - left).max(0) as u32;
                let height = (bounds.max.y.ceil() as i32 - top).max(0) as u32;
                let mut coverage = vec![0f32; (width * height) as usize];
                outlined.draw(|x, y, c| {
                    if x < width && y < height {
                        coverage[(y * width + x) as usize] = c;
                    }
                });
                // Pillow rendered these fonts in FreeType's monochrome mode,
                // which fills any pixel the outline touches.
                let mask = coverage
                    .into_iter()
                    .map(|c| if c > 0.0 { 255 } else { 0 })
                    .collect();
                (left, top, width, height, mask)
            }
            None => (0, 0, 0, 0, Vec::new()),
        };

        let glyph = Arc::new(Glyph {
            advance,
            left,
            top,
            width,
            height,
            mask,
        });
        self.glyphs
            .lock()
            .unwrap()
            .insert((size, ch), Arc::clone(&glyph));
        glyph
    }

    fn ascent(&self, size: Size) -> f32 {
        self.font(size.face()).as_scaled(size.pixels()).ascent()
    }

    fn line_middle_offset(&self, size: Size) -> f32 {
        let scaled = self.font(size.face()).as_scaled(size.pixels());
        (scaled.ascent() + scaled.descent()) / 2.0
    }

    /// The text's advance width and its ink extents relative to the baseline.
    fn measure(&self, size: Size, text: &str) -> (f32, i32, i32) {
        let mut width = 0f32;
        let mut top = 0i32;
        let mut bottom = 0i32;

        for ch in text.chars() {
            let glyph = self.glyph(size, ch);
            if glyph.height > 0 {
                top = top.min(glyph.top);
                bottom = bottom.max(glyph.top + glyph.height as i32);
            }
            width += glyph.advance;
        }

        (width, top, bottom)
    }

    pub fn draw_text(
        &self,
        image: &mut Image,
        at: (f32, f32),
        size: Size,
        text: &str,
        anchor: Anchor,
        color: [u8; 4],
    ) {
        let (x, y) = at;
        let (width, top, _bottom) = self.measure(size, text);

        let horizontal = match anchor {
            Anchor::LeftAscender | Anchor::LeftMiddle => 0.0,
            Anchor::MiddleTop => width / 2.0,
            Anchor::RightMiddle => width,
        };

        let baseline = match anchor {
            Anchor::LeftAscender => y + self.ascent(size),
            Anchor::MiddleTop => y - top as f32,
            // Pillow's "m" is the middle of the ascender..descender line,
            // not the middle of the glyph ink.
            Anchor::LeftMiddle | Anchor::RightMiddle => y + self.line_middle_offset(size),
        };

        let mut pen = x - horizontal;

        for ch in text.chars() {
            let glyph = self.glyph(size, ch);
            let gx = pen + glyph.left as f32;
            let gy = baseline + glyph.top as f32;

            for row in 0..glyph.height {
                for col in 0..glyph.width {
                    if glyph.mask[(row * glyph.width + col) as usize] != 0 {
                        image.put_pixel(
                            gx.trunc() as i64 + col as i64,
                            gy.trunc() as i64 + row as i64,
                            color,
                        );
                    }
                }
            }

            pen += glyph.advance;
        }
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}
