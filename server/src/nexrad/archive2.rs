use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use crate::nexrad::error::{Error, Result};

/// Volume header record length at the start of every Archive II volume.
const VOLUME_HEADER_LEN: usize = 24;
/// Legacy CTM header (12 bytes) followed by the message header (16 bytes).
const MESSAGE_HEADER_LEN: usize = 28;
/// Fixed slot size for messages other than Message 31.
const LEGACY_MESSAGE_SLOT_LEN: usize = 2432;
/// Message 31 header length, including the data block count.
const MESSAGE31_HEADER_LEN: usize = 32;

pub struct Volume {
    pub scans: BTreeMap<u8, Vec<Radial>>,
}

pub struct Radial {
    pub header: RadialHeader,
    pub volume: VolumeBlock,
    pub moments: Moments,
}

pub struct RadialHeader {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub elevation_number: u8,
    /// 0.5 when the azimuth resolution code is 1, otherwise 1.0.
    pub azimuth_resolution_deg: f64,
}

pub struct VolumeBlock {
    pub lat: f32,
    pub lon: f32,
}

#[derive(Default)]
pub struct Moments {
    pub reflectivity: Option<Moment>,
    pub velocity: Option<Moment>,
    pub spectrum_width: Option<Moment>,
    pub zdr: Option<Moment>,
    pub phi: Option<Moment>,
    pub rho: Option<Moment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Product {
    Ref,
    Vel,
    Sw,
    Zdr,
    Phi,
    Rho,
}

impl Product {
    pub fn as_str(self) -> &'static str {
        match self {
            Product::Ref => "REF",
            Product::Vel => "VEL",
            Product::Sw => "SW",
            Product::Zdr => "ZDR",
            Product::Phi => "PHI",
            Product::Rho => "RHO",
        }
    }
}

impl std::str::FromStr for Product {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_uppercase().as_str() {
            "REF" => Ok(Product::Ref),
            "VEL" => Ok(Product::Vel),
            "SW" => Ok(Product::Sw),
            "ZDR" => Ok(Product::Zdr),
            "PHI" => Ok(Product::Phi),
            "RHO" => Ok(Product::Rho),
            other => Err(Error::Format(format!("invalid product {other}"))),
        }
    }
}

impl Moments {
    pub fn get(&self, product: Product) -> Option<&Moment> {
        match product {
            Product::Ref => self.reflectivity.as_ref(),
            Product::Vel => self.velocity.as_ref(),
            Product::Sw => self.spectrum_width.as_ref(),
            Product::Zdr => self.zdr.as_ref(),
            Product::Phi => self.phi.as_ref(),
            Product::Rho => self.rho.as_ref(),
        }
    }
}

pub struct Moment {
    pub gates: u16,
    pub range_m: u16,
    pub interval_m: u16,
    pub word_size: u8,
    pub scale: f32,
    pub offset: f32,
    pub data: Vec<u8>,
}

impl Moment {
    pub fn scaled(&self) -> Result<Vec<f32>> {
        if self.word_size != 8 && self.word_size != 16 {
            return Err(Error::Unsupported(format!(
                "data word size {}",
                self.word_size
            )));
        }

        let expected = self.gates as usize * self.word_size as usize / 8;
        if self.data.len() != expected {
            return Err(Error::Format(format!(
                "moment data length {} does not match {} gates of {} bits",
                self.data.len(),
                self.gates,
                self.word_size
            )));
        }

        let mut scaled = Vec::with_capacity(self.gates as usize);
        let raw = self.data.chunks_exact(self.word_size as usize / 8);
        for chunk in raw {
            let value = if self.word_size == 8 {
                chunk[0] as u16
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            };
            scaled.push(match value {
                0 => 999.0,
                1 => 998.0,
                value if self.scale == 0.0 => value as f32,
                value => (value as f32 - self.offset) / self.scale,
            });
        }
        Ok(scaled)
    }
}

pub fn read_volume(path: &Path) -> Result<Volume> {
    let bytes =
        std::fs::read(path).map_err(|err| Error::Format(format!("{}: {err}", path.display())))?;
    parse_volume(&bytes)
}

