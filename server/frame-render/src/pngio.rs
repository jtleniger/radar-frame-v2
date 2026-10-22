//! PNG reading (grayscale street mask, RGBA frame) and RGBA writing.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::{Error, Result};

/// An 8-bit grayscale mask, plus the `tRNS` gray level if the PNG declares one.
///
/// GDAL writes `tRNS` for the `streets.tif` nodata value, and Pillow honours it
/// when the mask is pasted, so the renderer has to as well.
#[derive(Debug, Clone)]
pub struct Gray8 {
    pub width: usize,
    pub height: usize,
    pub samples: Vec<u8>,
    pub transparent: Option<u8>,
}

struct Decoded {
    width: usize,
    height: usize,
    color_type: png::ColorType,
    bit_depth: png::BitDepth,
    samples: Vec<u8>,
    trns: Option<Vec<u8>>,
}

fn decode(path: &Path) -> Result<Decoded> {
    let fail = |e: png::DecodingError| Error(format!("{}: {e}", path.display()));

    let file = File::open(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().map_err(fail)?;

    let size = reader
        .output_buffer_size()
        .ok_or_else(|| Error(format!("{}: PNG is too large to decode", path.display())))?;
    let mut samples = vec![0u8; size];
    let info = reader.next_frame(&mut samples).map_err(fail)?;
    samples.truncate(info.buffer_size());

    let trns = reader.info().trns.as_ref().map(|chunk| chunk.to_vec());

    Ok(Decoded {
        width: info.width as usize,
        height: info.height as usize,
        color_type: info.color_type,
        bit_depth: info.bit_depth,
        samples,
        trns,
    })
}

fn expect(decoded: &Decoded, color_type: png::ColorType, path: &Path) -> Result<()> {
    if decoded.color_type != color_type || decoded.bit_depth != png::BitDepth::Eight {
        return Err(Error(format!(
            "{}: expected an 8-bit {color_type:?} PNG, got {:?} {:?}",
            path.display(),
            decoded.color_type,
            decoded.bit_depth
        )));
    }
    Ok(())
}

/// Read an 8-bit, single-channel grayscale PNG, row-major.
///
/// This is what GDAL's PNG driver writes for the one-band byte `streets.tif`
/// raster with no color table.
pub fn read_gray8(path: &Path) -> Result<Gray8> {
    let decoded = decode(path)?;
    expect(&decoded, png::ColorType::Grayscale, path)?;

    // The png crate normalizes a grayscale `tRNS` payload to the low byte of
    // the 16-bit gray level, which is the sample value for 8-bit images.
    let transparent = decoded
        .trns
        .as_ref()
        .and_then(|chunk| chunk.first().copied());

    Ok(Gray8 {
        width: decoded.width,
        height: decoded.height,
        samples: decoded.samples,
        transparent,
    })
}

/// Read an 8-bit RGBA PNG, pixel data only.
pub fn read_rgba(path: &Path) -> Result<(usize, usize, Vec<u8>)> {
    let decoded = decode(path)?;
    expect(&decoded, png::ColorType::Rgba, path)?;
    Ok((decoded.width, decoded.height, decoded.samples))
}

/// Write an 8-bit RGBA PNG from a row-major `width * height * 4` buffer.
pub fn write_rgba(path: &Path, width: usize, height: usize, rgba: &[u8]) -> Result<()> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| Error(format!("frame size {width}x{height} is too large")))?;
    if rgba.len() != expected {
        return Err(Error(format!(
            "{}: expected {expected} RGBA bytes for {width}x{height}, got {}",
            path.display(),
            rgba.len()
        )));
    }
    let (width, height) = (u32::try_from(width), u32::try_from(height));
    let (Ok(width), Ok(height)) = (width, height) else {
        return Err(Error(format!(
            "frame size {width:?}x{height:?} does not fit a PNG header"
        )));
    };

    let fail = |e: png::EncodingError| Error(format!("{}: {e}", path.display()));

    let file = File::create(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(rgba).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_gray(path: &Path, width: u32, height: u32, samples: &[u8], transparent: Option<u16>) {
        let file = File::create(path).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        if let Some(gray) = transparent {
            encoder.set_trns(gray.to_be_bytes().to_vec());
        }
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(samples).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn rgba_round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.png");

        #[rustfmt::skip]
        let rgba: Vec<u8> = vec![
            1, 2, 3, 4,   5, 6, 7, 8,   9, 10, 11, 12,
            13, 14, 15, 16,   17, 18, 19, 20,   21, 22, 23, 24,
        ];
        write_rgba(&path, 3, 2, &rgba).unwrap();

        let (width, height, decoded) = read_rgba(&path).unwrap();
        assert_eq!((width, height), (3, 2));
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn grayscale_mask_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("streets.png");
        write_gray(&path, 3, 2, &[0, 255, 0, 255, 0, 255], None);

        let mask = read_gray8(&path).unwrap();
        assert_eq!((mask.width, mask.height), (3, 2));
        assert_eq!(mask.samples, vec![0, 255, 0, 255, 0, 255]);
        assert_eq!(mask.transparent, None);
    }

    #[test]
    fn grayscale_mask_reports_the_trns_gray_level() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("streets.png");
        write_gray(&path, 2, 1, &[1, 255], Some(1));

        let mask = read_gray8(&path).unwrap();
        assert_eq!(mask.transparent, Some(1));
    }

    #[test]
    fn rejects_non_grayscale_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rgba.png");
        write_rgba(&path, 1, 1, &[1, 2, 3, 255]).unwrap();

        let err = read_gray8(&path).unwrap_err().to_string();
        assert!(err.contains("8-bit Grayscale"), "{err}");
    }

    #[test]
    fn rejects_mismatched_buffer_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.png");
        assert!(write_rgba(&path, 2, 2, &[0; 3]).is_err());
    }
}
