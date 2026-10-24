use std::collections::BTreeMap;
use std::io::Read;
use std::ops::Range;
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
/// Upper bound on a downloaded Archive II object. Real volumes are far below
/// this; the cap keeps a corrupt `Content-Length` from filling the disk.
pub const MAX_VOLUME_BYTES: u64 = 256 * 1024 * 1024;
/// Upper bound on a decompressed volume or LDM record. Wrapping decoders in a
/// limit stops a decompression bomb from exhausting memory.
const MAX_DECOMPRESSED_BYTES: usize = 256 * 1024 * 1024;
const MAX_LDM_RECORD_BYTES: usize = 64 * 1024 * 1024;

pub struct Volume {
    pub scans: BTreeMap<u8, Vec<Radial>>,
    /// The decompressed stream that every [`Moment::data`] range indexes.
    payload: Vec<u8>,
}

impl Volume {
    pub(crate) fn from_payload(scans: BTreeMap<u8, Vec<Radial>>, payload: Vec<u8>) -> Self {
        Self { scans, payload }
    }

    /// The gate bytes of `moment`, borrowed from the volume's payload.
    pub fn moment_data(&self, moment: &Moment) -> &[u8] {
        &self.payload[moment.data.clone()]
    }

    /// Lazily decoded values of `moment`, in gate order.
    pub fn values(&self, moment: &Moment) -> Result<Values<'_>> {
        moment.values(self.moment_data(moment))
    }
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
    /// Gate bytes, as a range into the owning [`Volume`]'s payload.
    pub data: Range<usize>,
}

/// Lazily decoded gate values, in gate order.
pub struct Values<'a> {
    chunks: std::slice::ChunksExact<'a, u8>,
    word_size: u8,
    scale: f32,
    offset: f32,
}

impl Moment {
    /// Decode `data` as this moment's gates, without copying it.
    pub fn values<'a>(&self, data: &'a [u8]) -> Result<Values<'a>> {
        if self.word_size != 8 && self.word_size != 16 {
            return Err(Error::Unsupported(format!(
                "data word size {}",
                self.word_size
            )));
        }

        let expected = (self.gates as usize)
            .checked_mul(self.word_size as usize)
            .ok_or_else(|| Error::Format("moment data length overflow".to_string()))?
            / 8;
        if data.len() != expected {
            return Err(Error::Format(format!(
                "moment data length {} does not match {} gates of {} bits",
                data.len(),
                self.gates,
                self.word_size
            )));
        }

        Ok(Values {
            chunks: data.chunks_exact(self.word_size as usize / 8),
            word_size: self.word_size,
            scale: self.scale,
            offset: self.offset,
        })
    }
}

impl Iterator for Values<'_> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let chunk = self.chunks.next()?;
        let value = if self.word_size == 8 {
            chunk[0] as u16
        } else {
            u16::from_be_bytes([chunk[0], chunk[1]])
        };
        Some(match value {
            0 => 999.0,
            1 => 998.0,
            value if self.scale == 0.0 => value as f32,
            value => (value as f32 - self.offset) / self.scale,
        })
    }
}

pub fn read_volume(path: &Path) -> Result<Volume> {
    let size = std::fs::metadata(path)
        .map_err(|err| Error::Format(format!("{}: {err}", path.display())))?
        .len();
    if size > MAX_VOLUME_BYTES {
        return Err(Error::Format(format!(
            "{}: {size} bytes exceeds the {MAX_VOLUME_BYTES}-byte volume limit",
            path.display()
        )));
    }

    let bytes =
        std::fs::read(path).map_err(|err| Error::Format(format!("{}: {err}", path.display())))?;
    parse_volume(bytes)
}

/// Parse a volume from its raw bytes. The returned [`Volume`] owns `bytes`
/// (or the decompressed stream), and every moment borrows a range of it.
pub fn parse_volume(bytes: Vec<u8>) -> Result<Volume> {
    parse_volume_capped(bytes, MAX_DECOMPRESSED_BYTES, MAX_LDM_RECORD_BYTES)
}

