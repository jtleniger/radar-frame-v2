//! Client for the radar-frame HTTP service.
//!
//! The binary polls `/healthz`, `/readyz`, `/frame`, and `/status`, then
//! refreshes a Waveshare 7.3-inch seven-colour panel. The modules are public so
//! the polling and PNG conversion logic can be exercised without a display.

pub mod config;
pub mod display;
pub mod http;
pub mod logging;
pub mod poll;
pub mod status;

#[cfg(test)]
pub mod testsupport;
