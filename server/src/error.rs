//! Typed failures with an HTTP mapping.
//!
//! Every variant carries the file, setting, or endpoint that failed; the
//! message is for the log. The variant decides the response status, so a
//! handler propagates an error instead of guessing:
//!
//! - [`Error::Http`] is an upstream failure: `503` plus `Retry-After` when the
//!   upstream supplied one.
//! - [`Error::Io`] with [`std::io::ErrorKind::NotFound`] is `404`; any other
//!   I/O failure is `500`.
//! - Configuration, parsing, rendering, and internal failures are `500`.
//!
//! Response bodies are short and fixed; details never cross the HTTP surface.

use std::fmt;
use std::io;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub enum Error {
    /// Configuration file or settings validation.
    Config(String),
    /// Filesystem access; `kind` preserves `NotFound` for the response map.
    Io {
        kind: io::ErrorKind,
        message: String,
    },
    /// Upstream request failure. `status` is the upstream status when there
    /// was one; `retry_after` is its `Retry-After`, in seconds.
    Http {
        status: Option<u16>,
        retry_after: Option<u64>,
        message: String,
    },
    /// Malformed upstream or artifact data.
    Parse(String),
    /// Frame rendering or PNG encoding.
    Render(String),
    /// Anything that does not fit the classes above.
    Internal(String),
}

impl Error {
    /// Internal failure. Kept for call sites whose class is not meaningful to
    /// the HTTP surface; prefer a specific constructor.
    pub fn new(message: impl Into<String>) -> Self {
        Error::Internal(message.into())
    }

    pub fn config(message: impl Into<String>) -> Self {
        Error::Config(message.into())
    }

    /// I/O failure; the kind is what distinguishes `404` from `500`.
    pub fn io(message: impl Into<String>) -> Self {
        Error::Io {
            kind: io::ErrorKind::Other,
            message: message.into(),
        }
    }

    /// I/O failure that keeps the source's kind.
    pub fn io_source(error: &io::Error) -> Self {
        Error::Io {
            kind: error.kind(),
            message: error.to_string(),
        }
    }

    /// I/O failure with the failing path in the message.
    pub fn io_path(path: &std::path::Path, error: &io::Error) -> Self {
        Error::Io {
            kind: error.kind(),
            message: format!("{}: {error}", path.display()),
        }
    }

    /// Upstream transport or protocol failure without an HTTP status.
    pub fn http(message: impl Into<String>) -> Self {
        Error::Http {
            status: None,
            retry_after: None,
            message: message.into(),
        }
    }

    /// Upstream response with a non-success status.
    pub fn http_status(status: u16, retry_after: Option<u64>, message: impl Into<String>) -> Self {
        Error::Http {
            status: Some(status),
            retry_after,
            message: message.into(),
        }
    }

    /// No usable upstream result yet; tells the client how long to wait.
    pub fn unavailable(retry_after: u64, message: impl Into<String>) -> Self {
        Error::Http {
            status: None,
            retry_after: Some(retry_after),
            message: message.into(),
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Error::Parse(message.into())
    }

    pub fn render(message: impl Into<String>) -> Self {
        Error::Render(message.into())
    }

    /// Upstream status, when this error came from an HTTP response.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Http { status, .. } => *status,
            _ => None,
        }
    }

    /// `Retry-After` the upstream supplied, in seconds.
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Error::Http { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(message)
            | Error::Parse(message)
            | Error::Render(message)
            | Error::Internal(message) => f.write_str(message),
            Error::Io { message, .. } | Error::Http { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, body) = match &self {
            Error::Http { .. } => (StatusCode::SERVICE_UNAVAILABLE, "upstream unavailable\n"),
            Error::Io {
                kind: io::ErrorKind::NotFound,
                ..
            } => (StatusCode::NOT_FOUND, "not found\n"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal server error\n"),
        };

        let mut response = (status, body).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );

        if let Some(seconds) = self.retry_after() {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }

        response
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::io_source(&e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::parse(format!("json: {e}"))
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http {
            status: e.status().map(|status| status.as_u16()),
            retry_after: None,
            message: format!("http: {e}"),
        }
    }
}

impl From<crate::render::Error> for Error {
    fn from(e: crate::render::Error) -> Self {
        Error::Render(e.to_string())
    }
}

impl From<crate::view::error::Error> for Error {
    fn from(e: crate::view::error::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

impl From<askama::Error> for Error {
    fn from(e: askama::Error) -> Self {
        Error::Internal(format!("template: {e}"))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_after(response: &Response) -> Option<&str> {
        response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
    }

    #[test]
    fn upstream_failures_map_to_503_and_keep_retry_after() {
        let response = Error::http_status(502, Some(30), "bad gateway").into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(retry_after(&response), Some("30"));

        let response = Error::http("connection reset").into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(retry_after(&response), None);
    }

    #[test]
    fn missing_files_map_to_404_and_other_io_to_500() {
        let missing = Error::io_path(
            std::path::Path::new("frame.png"),
            &io::Error::new(io::ErrorKind::NotFound, "gone"),
        );
        assert_eq!(missing.into_response().status(), StatusCode::NOT_FOUND);

        let denied = Error::io_path(
            std::path::Path::new("frame.png"),
            &io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
        );
        assert_eq!(
            denied.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn internal_classes_map_to_500_with_a_short_body() {
        for error in [
            Error::config("bad bbox"),
            Error::parse("bad json"),
            Error::render("bad png"),
            Error::new("boom"),
        ] {
            let response = error.into_response();
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(retry_after(&response), None);
        }
    }
}