fn parse_volume_capped(
    mut payload: Vec<u8>,
    max_decompressed: usize,
    max_record: usize,
) -> Result<Volume> {
    if payload.starts_with(&[0x1f, 0x8b]) {
        payload = read_capped(
            flate2::read::GzDecoder::new(payload.as_slice()),
            max_decompressed,
            "decompressed volume",
        )?;
    }

    if payload.len() > max_decompressed {
        return Err(Error::Format(format!(
            "volume of {} bytes exceeds the {max_decompressed}-byte limit",
            payload.len()
        )));
    }

    if payload.len() < VOLUME_HEADER_LEN {
        return Err(Error::Format(format!(
            "volume header truncated: {} bytes",
            payload.len()
        )));
    }

    let mut scans = BTreeMap::new();
    let mut pos = VOLUME_HEADER_LEN;
    // Decompressed LDM records are appended past this point; the LDM walk must
    // stay inside the original stream or it would treat those bytes as records.
    let stream_end = payload.len();

    while stream_end.saturating_sub(pos) >= 4 {
        let size = read_i32_at(&payload, pos, "LDM record size")?;
        let size_start = pos;
        pos += 4;

        if size == 0 {
            // Older files have no LDM records: the remaining stream is a plain
            // sequence of messages starting right after the zero control word.
            parse_messages(&mut scans, &payload[size_start..stream_end], size_start)?;
            break;
        }

        let length = size.unsigned_abs() as usize;
        if length > max_record {
            return Err(Error::Format(format!(
                "LDM record at byte {size_start} is {length} bytes, above the {max_record}-byte limit"
            )));
        }
        let end = pos
            .checked_add(length)
            .ok_or_else(|| Error::Format(format!("LDM record at byte {size_start} overflows")))?;
        if end > stream_end {
            return Err(Error::Format(format!(
                "LDM record at byte {size_start} exceeds file length"
            )));
        }

        if payload[pos..end].starts_with(b"BZh") {
            let out = read_capped(
                bzip2::read::MultiBzDecoder::new(&payload[pos..end]),
                max_decompressed,
                "decompressed LDM record",
            )?;
            // Keep the record's bytes in the payload so moment ranges can
            // point at them instead of copying each moment.
            let base = payload.len();
            payload.extend_from_slice(&out);
            parse_messages(&mut scans, &payload[base..], base)?;
        } else {
            parse_messages(&mut scans, &payload[pos..end], pos)?;
        }
        pos = end;
    }

    Ok(Volume::from_payload(scans, payload))
}

/// Read at most `limit` bytes, refusing a stream that expands past it.
fn read_capped<R: Read>(reader: R, limit: usize, what: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(Error::Io)?;
    if out.len() > limit {
        return Err(Error::Format(format!(
            "{what} exceeds the {limit}-byte limit"
        )));
    }
    Ok(out)
}

