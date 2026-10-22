//! The radar frame service: HTTP API, mode schedule, forecast sources, and the
//! setup artifacts.
//!
//! The image work is delegated to the sibling crates `frame-render` (radar
//! frame) and `frame-view` (e-ink composition); this crate is the process that
//! used to be Flask plus `requests`, boto3, and the mode/state modules.

pub mod api;
pub mod clock;
pub mod config;
pub mod constants;
pub mod engine;
pub mod error;
pub mod logging;
pub mod modes;
pub mod paths;
pub mod setup;
pub mod sources;
pub mod state;
