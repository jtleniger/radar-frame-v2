//! Overpass street data download.
//!
//! The mask itself is rasterized by `frame-render streets`; this module only
//! owns the HTTP query, which runs when the mask or the OSM file is missing or
//! stale for the configured bbox.

use async_trait::async_trait;

use crate::error::{Error, Result};

const URL: &str = "https://overpass-api.de/api/interpreter";

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
}

impl Overpass {
    pub fn new(client: reqwest::Client) -> Self {
        Overpass { client }
    }
}

#[async_trait]
impl OsmFetcher for Overpass {
    async fn fetch(&self, query: &str) -> Result<String> {
        tracing::info!("executing overpass query:{query}");

        let response = self
            .client
            .post(URL)
            .form(&[("data", query)])
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error(format!("{URL}: {status}: {body}")));
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
}
