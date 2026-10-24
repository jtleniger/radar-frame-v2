//! Message 31 radial and data-block parsing.

use super::bytes::{read_f32_at, read_u16_at, read_u32_at};
use super::moment::parse_moment;
use super::{Moments, Radial, RadialHeader, VolumeBlock, MESSAGE31_HEADER_LEN};
use crate::nexrad::error::{Error, Result};

pub(super) fn parse_radial(body: &[u8], base: usize) -> Result<Radial> {
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
