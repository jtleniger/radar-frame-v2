//! Errors carry the file, setting, or endpoint that failed; every message is
//! meant to be readable in the log without context.

use std::fmt;

#[derive(Debug)]
pub struct Error(pub String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }
}

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

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(format!("json: {e}"))
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error(format!("http: {e}"))
    }
}

impl From<frame_render::Error> for Error {
    fn from(e: frame_render::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<frame_view::error::Error> for Error {
    fn from(e: frame_view::error::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<askama::Error> for Error {
    fn from(e: askama::Error) -> Self {
        Error(format!("template: {e}"))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
