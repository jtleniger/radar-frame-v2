//! LDM record walk and message framing.

use std::collections::BTreeMap;
use std::io::Read;

use super::bytes::read_i32_at;
use super::message::parse_messages;
use super::{Volume, VOLUME_HEADER_LEN};
use crate::nexrad::error::{Error, Result};

pub(super) fn parse_volume_capped(
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
