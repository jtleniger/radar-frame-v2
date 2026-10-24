//! NEXRAD Level II archive access over the public S3 bucket.
//!
//! Replaces boto3's anonymous `list_objects_v2` / `download_file` with the same
//! two HTTP operations: a ListObjectsV2 query for the day's prefix and a plain
//! GET of the selected key.

use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;
use tokio::io::AsyncWriteExt;

use super::{Object, RadarSource};
use crate::error::{Error, Result};
use crate::nexrad::archive2::MAX_VOLUME_BYTES;

pub const BASE_URL: &str = "https://unidata-nexrad-level2.s3.amazonaws.com";
const BUCKET_PREFIX: &str = "unidata-nexrad-level2";
/// A day has a few hundred volume keys. The cap only stops a broken listing
/// that insists there is another page forever.
const MAX_LIST_PAGES: usize = 100;

pub struct S3Radar {
    client: reqwest::Client,
    base_url: String,
}

impl S3Radar {
    pub fn new(client: reqwest::Client) -> Self {
        S3Radar::with_base_url(client, BASE_URL)
    }

    fn with_base_url(client: reqwest::Client, base_url: impl Into<String>) -> Self {
        S3Radar {
            client,
            base_url: base_url.into(),
        }
    }

    async fn list(&self, prefix: &str, token: Option<&str>) -> Result<Listing> {
        let url = format!("{}/", self.base_url);
        tracing::info!("s3 list: {BUCKET_PREFIX}/{prefix}");

        let mut query = vec![("list-type", "2"), ("prefix", prefix)];
        if let Some(token) = token {
            query.push(("continuation-token", token));
        }

        let started = Instant::now();
        let response = match self.client.get(url).query(&query).send().await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    event = "upstream",
                    source = "s3-list",
                    status = 0,
                    elapsed_ms = elapsed_ms(started),
                    bytes = 0,
                    error_kind = "transport",
                    "upstream request failed"
                );
                return Err(error.into());
            }
        };

        let status = response.status();
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                tracing::warn!(
                    event = "upstream",
                    source = "s3-list",
                    status = status.as_u16(),
                    elapsed_ms = elapsed_ms(started),
                    bytes = 0,
                    error_kind = "body",
                    "upstream response rejected"
                );
                return Err(error.into());
            }
        };
        let elapsed = elapsed_ms(started);
        if !status.is_success() {
            tracing::warn!(
                event = "upstream",
                source = "s3-list",
                status = status.as_u16(),
                elapsed_ms = elapsed,
                bytes = body.len(),
                error_kind = "status",
                "upstream request failed"
            );
            return Err(Error(format!("s3 list {prefix}: {status}: {body}")));
        }

        tracing::info!(
            event = "upstream",
            source = "s3-list",
            status = status.as_u16(),
            elapsed_ms = elapsed,
            bytes = body.len(),
            "upstream request"
        );

        parse_list(&body)
    }

    /// Every page of a prefix's listing, following `NextContinuationToken`.
    async fn day_objects(&self, prefix: &str) -> Result<Vec<Object>> {
        let mut objects = Vec::new();
        let mut token = None;

        for _ in 0..MAX_LIST_PAGES {
            let page = self.list(prefix, token.as_deref()).await?;
            objects.extend(page.objects);

            if !page.truncated {
                return Ok(objects);
            }
            token = Some(page.next_token.ok_or_else(|| {
                Error(format!(
                    "s3 list {prefix}: truncated without a continuation token"
                ))
            })?);
        }

        Err(Error(format!(
            "s3 list {prefix}: more than {MAX_LIST_PAGES} pages"
        )))
    }

    /// Listing entries that carry real volume data. `_MDM` objects are
    /// metadata-only placeholders and cannot be rendered.
    async fn volumes(&self, prefix: &str) -> Result<Vec<Object>> {
        let mut objects = self.day_objects(prefix).await?;
        objects.retain(|object| !object.key.ends_with("_MDM"));
        Ok(objects)
    }

    async fn get_to(&self, key: &str, destination: &Path, max_bytes: u64) -> Result<u64> {
        let url = format!("{}/{key}", self.base_url);
        self.stream_to(&url, destination, max_bytes).await
    }

    async fn stream_to(&self, url: &str, destination: &Path, max_bytes: u64) -> Result<u64> {
        tracing::info!("s3 get: {url}");

        let started = Instant::now();
        let mut response = match self.client.get(url).send().await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    event = "upstream",
                    source = "s3-get",
                    status = 0,
                    elapsed_ms = elapsed_ms(started),
                    bytes = 0,
                    error_kind = "transport",
                    "upstream request failed"
                );
                return Err(error.into());
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(
                event = "upstream",
                source = "s3-get",
                status = status.as_u16(),
                elapsed_ms = elapsed_ms(started),
                bytes = body.len(),
                error_kind = "status",
                "upstream request failed"
            );
            return Err(Error(format!("{url}: {status}: {body}")));
        }

        if let Some(length) = response.content_length() {
            if length > max_bytes {
                tracing::warn!(
                    event = "upstream",
                    source = "s3-get",
                    status = status.as_u16(),
                    elapsed_ms = elapsed_ms(started),
                    bytes = 0,
                    error_kind = "size",
                    "upstream response rejected"
                );
                return Err(Error(format!(
                    "{url}: Content-Length {length} exceeds the {max_bytes}-byte limit"
                )));
            }
        }

        let mut file = tokio::fs::File::create(destination)
            .await
            .map_err(|error| Error(format!("{}: {error}", destination.display())))?;
        let mut written = 0u64;

        loop {
            let chunk = match response.chunk().await {
                Ok(chunk) => chunk,
                Err(error) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(destination).await;
                    tracing::warn!(
                        event = "upstream",
                        source = "s3-get",
                        status = status.as_u16(),
                        elapsed_ms = elapsed_ms(started),
                        bytes = written,
                        error_kind = "body",
                        "upstream response rejected"
                    );
                    return Err(error.into());
                }
            };
            let Some(chunk) = chunk else { break };

            written = match written.checked_add(chunk.len() as u64) {
                Some(written) if written <= max_bytes => written,
                _ => {
                    drop(file);
                    let _ = tokio::fs::remove_file(destination).await;
                    tracing::warn!(
                        event = "upstream",
                        source = "s3-get",
                        status = status.as_u16(),
                        elapsed_ms = elapsed_ms(started),
                        bytes = written,
                        error_kind = "size",
                        "upstream response rejected"
                    );
                    return Err(Error(format!(
                        "{url}: response exceeds the {max_bytes}-byte limit"
                    )));
                }
            };

            if let Err(error) = file.write_all(&chunk).await {
                drop(file);
                let _ = tokio::fs::remove_file(destination).await;
                return Err(Error(format!("{}: {error}", destination.display())));
            }
        }

        file.flush()
            .await
            .map_err(|error| Error(format!("{}: {error}", destination.display())))?;
        file.sync_all()
            .await
            .map_err(|error| Error(format!("{}: {error}", destination.display())))?;
        drop(file);

        tracing::info!(
            event = "upstream",
            source = "s3-get",
            status = status.as_u16(),
            elapsed_ms = elapsed_ms(started),
            bytes = written,
            "upstream request"
        );

        Ok(written)
    }
}

