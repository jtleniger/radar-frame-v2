//! Walk an LDM record's message slots and collect Message 31 radials.

use std::collections::BTreeMap;

use super::radial::parse_radial;
use super::{Radial, LEGACY_MESSAGE_SLOT_LEN, MESSAGE31_HEADER_LEN, MESSAGE_HEADER_LEN};
use crate::nexrad::error::{Error, Result};

pub(super) fn parse_messages(
    scans: &mut BTreeMap<u8, Vec<Radial>>,
    data: &[u8],
    base: usize,
) -> Result<()> {
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
