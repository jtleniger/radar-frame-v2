//! Render the radar frame PNG from a NEXRAD Archive II volume.
//!
//! The gate coordinates and the output grid share the WGS84 lon/lat frame, so
//! no reprojection is involved: only an affine geotransform, the GDAL polygon
//! scanline fill, a nearest-entry color table lookup, and an alpha composite.
//!
//! This replaces the `gdal_rasterize` / `gdal_calc.py` / `gdaldem` /
//! `gdal_translate` / PIL sequence in `components/radar/render.py`.

pub mod archive;
pub mod colors;
pub mod osm;
pub mod pngio;
pub mod raster;
#[allow(clippy::module_inception)]
pub mod render;
pub mod streets;

use std::fmt;

/// Crate-local error; every message names the file or setting that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
