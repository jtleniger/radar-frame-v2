use crate::nexrad::archive2::{Product, Radial, Volume};
use crate::nexrad::error::{Error, Result};
use crate::nexrad::geo::Ortho;

/// Project one radial's gates and hand each one to `emit`, without building a
/// per-radial `Vec` of bins. Returns the number of gates emitted.
///
/// The ring is the four corners in counter-clockwise order; the polygon is
/// implicitly closed, so no duplicate fifth point is needed.
pub fn for_each_bin<F>(
    volume: &Volume,
    radial: &Radial,
    product: Product,
    ortho: &Ortho,
    minimum: Option<f32>,
    maximum: Option<f32>,
    mut emit: F,
) -> Result<usize>
where
    F: FnMut(&[(f64, f64); 4], f32),
{
    // The gate geometry always comes from the REF moment, whatever the product.
    let reference = radial
        .moments
        .reflectivity
        .as_ref()
        .ok_or_else(|| Error::Format("REF data moment missing".to_string()))?;
    let moment = radial
        .moments
        .get(product)
        .ok_or_else(|| Error::Format(format!("{} data moment missing", product.as_str())))?;
    let values = volume.values(moment)?;

    let phi = ((90.0f32 - radial.header.elevation_deg) * (std::f32::consts::PI / 180.0)) as f64;
    let mut theta_deg = 90.0f32 - radial.header.azimuth_deg;
    if theta_deg < 0.0 {
        theta_deg += 360.0;
    }
    let theta = (theta_deg * (std::f32::consts::PI / 180.0)) as f64;
    let half_azimuth = radial.header.azimuth_resolution_deg * (std::f64::consts::PI / 360.0);

    let sinphi = phi.sin();
    let interval = reference.interval_m as f64;
    let mut range = reference.range_m as f64;

    let mut bins = 0usize;

    for value in values {
        let next_range = range + interval;

        if value == 998.0
            || value == 999.0
            || minimum.is_some_and(|minimum| value < minimum)
            || maximum.is_some_and(|maximum| value > maximum)
        {
            range = next_range;
            continue;
        }

        let corners = [
            (range, theta + half_azimuth),
            (range, theta - half_azimuth),
            (next_range, theta + half_azimuth),
            (next_range, theta - half_azimuth),
        ];

        let mut points = [(0.0, 0.0); 4];
        let mut projected = true;
        for (point, (distance, angle)) in points.iter_mut().zip(corners) {
            let x = distance * sinphi * angle.cos();
            let y = distance * sinphi * angle.sin();
            match ortho.inverse(x, y) {
                Some(coordinate) => *point = coordinate,
                None => {
                    projected = false;
                    break;
                }
            }
        }

        if projected {
            // Same corner order as the pre-fusion ring: range+azimuth,
            // range-azimuth, next-azimuth, next+azimuth, then back.
            emit(&[points[0], points[1], points[3], points[2]], value);
            bins += 1;
        }

        range = next_range;
    }

    Ok(bins)
}

/// Coordinate precision of the pre-fusion output (`[{lon:.4},{lat:.4}]`).
pub fn round_coordinate(value: f64) -> f64 {
    round_fixed(value, 4)
}

/// Value precision of the pre-fusion output (`"value":{value:.1}`).
pub fn round_value(value: f32) -> f64 {
    round_fixed(f64::from(value), 1)
}

/// Round `value` to `decimals` decimal places exactly like `{value:.N}`
/// followed by parsing the text back, but without formatting or allocating.
///
/// The ties-to-even rule is applied to the exact binary value. The fast path
/// covers every value whose scaled magnitude fits in 53 bits; anything larger
/// (and every non-finite value) falls back to the formatter so the result is
/// still bit-identical.
fn round_fixed(value: f64, decimals: usize) -> f64 {
    if !value.is_finite() {
        return format!("{value:.decimals$}")
            .parse()
            .expect("a formatted float parses back");
    }

    let negative = value.is_sign_negative();
    let rounded = round_fixed_positive(value.abs(), decimals).unwrap_or_else(|| {
        format!("{:.decimals$}", value.abs())
            .parse()
            .expect("a formatted float parses back")
    });
    if negative {
        -rounded
    } else {
        rounded
    }
}

