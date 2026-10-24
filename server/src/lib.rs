//! The radar frame service: HTTP API, mode schedule, forecast sources, setup
//! artifacts, and the frame renderers.
//!
//! The image work lives in [`render`] (radar frame) and [`view`] (e-ink
//! composition); this package is the process that used to be Flask plus
//! `requests`, boto3, and the mode/state modules.

pub mod api;
pub mod cli;
pub mod clock;
pub mod config;
pub mod constants;
pub mod engine;
pub mod error;
pub mod frame;
pub mod logging;
pub mod metrics;
pub mod modes;
pub mod nexrad;
pub mod paths;
pub mod render;
pub mod setup;
pub mod sources;
pub mod state;
pub mod view;