fn parse_messages(scans: &mut BTreeMap<u8, Vec<Radial>>, data: &[u8], base: usize) -> Result<()> {
    let mut start = 0;

    while data.len().saturating_sub(start) >= MESSAGE_HEADER_LEN {
        let message_type = data[start + 15];

        if message_type != 31 {
            if data.len() - start < LEGACY_MESSAGE_SLOT_LEN {
                break;
            }
            start = start
                .checked_add(LEGACY_MESSAGE_SLOT_LEN)
                .ok_or_else(|| Error::Format("message offset overflow".to_string()))?;
            continue;
        }

        let body_start = start
            .checked_add(MESSAGE_HEADER_LEN)
            .ok_or_else(|| Error::Format("message offset overflow".to_string()))?;
        let header_end = body_start
            .checked_add(MESSAGE31_HEADER_LEN)
            .ok_or_else(|| Error::Format("message offset overflow".to_string()))?;
        if header_end > data.len() {
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

        let end = body_start
            .checked_add(radial_length)
            .ok_or_else(|| Error::Format("message offset overflow".to_string()))?;
        if end > data.len() {
            return Err(Error::Format(format!(
                "message 31 radial at byte {start} exceeds record length"
            )));
        }

        let radial_base = base
            .checked_add(body_start)
            .ok_or_else(|| Error::Format("message offset overflow".to_string()))?;
        let radial = parse_radial(&data[body_start..end], radial_base)?;
        scans
            .entry(radial.header.elevation_number)
            .or_default()
            .push(radial);
        start = end;
    }

    Ok(())
}

fn parse_radial(body: &[u8], base: usize) -> Result<Radial> {
    let azimuth_deg = read_f32_at(body, 12, "message 31 azimuth")?;
    let resolution_code = body[20];
    let elevation_number = body[22];
    let elevation_deg = read_f32_at(body, 24, "message 31 elevation")?;
    let data_block_count = read_u16_at(body, 30, "message 31 data block count")? as usize;

    let pointer_start = MESSAGE31_HEADER_LEN;
    let pointer_bytes = data_block_count
        .checked_mul(4)
        .ok_or_else(|| Error::Format("data block pointer count overflow".to_string()))?;
    let pointer_end = pointer_start
        .checked_add(pointer_bytes)
        .ok_or_else(|| Error::Format("data block pointer offset overflow".to_string()))?;
    if pointer_end > body.len() {
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
    let mut probe = pointer_end;
    let mut extra = 0;
    while extra < 20 && body.len().saturating_sub(probe) >= 4 {
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
        let name_start = offset
            .checked_add(1)
            .ok_or_else(|| Error::Format("data block offset overflow".to_string()))?;
        let name_end = offset
            .checked_add(4)
            .ok_or_else(|| Error::Format("data block offset overflow".to_string()))?;
        let name = body.get(name_start..name_end).ok_or_else(|| {
            Error::Format(format!(
                "data block pointer {offset} exceeds radial length {}",
                body.len()
            ))
        })?;

        match name {
            b"VOL" => {
                let lat_offset = offset
                    .checked_add(8)
                    .ok_or_else(|| Error::Format("VOL offset overflow".to_string()))?;
                let lon_offset = offset
                    .checked_add(12)
                    .ok_or_else(|| Error::Format("VOL offset overflow".to_string()))?;
                volume = Some(VolumeBlock {
                    lat: read_f32_at(body, lat_offset, "VOL latitude")?,
                    lon: read_f32_at(body, lon_offset, "VOL longitude")?,
                });
            }
            b"REF" => moments.reflectivity = Some(parse_moment(body, offset, base)?),
            b"VEL" => moments.velocity = Some(parse_moment(body, offset, base)?),
            b"SW " => moments.spectrum_width = Some(parse_moment(body, offset, base)?),
            b"ZDR" => moments.zdr = Some(parse_moment(body, offset, base)?),
            b"PHI" => moments.phi = Some(parse_moment(body, offset, base)?),
            b"RHO" => moments.rho = Some(parse_moment(body, offset, base)?),
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

fn parse_moment(body: &[u8], offset: usize, base: usize) -> Result<Moment> {
    let at = |delta: usize| {
        offset
            .checked_add(delta)
            .ok_or_else(|| Error::Format("moment offset overflow".to_string()))
    };

    let gates = read_u16_at(body, at(8)?, "moment gate count")?;
    let range_m = read_u16_at(body, at(10)?, "moment range")?;
    let interval_m = read_u16_at(body, at(12)?, "moment sample interval")?;
    let word_size = *body.get(at(19)?).ok_or_else(|| {
        Error::Format(format!(
            "moment at byte {offset} truncated before word size"
        ))
    })?;
    let scale = read_f32_at(body, at(20)?, "moment scale")?;
    let offset_value = read_f32_at(body, at(24)?, "moment offset")?;

    let data_start = at(28)?;
    let data_len = (gates as usize)
        .checked_mul(word_size as usize)
        .ok_or_else(|| Error::Format("moment data length overflow".to_string()))?
        / 8;
    let data_end = data_start
        .checked_add(data_len)
        .ok_or_else(|| Error::Format("moment data offset overflow".to_string()))?;
    body.get(data_start..data_end).ok_or_else(|| {
        Error::Format(format!(
            "moment data at byte {data_start} of length {data_len} exceeds radial length {}",
            body.len()
        ))
    })?;

    let absolute_start = base
        .checked_add(data_start)
        .ok_or_else(|| Error::Format("moment data offset overflow".to_string()))?;
    let absolute_end = base
        .checked_add(data_end)
        .ok_or_else(|| Error::Format("moment data offset overflow".to_string()))?;

    Ok(Moment {
        gates,
        range_m,
        interval_m,
        word_size,
        scale,
        offset: offset_value,
        data: absolute_start..absolute_end,
    })
}

fn read_u16_at(data: &[u8], offset: usize, what: &str) -> Result<u16> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| Error::Format(format!("{what} offset overflow")))?;
    let bytes = data
        .get(offset..end)
        .ok_or_else(|| Error::Format(format!("{what} at byte {offset} truncated")))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32_at(data: &[u8], offset: usize, what: &str) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| Error::Format(format!("{what} offset overflow")))?;
    let bytes = data
        .get(offset..end)
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
        assert!(parse_volume(vec![0u8; 10]).is_err());

        let mut bytes = vec![0u8; VOLUME_HEADER_LEN];
        bytes.extend_from_slice(&4096i32.to_be_bytes());
        bytes.extend_from_slice(&[0u8; 16]);
        assert!(parse_volume(bytes).is_err());
    }

    #[test]
    fn rejects_unsupported_word_size() {
        let payload = [0u8; 4];
        let moment = Moment {
            gates: 1,
            range_m: 0,
            interval_m: 0,
            word_size: 32,
            scale: 1.0,
            offset: 0.0,
            data: 0..4,
        };
        assert!(matches!(
            moment.values(&payload),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn decodes_eight_bit_gates_in_place() {
        let payload = [0u8, 1, 8, 10, 12];
        let moment = Moment {
            gates: 5,
            range_m: 0,
            interval_m: 0,
            word_size: 8,
            scale: 1.0,
            offset: 0.0,
            data: 0..5,
        };

        let values: Vec<f32> = moment.values(&payload).unwrap().collect();
        assert_eq!(values, vec![999.0, 998.0, 8.0, 10.0, 12.0]);
    }

    #[test]
    fn decodes_sixteen_bit_gates_with_scale_and_offset() {
        let payload = [0u8, 0, 0, 1, 0x02, 0x00];
        let moment = Moment {
            gates: 3,
            range_m: 0,
            interval_m: 0,
            word_size: 16,
            scale: 2.0,
            offset: 66.0,
            data: 0..6,
        };

        let values: Vec<f32> = moment.values(&payload).unwrap().collect();
        assert_eq!(values, vec![999.0, 998.0, 223.0]);
    }

    #[test]
    fn rejects_a_gzip_volume_that_expands_past_the_limit() {
        use std::io::Write;

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&[0u8; 128]).unwrap();
        let compressed = encoder.finish().unwrap();

        let Err(error) = parse_volume_capped(compressed, 64, 64) else {
            panic!("expected a decompression limit error");
        };
        let error = error.to_string();
        assert!(error.contains("exceeds the 64-byte limit"), "{error}");
    }

    #[test]
    fn rejects_an_ldm_record_above_the_limit() {
        let mut bytes = vec![0u8; VOLUME_HEADER_LEN];
        bytes.extend_from_slice(&64i32.to_be_bytes());
        bytes.extend_from_slice(&[0u8; 8]);

        let Err(error) = parse_volume_capped(bytes, 1024, 32) else {
            panic!("expected an LDM record limit error");
        };
        let error = error.to_string();
        assert!(error.contains("above the 32-byte limit"), "{error}");
    }

    #[test]
    fn rejects_a_plain_volume_above_the_limit() {
        let Err(error) = parse_volume_capped(vec![0u8; 64], 32, 32) else {
            panic!("expected a volume limit error");
        };
        let error = error.to_string();
        assert!(error.contains("exceeds the 32-byte limit"), "{error}");
    }
}