/// `Some` when the scaled magnitude fits exactly in an `f64`; `None` asks the
/// caller to use the formatter fallback.
fn round_fixed_positive(magnitude: f64, decimals: usize) -> Option<f64> {
    let bits = magnitude.to_bits();
    let exponent_bits = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mantissa, exponent) = if exponent_bits == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1u64 << 52), exponent_bits - 1075)
    };
    if mantissa == 0 {
        return Some(0.0);
    }

    // value * 10^decimals = mantissa * 5^decimals * 2^(exponent + decimals).
    let pow5 = 5u128.pow(decimals as u32);
    let numerator = mantissa as u128 * pow5;
    let shift = exponent + decimals as i32;

    let scaled = if shift >= 0 {
        let shift = shift as u32;
        if shift >= 128 || numerator > u128::MAX >> shift {
            return None;
        }
        numerator << shift
    } else {
        let shift = (-shift) as u32;
        if shift >= 128 {
            if shift == 128 && numerator > 1u128 << 127 {
                1
            } else {
                0
            }
        } else {
            let quotient = numerator >> shift;
            let remainder = numerator & ((1u128 << shift) - 1);
            let half = 1u128 << (shift - 1);
            if remainder > half || (remainder == half && quotient & 1 == 1) {
                quotient + 1
            } else {
                quotient
            }
        }
    };

    // `scaled as f64` must be exact, and the division below correctly rounded:
    // together they produce the same `f64` as parsing the formatted decimal.
    if scaled > (1u128 << 53) {
        return None;
    }
    let divisor = 10u128.pow(decimals as u32) as f64;
    Some(scaled as f64 / divisor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nexrad::archive2::{Moment, Moments, RadialHeader, VolumeBlock};
    use std::collections::BTreeMap;

    fn synthetic_radial() -> Radial {
        let moment = Moment {
            gates: 5,
            range_m: 1000,
            interval_m: 250,
            word_size: 8,
            scale: 1.0,
            offset: 0.0,
            data: 0..5,
        };

        Radial {
            header: RadialHeader {
                azimuth_deg: 0.0,
                elevation_deg: 0.0,
                elevation_number: 1,
                azimuth_resolution_deg: 1.0,
            },
            volume: VolumeBlock {
                lat: 35.0,
                lon: -97.0,
            },
            moments: Moments {
                reflectivity: Some(moment),
                ..Moments::default()
            },
        }
    }

    fn next_random(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn rounding_matches_the_serialized_precision() {
        let coordinates = [
            -97.85,
            -97.14999999999999,
            -97.123456789,
            35.1615,
            -0.00004,
            -0.00005,
            -0.00006,
            1.0000499999999,
            1.23456789e-9,
            0.03125,
            0.15625,
            1.03125,
            -0.03125,
            2.5,
            1e-300,
            -1e-300,
            900_000_000_000.0,
        ];
        for value in coordinates {
            assert_eq!(
                round_coordinate(value).to_bits(),
                format!("{value:.4}").parse::<f64>().unwrap().to_bits(),
                "{value:?}"
            );
        }

        let values = [
            10.0f32, 10.5, 41.45, 254.5, 300.0, -0.04, 998.0, 0.25, 1.25, 0.05, 0.15,
        ];
        for value in values {
            assert_eq!(
                round_value(value).to_bits(),
                format!("{value:.1}").parse::<f64>().unwrap().to_bits(),
                "{value:?}"
            );
        }
    }

    #[test]
    fn rounding_matches_the_formatter_over_random_bits() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..20_000 {
            let value = f64::from_bits(next_random(&mut state));
            if !value.is_finite() {
                continue;
            }
            let value = value.clamp(-1e9, 1e9);
            assert_eq!(
                round_coordinate(value).to_bits(),
                format!("{value:.4}").parse::<f64>().unwrap().to_bits(),
                "{value:?}"
            );
        }

        for _ in 0..20_000 {
            let value = f32::from_bits(next_random(&mut state) as u32);
            if !value.is_finite() {
                continue;
            }
            let value = value.clamp(-1e9, 1e9);
            assert_eq!(
                round_value(value).to_bits(),
                format!("{value:.1}").parse::<f64>().unwrap().to_bits(),
                "{value:?}"
            );
        }
    }

    #[test]
    fn filters_gates() {
        let volume = Volume::from_payload(
            BTreeMap::from([(1, vec![synthetic_radial()])]),
            vec![0, 1, 8, 10, 12],
        );
        let radial = &volume.scans[&1][0];
        let ortho = Ortho::new(35.0, -97.0);

        let mut bins = Vec::new();
        let count = for_each_bin(
            &volume,
            radial,
            Product::Ref,
            &ortho,
            Some(10.0),
            None,
            |ring, value| bins.push((*ring, value)),
        )
        .expect("bins");

        assert_eq!(count, 2);
        assert_eq!(bins.len(), 2);
        assert_eq!(bins[0].1, 10.0);
        assert_eq!(bins[1].1, 12.0);
        for (ring, _) in &bins {
            for (lon, lat) in ring {
                assert!(lon.is_finite() && lat.is_finite());
            }
        }
    }
}
