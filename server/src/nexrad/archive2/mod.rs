use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use crate::nexrad::error::{Error, Result};

mod bytes;
mod message;
mod moment;
mod parse;
mod radial;

/// Volume header record length at the start of every Archive II volume.
pub(super) const VOLUME_HEADER_LEN: usize = 24;
/// Legacy CTM header (12 bytes) followed by the message header (16 bytes).
pub(super) const MESSAGE_HEADER_LEN: usize = 28;
/// Fixed slot size for messages other than Message 31.
pub(super) const LEGACY_MESSAGE_SLOT_LEN: usize = 2432;
/// Message 31 header length, including the data block count.
pub(super) const MESSAGE31_HEADER_LEN: usize = 32;
/// Upper bound on a downloaded Archive II object. Real volumes are far below
/// this; the cap keeps a corrupt `Content-Length` from filling the disk.
pub const MAX_VOLUME_BYTES: u64 = 256 * 1024 * 1024;
/// Upper bound on a decompressed volume or LDM record. Wrapping decoders in a
/// limit stops a decompression bomb from exhausting memory.
pub(super) const MAX_DECOMPRESSED_BYTES: usize = 256 * 1024 * 1024;
pub(super) const MAX_LDM_RECORD_BYTES: usize = 64 * 1024 * 1024;

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
    parse::parse_volume_capped(bytes, MAX_DECOMPRESSED_BYTES, MAX_LDM_RECORD_BYTES)
}

#[cfg(test)]
mod tests {
    use super::parse::parse_volume_capped;
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
