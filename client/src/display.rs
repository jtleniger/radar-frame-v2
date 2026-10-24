//! Waveshare 7.3-inch (F) panel output.
//!
//! The server already quantizes each frame to the panel's seven colours. This
//! module decodes the PNG, maps any stray pixels to the nearest panel colour,
//! and packs two pixels per byte in the order the `epd-waveshare` driver wants.

use std::io::Cursor;
use std::thread;
use std::time::Duration;

use epd_waveshare::color::OctColor;
use epd_waveshare::epd7in3f::Epd7in3f;
use epd_waveshare::prelude::WaveshareDisplay;
use rppal::gpio::{Gpio, InputPin, OutputPin};
use rppal::hal::Delay;
use rppal::spi::{Bus, Mode, SimpleHalSpiDevice, SlaveSelect, Spi};
use thiserror::Error;

pub const PANEL_WIDTH: u32 = 800;
pub const PANEL_HEIGHT: u32 = 480;

/// GPIO wiring of the Waveshare 7.3-inch HAT on a Raspberry Pi.
const POWER_PIN: u8 = 18;
const BUSY_PIN: u8 = 24;
const DC_PIN: u8 = 25;
const RESET_PIN: u8 = 17;
const SPI_SPEED_HZ: u32 = 4_000_000;

const EINK_PALETTE: [(u8, u8, u8, OctColor); 7] = [
    (0, 0, 0, OctColor::Black),
    (255, 255, 255, OctColor::White),
    (0, 255, 0, OctColor::Green),
    (0, 0, 255, OctColor::Blue),
    (255, 0, 0, OctColor::Red),
    (255, 255, 0, OctColor::Yellow),
    (255, 128, 0, OctColor::Orange),
];