pub fn parse_volume(bytes: &[u8]) -> Result<Volume> {
    let decompressed;
    let data: &[u8] = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes).read_to_end(&mut out)?;
        decompressed = out;
        &decompressed
    } else {
        bytes
    };

    if data.len() < VOLUME_HEADER_LEN {
        return Err(Error::Format(format!(
            "volume header truncated: {} bytes",
            data.len()
        )));
    }

    let mut volume = Volume {
        scans: BTreeMap::new(),
    };
    let mut pos = VOLUME_HEADER_LEN;

    while pos + 4 <= data.len() {
        let size = read_i32_at(data, pos, "LDM record size")?;
        let size_start = pos;
        pos += 4;

        if size == 0 {
            // Older files have no LDM records: the remaining stream is a plain
            // sequence of messages starting right after the zero control word.
            parse_messages(&mut volume, &data[size_start..])?;
            break;
        }

        let length = size.unsigned_abs() as usize;
        let record = data.get(pos..pos + length).ok_or_else(|| {
            Error::Format(format!(
                "LDM record at byte {size_start} exceeds file length"
            ))
        })?;
        pos += length;

        if record.starts_with(b"BZh") {
            let mut decoder = bzip2::read::MultiBzDecoder::new(record);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out)?;
            parse_messages(&mut volume, &out)?;
        } else {
            parse_messages(&mut volume, record)?;
        }
    }

    Ok(volume)
}

fn parse_messages(volume: &mut Volume, data: &[u8]) -> Result<()> {
    let mut start = 0;

    while data.len() - start >= MESSAGE_HEADER_LEN {
        let message_type = data[start + 15];

        if message_type != 31 {
            if data.len() - start < LEGACY_MESSAGE_SLOT_LEN {
                break;
            }
            start += LEGACY_MESSAGE_SLOT_LEN;
            continue;
        }

        let body_start = start + MESSAGE_HEADER_LEN;
        if body_start + MESSAGE31_HEADER_LEN > data.len() {
            return Err(Error::Format(format!(
                "message 31 header at byte {body_start} truncated"
            )));
        }

        let compression_indicator = data[body_start + 16];
        if compression_indicator != 0 {
            return Err(Error::Unsupported(format!(
                "message 31 compression indicator {compression_indicator}"
            )));
        }

        let radial_length =
            u16::from_be_bytes([data[body_start + 18], data[body_start + 19]]) as usize;
        if radial_length < MESSAGE31_HEADER_LEN {
            return Err(Error::Format(format!(
                "message 31 radial length {radial_length} at byte {start} is too small"
            )));
        }

        let end = body_start + radial_length;
        if end > data.len() {
            return Err(Error::Format(format!(
                "message 31 radial at byte {start} exceeds record length"
            )));
        }

        let radial = parse_radial(&data[body_start..end])?;
        volume
            .scans
            .entry(radial.header.elevation_number)
            .or_default()
            .push(radial);
        start = end;
    }

    Ok(())
}

fn parse_radial(body: &[u8]) -> Result<Radial> {
    let azimuth_deg = read_f32_at(body, 12, "message 31 azimuth")?;
    let resolution_code = body[20];
    let elevation_number = body[22];
    let elevation_deg = read_f32_at(body, 24, "message 31 elevation")?;
    let data_block_count = read_u16_at(body, 30, "message 31 data block count")? as usize;

    let pointer_start = MESSAGE31_HEADER_LEN;
    let pointer_bytes = data_block_count * 4;
    if pointer_start + pointer_bytes > body.len() {
        return Err(Error::Format(format!(
            "message 31 data block pointers exceed radial length {}",
            body.len()
        )));
    }

    let mut pointers = Vec::with_capacity(data_block_count + 4);
    for i in 0..data_block_count {
        pointers.push(read_u32_at(
            body,
            pointer_start + i * 4,
            "message 31 data block pointer",
        )?);
    }

    // Newer RDA builds add block pointers past the count, terminated by the
    // start of the VOL block ("RVOL"). Zero pointers are padding.
    let mut probe = pointer_start + pointer_bytes;
    let mut extra = 0;
    while extra < 20 && probe + 4 <= body.len() {
        if &body[probe..probe + 4] == b"RVOL" {
            break;
        }
        let pointer = u32::from_be_bytes([
            body[probe],
            body[probe + 1],
            body[probe + 2],
            body[probe + 3],
        ]);
        if pointer != 0 {
            pointers.push(pointer);
        }
        probe += 4;
        extra += 1;
    }

    let mut volume = None;
    let mut moments = Moments::default();

    for pointer in pointers {
        let offset = pointer as usize;
        let name = body.get(offset + 1..offset + 4).ok_or_else(|| {
            Error::Format(format!(
                "data block pointer {offset} exceeds radial length {}",
                body.len()
            ))
        })?;

        match name {
            b"VOL" => {
                volume = Some(VolumeBlock {
                    lat: read_f32_at(body, offset + 8, "VOL latitude")?,
                    lon: read_f32_at(body, offset + 12, "VOL longitude")?,
                });
            }
            b"REF" => moments.reflectivity = Some(parse_moment(body, offset)?),
            b"VEL" => moments.velocity = Some(parse_moment(body, offset)?),
            b"SW " => moments.spectrum_width = Some(parse_moment(body, offset)?),
            b"ZDR" => moments.zdr = Some(parse_moment(body, offset)?),
            b"PHI" => moments.phi = Some(parse_moment(body, offset)?),
            b"RHO" => moments.rho = Some(parse_moment(body, offset)?),
            _ => {}
        }
    }

    let volume = volume.ok_or_else(|| Error::Format("radial has no VOL block".to_string()))?;

    Ok(Radial {
        header: RadialHeader {
            azimuth_deg,
            elevation_deg,
            elevation_number,
            azimuth_resolution_deg: if resolution_code == 1 { 0.5 } else { 1.0 },
        },
        volume,
        moments,
    })
}

