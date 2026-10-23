use std::f64::consts::{FRAC_PI_2, PI};

/// WGS84 semi-major axis in meters.
const A: f64 = 6_378_137.0;
/// WGS84 flattening.
const F: f64 = 1.0 / 298.257223563;
/// WGS84 first eccentricity squared.
const ES: f64 = 2.0 * F - F * F;
const ONE_ES: f64 = 1.0 - ES;
const EPS10: f64 = 1.0e-10;

pub enum Mode {
    NorthPole,
    SouthPole,
    Equatorial,
    Oblique,
}

/// Ellipsoidal orthographic projection in the WGS84 ellipsoid.
pub struct Ortho {
    lat0: f64,
    lon0: f64,
    sinph0: f64,
    cosph0: f64,
    nu0: f64,
    y_shift: f64,
    y_scale: f64,
    mode: Mode,
}

impl Ortho {
    pub fn new(lat0_deg: f64, lon0_deg: f64) -> Self {
        let lat0 = lat0_deg.to_radians();
        let lon0 = lon0_deg.to_radians();
        let sinph0 = lat0.sin();
        let cosph0 = lat0.cos();

        let mode = if (lat0.abs() - FRAC_PI_2).abs() <= EPS10 {
            if lat0 < 0.0 {
                Mode::SouthPole
            } else {
                Mode::NorthPole
            }
        } else if lat0.abs() > EPS10 {
            Mode::Oblique
        } else {
            Mode::Equatorial
        };

        let nu0 = 1.0 / (1.0 - ES * sinph0 * sinph0).sqrt();

        Ortho {
            lat0,
            lon0,
            sinph0,
            cosph0,
            nu0,
            y_shift: ES * nu0 * sinph0 * cosph0,
            y_scale: 1.0 / (1.0 - ES * cosph0 * cosph0).sqrt(),
            mode,
        }
    }

    /// Inverse projection from projected meters to `(longitude, latitude)` degrees.
    pub fn inverse(&self, x_m: f64, y_m: f64) -> Option<(f64, f64)> {
        let x = x_m / A;
        let y = y_m / A;

        match self.mode {
            Mode::NorthPole | Mode::SouthPole => self.polar_inverse(x, y),
            Mode::Equatorial => self.equatorial_inverse(x, y),
            Mode::Oblique => self.oblique_inverse(x, y),
        }
    }

    fn polar_inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let north = matches!(self.mode, Mode::NorthPole);
        let rh2 = x * x + y * y;

        let phi = if rh2 >= 1.0 - 1e-15 {
            if rh2 - 1.0 > EPS10 {
                return None;
            }
            0.0
        } else {
            (rh2 * ONE_ES / (1.0 - ES * rh2)).sqrt().acos() * if north { 1.0 } else { -1.0 }
        };

