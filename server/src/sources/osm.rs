//! Overpass street data download.
//!
//! The mask itself is rasterized by `frame-render streets`; this module only
//! owns the HTTP query, which runs when the mask or the OSM file is missing or
//! stale for the configured bbox.

use async_trait::async_trait;

use crate::error::{Error, Result};

pub const URL: &str = "https://overpass-api.de/api/interpreter";

/// The query from `sources/osm.py` and the README fallback, byte for byte.
pub fn query(min_lat: &str, min_lon: &str, max_lat: &str, max_lon: &str) -> String {
    format!(
        "\n    way[\"highway\"~\"motorway|primary\"]({min_lat},{min_lon},{max_lat},{max_lon});\n    out body;\n    >;\n    out skel qt;\n    "
    )
}

#[async_trait]
pub trait OsmFetcher: Send + Sync {
    /// Run the Overpass query and return the OSM XML body.
    async fn fetch(&self, query: &str) -> Result<String>;
}

pub struct Overpass {
    client: reqwest::Client,
    user_agent: String,
    url: String,
}

impl Overpass {
    pub fn new(client: reqwest::Client, user_agent: impl Into<String>) -> Self {
        Overpass {
            client,
            user_agent: user_agent.into(),
            url: URL.to_string(),
        }
    }
}

#[async_trait]
impl OsmFetcher for Overpass {
    async fn fetch(&self, query: &str) -> Result<String> {
        tracing::info!("executing overpass query:{query}");

        let response = self
            .client
            .post(&self.url)
            .header(reqwest::header::USER_AGENT, self.user_agent.as_str())
            .form(&[("data", query)])
            .send()
            .await?;

        let status = response.status();
        let retry_after = super::http::retry_after(response.headers());
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error::http_status(
                status.as_u16(),
                retry_after,
                format!("{}: {status}: {body}", self.url),
            ));
        }

        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_matches_the_python_source() {
        assert_eq!(
            query("35.35", "-97.26", "35.39", "-97.21"),
            "\n    way[\"highway\"~\"motorway|primary\"](35.35,-97.26,35.39,-97.21);\n    out body;\n    >;\n    out skel qt;\n    "
        );
    }

    #[tokio::test]
    async fn fetch_sends_the_configured_user_agent() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let read = socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbody")
                .await
                .unwrap();
            String::from_utf8_lossy(&request[..read]).into_owned()
        });

        let fetcher = Overpass {
            client: reqwest::Client::new(),
            user_agent: "radar-frame (osm@example.com)".into(),
            url: format!("http://{address}/api/interpreter"),
        };
        assert_eq!(fetcher.fetch("query").await.unwrap(), "body");

        let request = server.await.unwrap().to_ascii_lowercase();
        assert!(
            request.contains("user-agent: radar-frame (osm@example.com)"),
            "{request}"
        );
    }
}