#[derive(Debug, Error)]
pub enum DisplayError {
    #[error("frame PNG: {0}")]
    Png(String),
    #[error("frame is {width}x{height}, expected {PANEL_WIDTH}x{PANEL_HEIGHT}")]
    Size { width: u32, height: u32 },
    #[error("SPI: {0}")]
    Spi(#[from] rppal::spi::Error),
    #[error("GPIO: {0}")]
    Gpio(#[from] rppal::gpio::Error),
}

pub trait FrameDisplay {
    fn display(&mut self, png: &[u8]) -> Result<(), DisplayError>;
}

type Panel = Epd7in3f<SimpleHalSpiDevice, InputPin, OutputPin, OutputPin, Delay>;

pub struct EpdDisplay {
    panel: Panel,
    spi: SimpleHalSpiDevice,
    delay: Delay,
    awake: bool,
    _power: OutputPin,
}

impl EpdDisplay {
    pub fn new() -> Result<Self, DisplayError> {
        let gpio = Gpio::new()?;

        let mut power = gpio.get(POWER_PIN)?.into_output();
        power.set_high();
        thread::sleep(Duration::from_millis(100));

        let busy = gpio.get(BUSY_PIN)?.into_input();
        let dc = gpio.get(DC_PIN)?.into_output();
        let reset = gpio.get(RESET_PIN)?.into_output();

        let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, SPI_SPEED_HZ, Mode::Mode0)?;
        let mut spi = SimpleHalSpiDevice::new(spi);
        let mut delay = Delay::new();
        let panel = Panel::new(&mut spi, busy, dc, reset, &mut delay, None)?;

        Ok(EpdDisplay {
            panel,
            spi,
            delay,
            // `Panel::new` initializes the controller, so the first refresh
            // does not need another wake-up.
            awake: true,
            _power: power,
        })
    }
}

impl FrameDisplay for EpdDisplay {
    fn display(&mut self, png: &[u8]) -> Result<(), DisplayError> {
        // Decode before touching SPI: a malformed frame must not disturb the
        // image that is already on the panel.
        let frame = png_to_frame(png)?;

        if !self.awake {
            self.panel.wake_up(&mut self.spi, &mut self.delay)?;
            self.awake = true;
        }

        if let Err(error) =
            self.panel
                .update_and_display_frame(&mut self.spi, &frame, &mut self.delay)
        {
            // Re-initialize on the next refresh; the controller state after a
            // failed transfer is not trustworthy.
            self.awake = false;
            return Err(error.into());
        }

        let sleep = self.panel.sleep(&mut self.spi, &mut self.delay);
        self.awake = false;
        sleep?;

        Ok(())
    }
}

pub struct NoopDisplay;

impl FrameDisplay for NoopDisplay {
    fn display(&mut self, png: &[u8]) -> Result<(), DisplayError> {
        let frame = png_to_frame(png)?;
        tracing::info!(bytes = frame.len(), "noop display: frame decoded");
        Ok(())
    }
}

/// Decode a server PNG and pack it as seven-colour nibbles.
pub fn png_to_frame(png_bytes: &[u8]) -> Result<Vec<u8>, DisplayError> {
    let decoder = png::Decoder::new(Cursor::new(png_bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|error| DisplayError::Png(error.to_string()))?;

    let (width, height, color_type, bit_depth, palette) = {
        let info = reader.info();
        (
            info.width,
            info.height,
            info.color_type,
            info.bit_depth,
            info.palette.clone(),
        )
    };

    if (width, height) != (PANEL_WIDTH, PANEL_HEIGHT) {
        return Err(DisplayError::Size { width, height });
    }

    let buffer_size = reader
        .output_buffer_size()
        .ok_or_else(|| DisplayError::Png("image is too large".to_string()))?;
    let mut buffer = vec![0; buffer_size];
    let output = reader
        .next_frame(&mut buffer)
        .map_err(|error| DisplayError::Png(error.to_string()))?;
    let data = &buffer[..output.buffer_size()];

    let pixels = rgba_pixels(
        width,
        height,
        color_type,
        bit_depth,
        palette.as_deref(),
        data,
    )?;

    if pixels.len() % 2 != 0 {
        return Err(DisplayError::Png(
            "frame has an odd pixel count".to_string(),
        ));
    }

    let mut frame = Vec::with_capacity(pixels.len() / 2);
    for pair in pixels.chunks_exact(2) {
        frame.push(OctColor::colors_byte(
            nearest_color(pair[0]),
            nearest_color(pair[1]),
        ));
    }
    Ok(frame)
}

fn rgba_pixels(
    width: u32,
    height: u32,
    color_type: png::ColorType,
    bit_depth: png::BitDepth,
    palette: Option<&[u8]>,
    data: &[u8],
) -> Result<Vec<[u8; 4]>, DisplayError> {
    let count = (width * height) as usize;
    let mut pixels = Vec::with_capacity(count);
    let unsupported = || {
        DisplayError::Png(format!(
            "unsupported PNG {color_type:?} at {bit_depth:?} bit depth"
        ))
    };

    match color_type {
        png::ColorType::Rgb => {
            if bit_depth != png::BitDepth::Eight || data.len() < count * 3 {
                return Err(unsupported());
            }
            for pixel in data.chunks_exact(3).take(count) {
                pixels.push([pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        png::ColorType::Rgba => {
            if bit_depth != png::BitDepth::Eight || data.len() < count * 4 {
                return Err(unsupported());
            }
            for pixel in data.chunks_exact(4).take(count) {
                pixels.push([pixel[0], pixel[1], pixel[2], pixel[3]]);
            }
        }
        png::ColorType::Grayscale => {
            if bit_depth != png::BitDepth::Eight || data.len() < count {
                return Err(unsupported());
            }
            for value in data.iter().copied().take(count) {
                pixels.push([value, value, value, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            if bit_depth != png::BitDepth::Eight || data.len() < count * 2 {
                return Err(unsupported());
            }
            for pixel in data.chunks_exact(2).take(count) {
                pixels.push([pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Indexed => {
            let palette = palette
                .ok_or_else(|| DisplayError::Png("indexed PNG without a palette".to_string()))?;
            let depth = bit_depth as usize;
            if !matches!(depth, 1 | 2 | 4 | 8) {
                return Err(unsupported());
            }
            for index in 0..count {
                let entry = if depth == 8 {
                    data.get(index).copied()
                } else {
                    let bit = index * depth;
                    let byte = data.get(bit / 8).copied();
                    byte.map(|byte| {
                        let shift = 8 - depth - (bit % 8);
                        (byte >> shift) & ((1u16 << depth) - 1) as u8
                    })
                }
                .ok_or_else(|| DisplayError::Png("indexed PNG is truncated".to_string()))?
                    as usize;
                let offset = entry * 3;
                let rgb = palette
                    .get(offset..offset + 3)
                    .ok_or_else(|| DisplayError::Png("palette index out of range".to_string()))?;
                pixels.push([rgb[0], rgb[1], rgb[2], 255]);
            }
        }
    }

    for pixel in &mut pixels {
        if pixel[3] < 128 {
            *pixel = [255, 255, 255, 255];
        }
    }

    Ok(pixels)
}

fn nearest_color([r, g, b, _]: [u8; 4]) -> OctColor {
    // The panel has no grey level. Without this, a neutral pixel can land on
    // orange or green; map it to the nearer endpoint instead.
    let min = r.min(g).min(b);
    let max = r.max(g).max(b);
    if max - min < 24 {
        return if (r as u16 + g as u16 + b as u16) / 3 < 128 {
            OctColor::Black
        } else {
            OctColor::White
        };
    }

    let mut best = OctColor::White;
    let mut best_distance = i32::MAX;
    for (red, green, blue, color) in EINK_PALETTE {
        let dr = r as i32 - red as i32;
        let dg = g as i32 - green as i32;
        let db = b as i32 - blue as i32;
        let distance = dr * dr + dg * dg + db * db;
        if distance < best_distance {
            best_distance = distance;
            best = color;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::rgb_png;

    fn panel_png() -> Vec<u8> {
        let mut pixels = vec![[255u8, 255, 255]; (PANEL_WIDTH * PANEL_HEIGHT) as usize];
        pixels[0] = [255, 0, 0];
        pixels[1] = [0, 255, 0];
        pixels[2] = [0, 0, 255];
        pixels[3] = [255, 255, 0];
        pixels[4] = [200, 200, 200];
        pixels[5] = [20, 20, 20];
        rgb_png(PANEL_WIDTH, PANEL_HEIGHT, &pixels)
    }

    #[test]
    fn packs_red_green_blue_yellow_and_neutral_pixels() {
        let frame = png_to_frame(&panel_png()).unwrap();
        assert_eq!(frame.len(), (PANEL_WIDTH * PANEL_HEIGHT / 2) as usize);
        assert_eq!(
            frame[0],
            OctColor::colors_byte(OctColor::Red, OctColor::Green)
        );
        assert_eq!(
            frame[1],
            OctColor::colors_byte(OctColor::Blue, OctColor::Yellow)
        );
        assert_eq!(
            frame[2],
            OctColor::colors_byte(OctColor::White, OctColor::Black)
        );
    }

    #[test]
    fn rejects_a_frame_with_the_wrong_panel_size() {
        let png = rgb_png(
            2,
            2,
            &[[0, 0, 0], [255, 255, 255], [255, 0, 0], [0, 255, 0]],
        );
        let error = png_to_frame(&png).unwrap_err();
        assert!(matches!(
            error,
            DisplayError::Size {
                width: 2,
                height: 2
            }
        ));
    }

    #[test]
    fn neutral_pixels_do_not_become_chromatic() {
        assert_eq!(nearest_color([200, 200, 200, 255]), OctColor::White);
        assert_eq!(nearest_color([20, 20, 20, 255]), OctColor::Black);
    }
}