fn day_prefix(now: DateTime<Utc>) -> String {
    format!("{:04}/{:02}/{:02}", now.year(), now.month(), now.day())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[async_trait]
impl RadarSource for S3Radar {
    async fn latest(&self, nexrad_id: &str, now: DateTime<Utc>) -> Result<Option<Object>> {
        let mut prefix = format!("{}/{}", day_prefix(now), nexrad_id);
        let mut objects = self.volumes(&prefix).await?;

        if objects.is_empty() {
            // Midnight UTC can leave today with no searchable volumes yet,
            // and an MDM-only day has nothing to render; either way, use the
            // previous day's last volume.
            let yesterday = now - Duration::days(1);
            prefix = format!("{}/{}", day_prefix(yesterday), nexrad_id);
            objects = self.volumes(&prefix).await?;
        }

        if objects.is_empty() {
            tracing::error!("no volumes found for prefix {prefix}");
            return Ok(None);
        }

        objects.sort_by_key(|object| std::cmp::Reverse(object.last_modified));
        Ok(objects.into_iter().next())
    }

    async fn download(&self, object: &Object, destination: &Path) -> Result<u64> {
        self.get_to(&object.key, destination, MAX_VOLUME_BYTES)
            .await
    }
}

/// One ListObjectsV2 page: its objects and whether another page follows.
#[derive(Debug, Default)]
pub struct Listing {
    pub objects: Vec<Object>,
    pub truncated: bool,
    pub next_token: Option<String>,
}

/// Parse a ListObjectsV2 result. Only `Contents` entries with a `Key` and a
/// `LastModified` are returned; `IsTruncated` and `NextContinuationToken`
/// describe the following page.
pub fn parse_list(body: &str) -> Result<Listing> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);

    let mut objects = Vec::new();
    let mut inside_contents = false;
    let mut tag: Vec<u8> = Vec::new();
    let mut key: Option<String> = None;
    let mut last_modified: Option<String> = None;
    let mut truncated = false;
    let mut next_token = None;
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                let name = event.name().as_ref().to_vec();
                if name == b"Contents" {
                    inside_contents = true;
                    key = None;
                    last_modified = None;
                }
                tag = name;
            }
            Ok(Event::End(event)) => {
                let name = event.name().as_ref().to_vec();
                if name == b"Contents" {
                    if let (Some(key), Some(last_modified)) = (key.take(), last_modified.take()) {
                        let time = DateTime::parse_from_rfc3339(&last_modified)
                            .map_err(|e| Error(format!("s3 LastModified {last_modified}: {e}")))?
                            .with_timezone(&Utc);
                        objects.push(Object {
                            key,
                            last_modified: time,
                        });
                    }
                    inside_contents = false;
                }
                tag.clear();
            }
            Ok(Event::Text(event)) => {
                let text = event
                    .unescape()
                    .map_err(|e| Error(format!("s3 list xml: {e}")))?
                    .into_owned();
                if inside_contents && tag == b"Key" {
                    key = Some(text);
                } else if inside_contents && tag == b"LastModified" {
                    last_modified = Some(text);
                } else if tag == b"IsTruncated" {
                    truncated = text.eq_ignore_ascii_case("true");
                } else if tag == b"NextContinuationToken" {
                    next_token = Some(text);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(Error(format!("s3 list xml: {e}"))),
        }
        buffer.clear();
    }

    Ok(Listing {
        objects,
        truncated,
        next_token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const LIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>unidata-nexrad-level2</Name>
  <Prefix>2026/10/21/KTLX</Prefix>
  <KeyCount>3</KeyCount>
  <Contents>
    <Key>2026/10/21/KTLX/KTLX20261021_153000_V06</Key>
    <LastModified>2026-10-21T15:30:00.000Z</LastModified>
    <ETag>"a"</ETag>
    <Size>100</Size>
  </Contents>
  <Contents>
    <Key>2026/10/21/KTLX/KTLX20261021_153500_V06_MDM</Key>
    <LastModified>2026-10-21T15:35:00.000Z</LastModified>
  </Contents>
  <Contents>
    <Key>2026/10/21/KTLX/KTLX20261021_153500_V06</Key>
    <LastModified>2026-10-21T15:35:00.000Z</LastModified>
  </Contents>
</ListBucketResult>"#;

    #[test]
    fn parses_contents_and_times() {
        let listing = parse_list(LIST).unwrap();
        assert_eq!(listing.objects.len(), 3);
        assert!(!listing.truncated);
        assert_eq!(listing.next_token, None);
        assert_eq!(
            listing.objects[2].key,
            "2026/10/21/KTLX/KTLX20261021_153500_V06"
        );
        assert_eq!(
            listing.objects[2].last_modified.to_string(),
            "2026-10-21 15:35:00 UTC"
        );
    }

    #[test]
    fn parses_the_next_page_marker() {
        let listing = parse_list(
            "<ListBucketResult><IsTruncated>true</IsTruncated>\
             <NextContinuationToken>1/abc+def=</NextContinuationToken>\
             <Contents><Key>k</Key>\
             <LastModified>2026-10-21T15:35:00Z</LastModified></Contents>\
             </ListBucketResult>",
        )
        .unwrap();
        assert!(listing.truncated);
        assert_eq!(listing.next_token.as_deref(), Some("1/abc+def="));
        assert_eq!(listing.objects.len(), 1);
    }

    #[test]
    fn ignores_objects_without_times() {
        let listing =
            parse_list("<ListBucketResult><Contents><Key>k</Key></Contents></ListBucketResult>")
                .unwrap();
        assert!(listing.objects.is_empty());
    }

    #[test]
    fn day_prefix_matches_the_bucket_layout() {
        let now = DateTime::parse_from_rfc3339("2026-10-21T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(day_prefix(now), "2026/10/21");
    }

    async fn serve(body: &'static [u8]) -> String {
        use axum::routing::get;

        let app = axum::Router::new().route(
            "/object",
            get(move || async move { axum::body::Body::from(body) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{address}/object")
    }

    /// Serve S3 listings from `handler`, recording each query string.
    async fn serve_listing(
        handler: impl Fn(HashMap<String, String>) -> &'static str + Send + Sync + 'static,
    ) -> (String, Arc<Mutex<Vec<HashMap<String, String>>>>) {
        use axum::routing::get;

        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let handler = Arc::new(handler);
        let app = axum::Router::new().route(
            "/",
            get(
                move |axum::extract::Query(params): axum::extract::Query<
                    HashMap<String, String>,
                >| {
                    let recorded = Arc::clone(&recorded);
                    let handler = Arc::clone(&handler);
                    async move {
                        recorded.lock().unwrap().push(params.clone());
                        handler(params)
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), requests)
    }

    const PAGE_ONE: &str = r#"<ListBucketResult>
  <IsTruncated>true</IsTruncated>
  <NextContinuationToken>page-2</NextContinuationToken>
  <Contents>
    <Key>2026/10/21/KTLX/KTLX20261021_153000_V06</Key>
    <LastModified>2026-10-21T15:30:00.000Z</LastModified>
  </Contents>
</ListBucketResult>"#;

    const PAGE_TWO: &str = r#"<ListBucketResult>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>2026/10/21/KTLX/KTLX20261021_154500_V06</Key>
    <LastModified>2026-10-21T15:45:00.000Z</LastModified>
  </Contents>
</ListBucketResult>"#;

    fn latest_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-21T16:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[tokio::test]
    async fn follows_continuation_tokens_to_the_newest_volume() {
        let (base, requests) = serve_listing(|params| {
            if params.get("continuation-token").map(String::as_str) == Some("page-2") {
                PAGE_TWO
            } else {
                PAGE_ONE
            }
        })
        .await;

        let radar = S3Radar::with_base_url(reqwest::Client::new(), base);
        let latest = radar.latest("KTLX", latest_now()).await.unwrap().unwrap();

        assert_eq!(
            latest.key, "2026/10/21/KTLX/KTLX20261021_154500_V06",
            "the newest object was on the second page"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert_eq!(requests[1].get("continuation-token").unwrap(), "page-2");
        assert_eq!(requests[1].get("prefix").unwrap(), "2026/10/21/KTLX");
    }

    #[tokio::test]
    async fn falls_back_to_yesterday_when_today_is_mdm_only() {
        const TODAY_MDM: &str = r#"<ListBucketResult>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>2026/10/21/KTLX/KTLX20261021_153000_V06_MDM</Key>
    <LastModified>2026-10-21T15:30:00.000Z</LastModified>
  </Contents>
</ListBucketResult>"#;
        const YESTERDAY: &str = r#"<ListBucketResult>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>2026/10/20/KTLX/KTLX20261020_235500_V06</Key>
    <LastModified>2026-10-20T23:55:00.000Z</LastModified>
  </Contents>
</ListBucketResult>"#;

        let (base, requests) = serve_listing(|params| {
            if params["prefix"].starts_with("2026/10/20") {
                YESTERDAY
            } else {
                TODAY_MDM
            }
        })
        .await;

        let radar = S3Radar::with_base_url(reqwest::Client::new(), base);
        let latest = radar.latest("KTLX", latest_now()).await.unwrap().unwrap();

        assert_eq!(latest.key, "2026/10/20/KTLX/KTLX20261020_235500_V06");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert_eq!(requests[0].get("prefix").unwrap(), "2026/10/21/KTLX");
        assert_eq!(requests[1].get("prefix").unwrap(), "2026/10/20/KTLX");
    }

    #[tokio::test]
    async fn rejects_a_truncated_page_without_a_token() {
        let (base, _) = serve_listing(|_| {
            "<ListBucketResult><IsTruncated>true</IsTruncated></ListBucketResult>"
        })
        .await;

        let radar = S3Radar::with_base_url(reqwest::Client::new(), base);
        let error = radar
            .latest("KTLX", latest_now())
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("continuation token"), "{error}");
    }

    #[tokio::test]
    async fn streams_a_body_to_the_destination() {
        let url = serve(b"abcdef").await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("volume.part");
        let radar = S3Radar::new(reqwest::Client::new());

        let written = radar.stream_to(&url, &destination, 8).await.unwrap();

        assert_eq!(written, 6);
        assert_eq!(std::fs::read(&destination).unwrap(), b"abcdef");
    }

    #[tokio::test]
    async fn rejects_a_body_above_the_cap_and_removes_the_part_file() {
        let url = serve(b"abcdefghi").await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("volume.part");
        let radar = S3Radar::new(reqwest::Client::new());

        let error = radar
            .stream_to(&url, &destination, 8)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("exceeds"), "{error}");
        assert!(!destination.exists());
    }
}
