use crate::nexrad::archive2::{Product, Radial};
use crate::nexrad::error::{Error, Result};
use crate::nexrad::geo::Ortho;

/// One radar gate rendered as a closed polygon ring of `(longitude, latitude)`.
pub struct Bin {
    pub ring: [(f64, f64); 5],
    pub value: f32,
}

pub fn build_bins(
    radial: &Radial,
    product: Product,
    ortho: &Ortho,
    minimum: Option<f32>,
    maximum: Option<f32>,
) -> Result<Vec<Bin>> {
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
    let values = moment.scaled()?;

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

    let mut bins = Vec::new();

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
            bins.push(Bin {
                ring: [points[0], points[1], points[3], points[2], points[0]],
                value,
            });
        }

        range = next_range;
    }

    Ok(bins)
}

/// Coordinate precision of the pre-fusion output (`[{lon:.4},{lat:.4}]`).
///
/// Formatting is the rounding rule, so the frame path rounds coordinates
/// through the same formatting and rasterizes exactly the numbers the two-step
/// output carried.
pub fn round_coordinate(value: f64) -> f64 {
    format!("{value:.4}")
        .parse()
        .expect("a formatted float parses back")
}

/// Value precision of the pre-fusion output (`"value":{value:.1}`).
pub fn round_value(value: f32) -> f64 {
    format!("{value:.1}")
        .parse()
        .expect("a formatted float parses back")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nexrad::archive2::{Moment, Moments, RadialHeader, VolumeBlock};

    fn synthetic_radial() -> Radial {
        let moment = Moment {
            gates: 5,
            range_m: 1000,
            interval_m: 250,
            word_size: 8,
            scale: 1.0,
            offset: 0.0,
            data: vec![0, 1, 8, 10, 12],
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
        ];
        for value in coordinates {
            assert_eq!(
                format!("{:.4}", round_coordinate(value)),
                format!("{value:.4}")
            );
        }

        let values = [10.0f32, 10.5, 41.45, 254.5, 300.0, -0.04, 998.0];
        for value in values {
            assert_eq!(format!("{:.1}", round_value(value)), format!("{value:.1}"));
        }
    }

    #[test]
    fn filters_gates() {
        let radial = synthetic_radial();
        let ortho = Ortho::new(35.0, -97.0);
        let bins = build_bins(&radial, Product::Ref, &ortho, Some(10.0), None).expect("bins");

        assert_eq!(bins.len(), 2);
        assert_eq!(bins[0].value, 10.0);
        assert_eq!(bins[1].value, 12.0);
        for bin in &bins {
            assert_eq!(bin.ring[0], bin.ring[4]);
            for (lon, lat) in bin.ring {
                assert!(lon.is_finite() && lat.is_finite());
            }
        }
    }
}
