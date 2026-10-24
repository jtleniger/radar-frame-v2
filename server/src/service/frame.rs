//! The frame most recently produced by a cycle, published as immutable bytes.
//!
//! Serving from memory keeps `/frame` independent of the renderer's file write
//! and gives every response a stable ETag for `If-None-Match`. The file on disk
//! remains a debug artifact and a seed for the first request after a restart.

use std::io::Cursor;
use std::path::Path;

use axum::body::Bytes;
use chrono::{DateTime, Utc};

use crate::error::{Error, Result};
use crate::state::Mode;

#[derive(Clone, Debug)]
pub struct PublishedFrame {
    pub bytes: Bytes,
    /// Quoted entity tag, e.g. `"6f1a0c…"`.
    pub etag: String,
    pub generated_at: DateTime<Utc>,
    pub mode: Option<Mode>,
    pub degraded: Option<String>,
}

impl PublishedFrame {
    /// Validate and publish `bytes`. A frame that does not decode as PNG is
    /// rejected so a torn or corrupt write can never replace the last good
    /// frame.
    pub fn new(
        bytes: Vec<u8>,
        generated_at: DateTime<Utc>,
        mode: Option<Mode>,
        degraded: Option<String>,
    ) -> Result<Self> {
        validate(&bytes)?;
        let etag = etag(&bytes);
        Ok(PublishedFrame {
            bytes: Bytes::from(bytes),
            etag,
            generated_at,
            mode,
            degraded,
        })
    }

    /// Load the frame file left by a previous process and publish it. Bad or
    /// unreadable files are ignored; the next cycle will produce a fresh one.
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!("{}: {error}", path.display());
                return None;
            }
        };
        let generated_at = std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now());

        match Self::new(bytes, generated_at, None, None) {
            Ok(frame) => {
                tracing::info!(
                    "reusing published frame: bytes={} generated_at={}",
                    frame.bytes.len(),
                    frame.generated_at
                );
                Some(frame)
            }
            Err(error) => {
                tracing::warn!("{}: ignoring unusable frame: {error}", path.display());
                None
            }
        }
    }

    /// Serve this frame, honouring a matching `If-None-Match`.
    pub fn response(&self, headers: &axum::http::HeaderMap) -> axum::response::Response {
        use axum::http::{header, HeaderValue, StatusCode};
        use axum::response::IntoResponse;

        let etag =
            HeaderValue::from_str(&self.etag).unwrap_or_else(|_| HeaderValue::from_static(""));

        if if_none_match(headers, &self.etag) {
            let mut response = StatusCode::NOT_MODIFIED.into_response();
            response.headers_mut().insert(header::ETAG, etag);
            return response;
        }

        let mut response = self.bytes.clone().into_response();
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
        response.headers_mut().insert(header::ETAG, etag);
        response
    }
}

fn if_none_match(headers: &axum::http::HeaderMap, etag: &str) -> bool {
    let Some(value) = headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };

    value.split(',').map(str::trim).any(|candidate| {
        candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
    })
}

fn validate(bytes: &[u8]) -> Result<()> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|error| Error::render(format!("frame png: {error}")))?;
    if reader.info().width == 0 || reader.info().height == 0 {
        return Err(Error::render("frame png: empty image"));
    }

    // Decode the whole image, not just the header, so a truncated file can
    // never replace the last good frame.
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| Error::render("frame png: image too large"))?;
    let mut buffer = vec![0; size];
    reader
        .next_frame(&mut buffer)
        .map_err(|error| Error::render(format!("frame png: {error}")))?;
    Ok(())
}

/// Content-addressed tag. It only has to be stable for the life of the
/// process, so the default hasher is enough and avoids another dependency.
fn etag(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.png");
        crate::view::image::Image::new(2, 2, [0, 0, 0, 255])
            .write_png(&path)
            .unwrap();
        std::fs::read(path).unwrap()
    }

    #[test]
    fn rejects_non_png_bytes() {
        assert!(PublishedFrame::new(b"not a png".to_vec(), Utc::now(), None, None).is_err());
    }

    #[test]
    fn rejects_truncated_png() {
        let mut bytes = png();
        bytes.truncate(bytes.len() / 2);
        assert!(PublishedFrame::new(bytes, Utc::now(), None, None).is_err());
    }

    #[test]
    fn tags_follow_content() {
        let first = PublishedFrame::new(png(), Utc::now(), None, None).unwrap();
        let second = PublishedFrame::new(png(), Utc::now(), None, None).unwrap();
        assert_eq!(first.etag, second.etag);

        let mut other = png();
        other.push(0);
        let other = PublishedFrame::new(other, Utc::now(), None, None).unwrap();
        assert_ne!(first.etag, other.etag);
    }
}