        let lam = x.atan2(if north { -y } else { y });
        Some(self.finish(phi, lam))
    }

    fn equatorial_inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let b = A * (1.0 - F);
        if x * x + (y * A / b).powi(2) > 1.0 + 1e-11 {
            return None;
        }

        let sinphi2 = if y == 0.0 {
            0.0
        } else {
            1.0 / (((1.0 - ES) / y).powi(2) + ES)
        };

        if sinphi2 > 1.0 - 1e-11 {
            let phi = FRAC_PI_2 * if y > 0.0 { 1.0 } else { -1.0 };
            return Some(self.finish(phi, 0.0));
        }

        let phi = sinphi2.sqrt().asin() * if y > 0.0 { 1.0 } else { -1.0 };
        let sinlam = x * ((1.0 - ES * sinphi2) / (1.0 - sinphi2)).sqrt();
        let lam = if sinlam.abs() - 1.0 > -1e-15 {
            FRAC_PI_2 * if x > 0.0 { 1.0 } else { -1.0 }
        } else {
            sinlam.asin()
        };
        Some(self.finish(phi, lam))
    }

    fn oblique_inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let yr = (y - self.y_shift) / self.y_scale;
        if x * x + yr * yr > 1.0 + 1e-11 {
            return None;
        }

        let rh = (x * x + yr * yr).sqrt();
        let sinc = rh.min(1.0);
        let cosc = (1.0 - sinc * sinc).sqrt();

        let (mut phi, mut lam) = if rh.abs() <= EPS10 {
            (self.lat0, 0.0)
        } else {
            let phi = cosc * self.sinph0 + yr * sinc * self.cosph0 / rh;
            let yy = (cosc - self.sinph0 * phi) * rh;
            let xx = x * sinc * self.cosph0;
            let phi = if phi.abs() >= 1.0 {
                if phi < 0.0 {
                    -FRAC_PI_2
                } else {
                    FRAC_PI_2
                }
            } else {
                phi.asin()
            };
            let lam = if yy == 0.0 {
                if xx == 0.0 {
                    0.0
                } else if xx < 0.0 {
                    -FRAC_PI_2
                } else {
                    FRAC_PI_2
                }
            } else {
                xx.atan2(yy)
            };
            (phi, lam)
        };

        for _ in 0..20 {
            let cosphi = phi.cos();
            let sinphi = phi.sin();
            let coslam = lam.cos();
            let sinlam = lam.sin();
            let om = 1.0 - ES * sinphi * sinphi;
            let nu = 1.0 / om.sqrt();
            let nx = nu * cosphi * sinlam;
            let ny = nu * (sinphi * self.cosph0 - cosphi * self.sinph0 * coslam)
                + ES * (self.nu0 * self.sinph0 - nu * sinphi) * self.cosph0;
            let rho = ONE_ES * nu / om;
            let j11 = -rho * sinphi * sinlam;
            let j12 = nu * cosphi * coslam;
            let j21 = rho * (cosphi * self.cosph0 + sinphi * self.sinph0 * coslam);
            let j22 = nu * self.sinph0 * cosphi * sinlam;
            let det = j11 * j22 - j12 * j21;
            let dx = x - nx;
            let dy = y - ny;
            let dphi = (j22 * dx - j12 * dy) / det;
            let dlam = (-j21 * dx + j11 * dy) / det;

            phi += dphi;
            if phi > FRAC_PI_2 {
                phi = FRAC_PI_2 - (phi - FRAC_PI_2);
                lam += PI;
            } else if phi < -FRAC_PI_2 {
                phi = -FRAC_PI_2 + (-FRAC_PI_2 - phi);
                lam += PI;
            }
            lam += dlam;

            if dphi.abs() < 1e-12 && dlam.abs() < 1e-12 {
                return Some(self.finish(phi, lam));
            }
        }

        None
    }

    fn finish(&self, phi: f64, lam: f64) -> (f64, f64) {
        let lon = normalize(self.lon0 + lam).to_degrees();
        let lat = phi.to_degrees();
        (lon, lat)
    }

    /// Forward projection from degrees to meters. Test use only.
    #[cfg(test)]
    pub fn forward(&self, lat_deg: f64, lon_deg: f64) -> (f64, f64) {
        let phi = lat_deg.to_radians();
        let lam = normalize(lon_deg.to_radians() - self.lon0);
        let cosphi = phi.cos();
        let sinphi = phi.sin();
        let coslam = lam.cos();
        let sinlam = lam.sin();

        let nu = 1.0 / (1.0 - ES * sinphi * sinphi).sqrt();
        let xp = nu * cosphi * sinlam;
        let yp = nu * (sinphi * self.cosph0 - cosphi * self.sinph0 * coslam)
            + ES * (self.nu0 * self.sinph0 - nu * sinphi) * self.cosph0;

        (xp * A, yp * A)
    }
}

/// Degrees as the reference tool passes them to PROJ: the shortest decimal
/// representation of the float32 radar coordinate, parsed back as a double.
pub fn shortest_f32_degrees(value: f32) -> f64 {
    value
        .to_string()
        .parse::<f64>()
        .expect("float formatting round-trips")
}

fn normalize(mut lam: f64) -> f64 {
    if lam.abs() <= PI {
        return lam;
    }
    lam += PI;
    lam -= 2.0 * PI * (lam / (2.0 * PI)).floor();
    lam -= PI;
    lam
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAT0: f64 = 35.333_362_579_345_7;
    const LON0: f64 = -97.277_763_366_699_22;

    #[test]
    fn center_is_identity() {
        let ortho = Ortho::new(LAT0, LON0);
        let (lon, lat) = ortho.inverse(0.0, 0.0).expect("center projects");
        assert!((lon - LON0).abs() < 1e-12, "lon {lon}");
        assert!((lat - LAT0).abs() < 1e-12, "lat {lat}");
    }

    #[test]
    fn round_trip_oblique() {
        let ortho = Ortho::new(LAT0, LON0);
        for (lat, lon) in [
            (35.5, -97.0),
            (34.9, -97.6),
            (35.333, -96.5),
            (36.0, -97.277),
        ] {
            let (x, y) = ortho.forward(lat, lon);
            let (lon_out, lat_out) = ortho.inverse(x, y).expect("point projects");
            assert!((lat_out - lat).abs() < 1e-9, "lat {lat_out} != {lat}");
            assert!((lon_out - lon).abs() < 1e-9, "lon {lon_out} != {lon}");
        }
    }

    #[test]
    fn round_trip_equatorial() {
        let ortho = Ortho::new(0.0, 0.0);
        for (lat, lon) in [(0.5, 0.5), (-0.5, -0.5)] {
            let (x, y) = ortho.forward(lat, lon);
            let (lon_out, lat_out) = ortho.inverse(x, y).expect("point projects");
            assert!((lat_out - lat).abs() < 1e-9, "lat {lat_out} != {lat}");
            assert!((lon_out - lon).abs() < 1e-9, "lon {lon_out} != {lon}");
        }
    }

    #[test]
    fn outside_domain() {
        let ortho = Ortho::new(LAT0, LON0);
        assert!(ortho.inverse(1.0e9, 0.0).is_none());
    }
}
