//! The radar frame service: HTTP API, mode schedule, forecast sources, setup
//! artifacts, and the frame renderers.
//!
//! The image work lives in [`render`] (radar frame) and [`view`] (e-ink
//! composition); [`service`] owns the process lifecycle, [`http`] the wire
//! contract, and [`config`] the settings and paths.

pub mod cli;
pub mod config;
pub mod error;
pub mod http;
pub mod nexrad;
pub mod render;
pub mod service;
pub mod sources;
pub mod state;
pub mod telemetry;
pub mod util;
pub mod view;
