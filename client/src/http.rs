//! Blocking HTTP access to the radar-frame server.
//!
//! HTTP statuses are returned to the caller instead of being collapsed into a
//! generic error: `304` is the normal "no new frame" answer, and `500` and
//! `503` need different retry behaviour.

use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;
use ureq::Agent;
use url::Url;

use crate::config::ServerConfig;

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).trim().to_string()
    }
}

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("invalid server URL: {0}")]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    Transport(#[from] ureq::Error),
}

pub struct ServerClient {
    agent: Agent,
    base_url: Url,
}

impl ServerClient {
    pub fn new(config: &ServerConfig) -> Self {
        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(config.request_timeout))
            .timeout_connect(Some(config.connect_timeout))
            .user_agent(concat!("radar-frame-client/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();

        ServerClient {
            agent,
            base_url: config.base_url.clone(),
        }
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub fn healthz(&self) -> Result<HttpResponse, HttpError> {
        self.get("healthz", None, false)
    }

    pub fn readyz(&self) -> Result<HttpResponse, HttpError> {
        self.get("readyz", None, false)
    }

    pub fn status(&self) -> Result<HttpResponse, HttpError> {
        self.get("status", None, false)
    }

    /// `GET /frame`, conditional on the frame currently on the panel.
    pub fn frame(&self, etag: Option<&str>, debug: bool) -> Result<HttpResponse, HttpError> {
        self.get("frame", etag, debug)
    }

    fn get(
        &self,
        endpoint: &str,
        etag: Option<&str>,
        debug: bool,
    ) -> Result<HttpResponse, HttpError> {
        let mut url = self.base_url.join(endpoint)?;
        if debug {
            url.query_pairs_mut().append_pair("debug", "true");
        }

        let mut request = self.agent.get(url.as_str());
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag);
        }

        let mut response = request.call()?;
        let status = response.status().as_u16();
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);
        let body = response.body_mut().read_to_vec()?;

        Ok(HttpResponse {
            status,
            etag,
            retry_after,
            body,
        })
    }
}

/// Parse the server's `Retry-After` header: delta seconds, or an HTTP date.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    DateTime::parse_from_rfc2822(value).ok().map(|time| {
        let seconds = time
            .with_timezone(&Utc)
            .signed_duration_since(Utc::now())
            .num_seconds()
            .max(0) as u64;
        Duration::from_secs(seconds)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::{TestResponse, TestServer};

    fn config(base_url: Url) -> ServerConfig {
        ServerConfig {
            base_url,
            poll_interval: Duration::from_secs(120),
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
        }
    }

    #[test]
    fn parses_retry_after_seconds_and_dates() {
        assert_eq!(parse_retry_after(" 42 "), Some(Duration::from_secs(42)));
        assert_eq!(parse_retry_after("soon"), None);

        let date = (Utc::now() + chrono::Duration::seconds(20))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let parsed = parse_retry_after(&date).unwrap();
        assert!((15..=20).contains(&parsed.as_secs()));
    }

    #[test]
    fn sends_if_none_match_and_returns_304_with_etag() {
        let server = TestServer::spawn(|request| match request.path.as_str() {
            "/frame" => {
                assert_eq!(request.header("if-none-match"), Some("\"abc\""));
                TestResponse::new(304, Vec::new()).header("ETag", "\"abc\"")
            }
            other => panic!("unexpected path {other}"),
        });

        let client = ServerClient::new(&config(server.base_url()));
        let response = client.frame(Some("\"abc\""), false).unwrap();
        assert_eq!(response.status, 304);
        assert_eq!(response.etag.as_deref(), Some("\"abc\""));
        assert!(response.body.is_empty());
    }

    #[test]
    fn debug_requests_add_the_debug_query() {
        let server = TestServer::spawn(|request| match request.path.as_str() {
            "/frame?debug=true" => TestResponse::new(200, b"png".to_vec()),
            other => panic!("unexpected path {other}"),
        });

        let client = ServerClient::new(&config(server.base_url()));
        let response = client.frame(None, true).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"png");
    }

    #[test]
    fn preserves_server_errors_and_retry_after() {
        let server = TestServer::spawn(|_| {
            TestResponse::new(503, b"not ready\n".to_vec()).header("Retry-After", "60")
        });

        let client = ServerClient::new(&config(server.base_url()));
        let response = client.healthz().unwrap();
        assert_eq!(response.status, 503);
        assert_eq!(response.retry_after, Some(Duration::from_secs(60)));
        assert_eq!(response.body_text(), "not ready");
    }
}
