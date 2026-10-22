//! NEXRAD Level II archive access over the public S3 bucket.
//!
//! Replaces boto3's anonymous `list_objects_v2` / `download_file` with the same
//! two HTTP operations: a ListObjectsV2 query for the day's prefix and a plain
//! GET of the selected key.

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;

use super::{Object, RadarSource};
use crate::error::{Error, Result};

const BASE_URL: &str = "https://unidata-nexrad-level2.s3.amazonaws.com";
const BUCKET_PREFIX: &str = "unidata-nexrad-level2";

pub struct S3Radar {
    client: reqwest::Client,
}

impl S3Radar {
    pub fn new(client: reqwest::Client) -> Self {
        S3Radar { client }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<Object>> {
        let url = format!("{BASE_URL}/");
        tracing::info!("s3 list: {BUCKET_PREFIX}/{prefix}");

        let response = self
            .client
            .get(url)
            .query(&[("list-type", "2"), ("prefix", prefix)])
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error(format!("s3 list {prefix}: {status}: {body}")));
        }

        parse_list(&body)
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>> {
        let url = format!("{BASE_URL}/{key}");
        tracing::info!("s3 get: {key}");

        let response = self.client.get(url.clone()).send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error(format!("{url}: {status}: {body}")));
        }

        Ok(response.bytes().await?.to_vec())
    }
}

fn day_prefix(now: DateTime<Utc>) -> String {
    format!("{:04}/{:02}/{:02}", now.year(), now.month(), now.day())
}

#[async_trait]
impl RadarSource for S3Radar {
    async fn latest(&self, nexrad_id: &str, now: DateTime<Utc>) -> Result<Option<Object>> {
        let mut prefix = format!("{}/{}", day_prefix(now), nexrad_id);
        let mut objects = self.list(&prefix).await?;

        if objects.is_empty() {
            // Midnight UTC can leave today with no objects yet.
            let yesterday = now - Duration::days(1);
            prefix = format!("{}/{}", day_prefix(yesterday), nexrad_id);
            objects = self.list(&prefix).await?;
        }

        objects.retain(|object| !object.key.ends_with("_MDM"));
        if objects.is_empty() {
            tracing::error!("no objects found for prefix {prefix}");
            return Ok(None);
        }

        objects.sort_by_key(|object| std::cmp::Reverse(object.last_modified));
        Ok(objects.into_iter().next())
    }

    async fn download(&self, object: &Object) -> Result<Vec<u8>> {
        self.get(&object.key).await
    }
}

/// Parse a ListObjectsV2 result. Only `Contents` entries with a `Key` and a
/// `LastModified` are returned.
pub fn parse_list(body: &str) -> Result<Vec<Object>> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);

    let mut objects = Vec::new();
    let mut inside_contents = false;
    let mut tag: Vec<u8> = Vec::new();
    let mut key: Option<String> = None;
    let mut last_modified: Option<String> = None;
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
            Ok(Event::Text(event)) if inside_contents => {
                let text = event
                    .unescape()
                    .map_err(|e| Error(format!("s3 list xml: {e}")))?
                    .into_owned();
                if tag == b"Key" {
                    key = Some(text);
                } else if tag == b"LastModified" {
                    last_modified = Some(text);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(Error(format!("s3 list xml: {e}"))),
        }
        buffer.clear();
    }

    Ok(objects)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let objects = parse_list(LIST).unwrap();
        assert_eq!(objects.len(), 3);
        assert_eq!(objects[2].key, "2026/10/21/KTLX/KTLX20261021_153500_V06");
        assert_eq!(
            objects[2].last_modified.to_string(),
            "2026-10-21 15:35:00 UTC"
        );
    }

    #[test]
    fn ignores_objects_without_times() {
        let objects =
            parse_list("<ListBucketResult><Contents><Key>k</Key></Contents></ListBucketResult>")
                .unwrap();
        assert!(objects.is_empty());
    }

    #[test]
    fn day_prefix_matches_the_bucket_layout() {
        let now = DateTime::parse_from_rfc3339("2026-10-21T15:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(day_prefix(now), "2026/10/21");
    }
}
