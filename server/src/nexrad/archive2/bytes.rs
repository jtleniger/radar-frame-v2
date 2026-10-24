//! Bounds-checked big-endian scalar reads.

use crate::nexrad::error::{Error, Result};

pub(super) fn read_u16_at(data: &[u8], offset: usize, what: &str) -> Result<u16> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| Error::Format(format!("{what} offset overflow")))?;
    let bytes = data
        .get(offset..end)
        .ok_or_else(|| Error::Format(format!("{what} at byte {offset} truncated")))?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

pub(super) fn read_u32_at(data: &[u8], offset: usize, what: &str) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| Error::Format(format!("{what} offset overflow")))?;
    let bytes = data
        .get(offset..end)
        .ok_or_else(|| Error::Format(format!("{what} at byte {offset} truncated")))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(super) fn read_i32_at(data: &[u8], offset: usize, what: &str) -> Result<i32> {
    Ok(read_u32_at(data, offset, what)? as i32)
}

pub(super) fn read_f32_at(data: &[u8], offset: usize, what: &str) -> Result<f32> {
    Ok(f32::from_bits(read_u32_at(data, offset, what)?))
}
