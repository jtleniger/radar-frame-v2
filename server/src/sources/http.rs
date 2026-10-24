//! Shared policy for upstream weather requests.
//!
//! Bodies are bounded after decompression so a corrupt or hostile response
//! cannot exhaust memory; gzip is negotiated by the client and every request
//! carries an explicit `Accept`. Each call logs one structured event with its
//! status, latency, and size, never the body itself.

use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, ACCEPT, RETRY_AFTER};
use reqwest::Url;

use crate::error::{Error, Result};

/// Upper bound on a decompressed upstream response body.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Connection establishment budget for every upstream request.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Total budget for one upstream request, including the response body.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How much of a failed response's body to keep for the error message.
const MAX_ERROR_BODY_BYTES: usize = 512;

pub async fn get(client: &reqwest::Client, url: Url, accept: &str, source: &str) -> Result<String> {
    let started = Instant::now();
    let response = client
        .get(url.clone())
        .header(ACCEPT, accept)
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(
                event = "upstream",
                source,
                status = 0,
                elapsed_ms = elapsed_ms(started),
                bytes = 0,
                error_kind = "transport",
                "upstream request failed"
            );
            Error::http(format!("{url}: {error}"))
        })?;

    let status = response.status();
    let retry_after = retry_after(response.headers());
    let elapsed = elapsed_ms(started);

    if !status.is_success() {
        let body = error_body(response).await;
        tracing::warn!(
            event = "upstream",
            source,
            status = status.as_u16(),
            elapsed_ms = elapsed,
            bytes = body.len(),
            error_kind = "status",
            "upstream request failed"
        );
        return Err(Error::http_status(
            status.as_u16(),
            retry_after,
            format!("{url}: {status}: {body}"),
        ));
    }

    let body = match body(response).await {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(
                event = "upstream",
                source,
                status = status.as_u16(),
                elapsed_ms = elapsed,
                bytes = 0,
                error_kind = "body",
                "upstream response rejected"
            );
            return Err(Error::http(format!("{url}: {error}")));
        }
    };

    tracing::info!(
        event = "upstream",
        source,
        status = status.as_u16(),
        elapsed_ms = elapsed,
        bytes = body.len(),
        "upstream request"
    );

    Ok(body)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// `Retry-After` in seconds. HTTP-date values are ignored; the mapping then
/// omits the header instead of guessing a delay.
pub fn retry_after(headers: &HeaderMap) -> Option<u64> {
    headers.get(RETRY_AFTER)?.to_str().ok()?.trim().parse().ok()
}

/// Read a successful body, refusing anything above [`MAX_BODY_BYTES`].
async fn body(mut response: reqwest::Response) -> Result<String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let size = body.len().saturating_add(chunk.len());
        if size > MAX_BODY_BYTES {
            return Err(Error::http(format!(
                "response exceeds {MAX_BODY_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Read only enough of a failed body for the log and the error message.
async fn error_body(mut response: reqwest::Response) -> String {
    let mut body = Vec::new();
    while body.len() < MAX_ERROR_BODY_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    body.truncate(MAX_ERROR_BODY_BYTES);
    String::from_utf8_lossy(&body).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get as route;
    use axum::Router;

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{address}/body")
    }

    #[tokio::test]
    async fn reads_a_body_within_the_cap() {
        let url = serve(Router::new().route("/body", route(|| async { "{\"ok\":true}" }))).await;
        let body = get(
            &reqwest::Client::new(),
            url.parse().unwrap(),
            "application/json",
            "test",
        )
        .await
        .unwrap();
        assert_eq!(body, "{\"ok\":true}");
    }

    #[tokio::test]
    async fn rejects_a_body_above_the_cap() {
        let url =
            serve(Router::new().route("/body", route(|| async { vec![b'a'; MAX_BODY_BYTES + 1] })))
                .await;
        let error = get(
            &reqwest::Client::new(),
            url.parse().unwrap(),
            "application/json",
            "test",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("exceeds"), "{error}");
    }

    #[tokio::test]
    async fn rejects_a_gzip_body_that_expands_above_the_cap() {
        use std::io::Write;

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&vec![b'a'; MAX_BODY_BYTES + 1]).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < MAX_BODY_BYTES);

        let url = serve(Router::new().route(
            "/body",
            route(move || async move {
                (
                    [(axum::http::header::CONTENT_ENCODING, "gzip")],
                    compressed.clone(),
                )
            }),
        ))
        .await;
        let error = get(
            &reqwest::Client::new(),
            url.parse().unwrap(),
            "application/json",
            "test",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("exceeds"), "{error}");
    }

    #[tokio::test]
    async fn reports_a_failed_status_with_a_short_body() {
        let url = serve(Router::new().route(
            "/body",
            route(|| async { (axum::http::StatusCode::BAD_GATEWAY, "upstream down") }),
        ))
        .await;
        let error = get(
            &reqwest::Client::new(),
            url.parse().unwrap(),
            "application/json",
            "test",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("502"), "{error}");
        assert!(error.contains("upstream down"), "{error}");
    }
}
