//! Render the radar frame PNG from NEXRAD data.
//!
//! Two entry points share the same rasterizer, color table, composite, and
//! street overlay:
//!
//! - `radar`: pre-rendered per-elevation GeoJSON gates (the `nexrad-geojson`
//!   output);
//! - `frame`: an Archive II volume, projected and rasterized directly, with no
//!   GeoJSON intermediate.
//!
//! This crate replaces the `gdal_rasterize` / `gdal_calc.py` / `gdaldem` /
//! `gdal_translate` / PIL sequence in `components/radar/render.py`. The gate
//! coordinates and the output grid share the WGS84 lon/lat frame, so no
//! reprojection is involved: only an affine geotransform, the GDAL polygon
//! scanline fill, a nearest-entry color table lookup, and an alpha composite.

pub mod archive;
pub mod colors;
pub mod osm;
pub mod pngio;
pub mod raster;
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
