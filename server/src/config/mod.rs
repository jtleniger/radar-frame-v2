//! Configuration, derived settings, runtime paths, and frame dimensions.

#[allow(clippy::module_inception)] // `config::config::Config` mirrors the file it parses.
pub mod config;
pub mod constants;
pub mod paths;
pub mod settings;

pub use config::{Config, DEFAULT_USER_AGENT};
pub use paths::Paths;
pub use settings::Settings;