fn parse_moment(body: &[u8], offset: usize) -> Result<Moment> {
    let gates = read_u16_at(body, offset + 8, "moment gate count")?;
    let range_m = read_u16_at(body, offset + 10, "moment range")?;
    let interval_m = read_u16_at(body, offset + 12, "moment sample interval")?;
    let word_size = *body.get(offset + 19).ok_or_else(|| {
        Error::Format(format!(
            "moment at byte {offset} truncated before word size"
        ))
    })?;
    let scale = read_f32_at(body, offset + 20, "moment scale")?;
    let offset_value = read_f32_at(body, offset + 24, "moment offset")?;

    let data_start = offset + 28;
    let data_len = gates as usize * word_size as usize / 8;
    let data = body
        .get(data_start..data_start + data_len)
        .ok_or_else(|| {
            Error::Format(format!(
                "moment data at byte {data_start} of length {data_len} exceeds radial length {}",
                body.len()
            ))
        })?
        .to_vec();

    Ok(Moment {
        gates,
        range_m,
        interval_m,
        word_size,
        scale,
        offset: offset_value,
        data,
    })
}

fn read_u16_at(data: &[u8], offset: usize, what: &str) -> Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| Error::Format(format!("{what} at byte {offset} truncated")))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32_at(data: &[u8], offset: usize, what: &str) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| Error::Format(format!("{what} at byte {offset} truncated")))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_i32_at(data: &[u8], offset: usize, what: &str) -> Result<i32> {
    Ok(read_u32_at(data, offset, what)? as i32)
}

fn read_f32_at(data: &[u8], offset: usize, what: &str) -> Result<f32> {
    Ok(f32::from_bits(read_u32_at(data, offset, what)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        let volume = read_volume(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/nexrad/ktlx-mini.ar2"
        )))
        .expect("fixture parses");
        assert_eq!(volume.scans.len(), 1);

        let radials = volume.scans.get(&1).expect("elevation 1");
        assert_eq!(radials.len(), 20);

        for radial in radials {
            let reflectivity = radial.moments.reflectivity.as_ref().expect("REF moment");
            assert_eq!(reflectivity.gates, 1832);
            assert_eq!(reflectivity.scale, 2.0);
            assert_eq!(reflectivity.offset, 66.0);
        }

        let first = &radials[0];
        assert!(first.moments.zdr.is_some());
        assert!(first.moments.phi.is_some());
        assert!(first.moments.rho.is_some());
        assert_eq!(first.header.elevation_number, 1);
        assert_eq!(first.header.azimuth_resolution_deg, 0.5);
    }

    #[test]
    fn truncated_input() {
        assert!(parse_volume(&[0u8; 10]).is_err());

        let mut bytes = vec![0u8; VOLUME_HEADER_LEN];
        bytes.extend_from_slice(&4096i32.to_be_bytes());
        bytes.extend_from_slice(&[0u8; 16]);
        assert!(parse_volume(&bytes).is_err());
    }

    #[test]
    fn scaled_rejects_unsupported_word_size() {
        let moment = Moment {
            gates: 1,
            range_m: 0,
            interval_m: 0,
            word_size: 32,
            scale: 1.0,
            offset: 0.0,
            data: vec![0; 4],
        };
        assert!(matches!(moment.scaled(), Err(Error::Unsupported(_))));
    }
}
