//! Moment (data block) headers and gate ranges.

use super::bytes::{read_f32_at, read_u16_at};
use super::Moment;
use crate::nexrad::error::{Error, Result};

pub(super) fn parse_moment(body: &[u8], offset: usize, base: usize) -> Result<Moment> {
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
