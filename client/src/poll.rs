//! One polling cycle and the retry policy around it.
//!
//! The server contract is:
//! - `200` on `/frame` means new PNG bytes;
//! - `304` means nothing is due (and on a cold start, the process must ask for
//!   the published frame with `?debug=true`);
//! - `503` means no usable frame yet and may carry `Retry-After`;
//! - `500` is an internal failure and is retried with exponential backoff.
//!
//! `/status` is fetched after every attempt for diagnostics and to schedule the
//! next poll at the server's `next_attempt`.

use std::time::Duration;

use chrono::Utc;

use crate::config::ServerConfig;
use crate::display::FrameDisplay;
use crate::http::{HttpError, HttpResponse, ServerClient};
use crate::status::ServerStatus;

pub const MIN_POLL: Duration = Duration::from_secs(15);
/// Upper bound for a successful cycle when `/status` asks for more time.
pub const MAX_SUCCESS_SLEEP: Duration = Duration::from_secs(300);
/// Upper bound for any retry, including `Retry-After`.
pub const MAX_SLEEP: Duration = Duration::from_secs(1800);

#[derive(Clone, Debug, Default)]
pub struct PollState {
    /// ETag of the frame currently on the panel. It is only advanced after a
    /// successful display, so a failed refresh is retried.
    pub displayed_etag: Option<String>,
    pub failures: u32,
}

#[derive(Clone, Debug)]
pub enum PollOutcome {
    Displayed {
        bytes: usize,
        status: Option<ServerStatus>,
    },
    Unmodified {
        status: Option<ServerStatus>,
    },
    NotLive {
        status: u16,
        body: String,
    },
    NotReady {
        reason: String,
        retry_after: Option<Duration>,
        status: Option<ServerStatus>,
    },
    ServerError {
        status: u16,
        body: String,
        retry_after: Option<Duration>,
        status_report: Option<ServerStatus>,
    },
    TransportError {
        message: String,
    },
    DisplayFailed {
        message: String,
        status: Option<ServerStatus>,
    },
}

impl PollOutcome {
    pub fn is_success(&self) -> bool {
        matches!(
            self,
            PollOutcome::Displayed { .. } | PollOutcome::Unmodified { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub struct PollResult {
    pub outcome: PollOutcome,
    pub delay: Duration,
}

pub fn poll_once(
    client: &ServerClient,
    display: &mut dyn FrameDisplay,
    state: &mut PollState,
    config: &ServerConfig,
) -> PollResult {
    match client.healthz() {
        Ok(response) if response.status == 200 => {}
        Ok(response) => {
            state.failures = state.failures.saturating_add(1);
            return PollResult {
                outcome: PollOutcome::NotLive {
                    status: response.status,
                    body: response.body_text(),
                },
                delay: error_delay(state.failures),
            };
        }
        Err(error) => {
            state.failures = state.failures.saturating_add(1);
            return PollResult {
                outcome: PollOutcome::TransportError {
                    message: error.to_string(),
                },
                delay: error_delay(state.failures),
            };
        }
    }

    let response = match client.frame(state.displayed_etag.as_deref(), false) {
        Ok(response) => response,
        Err(error) => return transport_failure(state, error),
    };

    match response.status {
        200 => display_response(client, display, state, response, config),
        304 => {
            if state.displayed_etag.is_some() {
                state.failures = 0;
                let status = fetch_status(client);
                PollResult {
                    outcome: PollOutcome::Unmodified {
                        status: status.clone(),
                    },
                    delay: success_delay(status.as_ref(), config),
                }
            } else {
                // A restarted client has nothing on the panel. The scheduled
                // endpoint legitimately answers 304 before the next cycle, so
                // ask for the published frame explicitly.
                match client.frame(None, true) {
                    Ok(debug) if debug.status == 200 => {
                        display_response(client, display, state, debug, config)
                    }
                    Ok(debug) if debug.status == 503 => unavailable(client, state, debug),
                    Ok(debug) => server_failure(client, state, debug),
                    Err(error) => transport_failure(state, error),
                }
            }
        }
        503 => unavailable(client, state, response),
        _ => server_failure(client, state, response),
    }
}

fn display_response(
    client: &ServerClient,
    display: &mut dyn FrameDisplay,
    state: &mut PollState,
    response: HttpResponse,
    config: &ServerConfig,
) -> PollResult {
    let bytes = response.body.len();
    let etag = response.etag;

    match display.display(&response.body) {
        Ok(()) => {
            state.displayed_etag = etag;
            state.failures = 0;
            let status = fetch_status(client);
            PollResult {
                outcome: PollOutcome::Displayed {
                    bytes,
                    status: status.clone(),
                },
                delay: success_delay(status.as_ref(), config),
            }
        }
        Err(error) => {
            state.failures = state.failures.saturating_add(1);
            let status = fetch_status(client);
            PollResult {
                outcome: PollOutcome::DisplayFailed {
                    message: error.to_string(),
                    status,
                },
                delay: error_delay(state.failures),
            }
        }
    }
}

fn unavailable(client: &ServerClient, state: &mut PollState, response: HttpResponse) -> PollResult {
    let readiness_reason = client
        .readyz()
        .ok()
        .filter(|response| response.status != 200)
        .map(|response| response.body_text())
        .unwrap_or_default();
    let status = fetch_status(client);

    let reason = if readiness_reason.is_empty() {
        status
            .as_ref()
            .map(|status| status.ready_reason.clone())
            .filter(|reason| !reason.is_empty())
            .unwrap_or_else(|| response.body_text())
    } else {
        readiness_reason
    };

    state.failures = state.failures.saturating_add(1);
    let delay = response
        .retry_after
        .unwrap_or_else(|| error_delay(state.failures))
        .clamp(MIN_POLL, MAX_SLEEP);

    PollResult {
        outcome: PollOutcome::NotReady {
            reason,
            retry_after: response.retry_after,
            status,
        },
        delay,
    }
}

fn server_failure(
    client: &ServerClient,
    state: &mut PollState,
    response: HttpResponse,
) -> PollResult {
    let status_report = fetch_status(client);
    state.failures = state.failures.saturating_add(1);
    let delay = response
        .retry_after
        .unwrap_or_else(|| error_delay(state.failures))
        .clamp(MIN_POLL, MAX_SLEEP);

    PollResult {
        outcome: PollOutcome::ServerError {
            status: response.status,
            body: response.body_text(),
            retry_after: response.retry_after,
            status_report,
        },
        delay,
    }
}

fn transport_failure(state: &mut PollState, error: HttpError) -> PollResult {
    state.failures = state.failures.saturating_add(1);
    PollResult {
        outcome: PollOutcome::TransportError {
            message: error.to_string(),
        },
        delay: error_delay(state.failures),
    }
}

fn fetch_status(client: &ServerClient) -> Option<ServerStatus> {
    match client.status() {
        Ok(response) if response.is_success() => match serde_json::from_slice(&response.body) {
            Ok(status) => Some(status),
            Err(error) => {
                tracing::warn!("cannot parse /status: {error}");
                None
            }
        },
        Ok(response) => {
            tracing::warn!(status = response.status, "GET /status failed");
            None
        }
        Err(error) => {
            tracing::warn!("GET /status failed: {error}");
            None
        }
    }
}

fn success_delay(status: Option<&ServerStatus>, config: &ServerConfig) -> Duration {
    let Some(next_attempt) = status.and_then(|status| status.next_attempt) else {
        return config.poll_interval;
    };

    let seconds = next_attempt.signed_duration_since(Utc::now()).num_seconds();
    if seconds <= 0 {
        return MIN_POLL;
    }

    let upper = config
        .poll_interval
        .saturating_mul(4)
        .min(MAX_SUCCESS_SLEEP)
        .max(config.poll_interval)
        .max(MIN_POLL);
    Duration::from_secs((seconds as u64).clamp(MIN_POLL.as_secs(), upper.as_secs()))
}

fn error_delay(failures: u32) -> Duration {
    let shift = failures.saturating_sub(1).min(5);
    let factor = 1u64 << shift;
    let base = 60u64;
    Duration::from_secs(base.saturating_mul(factor).min(MAX_SLEEP.as_secs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::DisplayError;
    use crate::testsupport::{rgb_png, TestResponse, TestServer};
    use url::Url;

    struct RecordingDisplay {
        frames: Vec<Vec<u8>>,
        fail: bool,
    }

    impl RecordingDisplay {
        fn new() -> Self {
            RecordingDisplay {
                frames: Vec::new(),
                fail: false,
            }
        }
    }

    impl FrameDisplay for RecordingDisplay {
        fn display(&mut self, png: &[u8]) -> Result<(), DisplayError> {
            if self.fail {
                return Err(DisplayError::Png("display failed".to_string()));
            }
            self.frames.push(png.to_vec());
            Ok(())
        }
    }

    fn config(base_url: Url) -> ServerConfig {
        ServerConfig {
            base_url,
            poll_interval: Duration::from_secs(120),
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
        }
    }

    fn png() -> Vec<u8> {
        rgb_png(
            2,
            2,
            &[[0, 0, 0], [255, 255, 255], [255, 0, 0], [0, 255, 0]],
        )
    }

    fn status_json(next_attempt: chrono::DateTime<Utc>) -> String {
        format!(
            r#"{{"version":"0.1.0","mode":"Clear","mode_select":"Auto","ready":true,"ready_reason":"ready","failures":0,"next_attempt":"{}","frame":{{"available":true,"age_seconds":1,"bytes":4,"etag":"\"new\""}},"counters":{{"cycles_total":1}}}}"#,
            next_attempt.to_rfc3339()
        )
    }

    #[test]
    fn displays_a_200_and_uses_the_server_next_attempt() {
        let next = Utc::now() + chrono::Duration::seconds(60);
        let body = status_json(next);
        let image = png();
        let server = TestServer::spawn(move |request| match request.path.as_str() {
            "/healthz" => TestResponse::new(200, b"ok\n".to_vec()),
            "/frame" => TestResponse::new(200, image.clone()).header("ETag", "\"new\""),
            "/status" => TestResponse::new(200, body.clone()),
            other => panic!("unexpected path {other}"),
        });

        let client = ServerClient::new(&config(server.base_url()));
        let mut display = RecordingDisplay::new();
        let mut state = PollState::default();
        let result = poll_once(
            &client,
            &mut display,
            &mut state,
            &config(server.base_url()),
        );

        assert!(matches!(result.outcome, PollOutcome::Displayed { .. }));
        assert_eq!(display.frames, vec![png()]);
        assert_eq!(state.displayed_etag.as_deref(), Some("\"new\""));
        assert!((50..=60).contains(&result.delay.as_secs()));
        let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
        assert_eq!(paths, vec!["/healthz", "/frame", "/status"]);
        assert!(server.requests()[1].header("if-none-match").is_none());
    }

    #[test]
    fn keeps_the_frame_on_304() {
        let body = status_json(Utc::now() + chrono::Duration::seconds(60));
        let server = TestServer::spawn(move |request| match request.path.as_str() {
            "/healthz" => TestResponse::new(200, b"ok\n".to_vec()),
            "/frame" => {
                assert_eq!(request.header("if-none-match"), Some("\"old\""));
                TestResponse::new(304, Vec::new()).header("ETag", "\"old\"")
            }
            "/status" => TestResponse::new(200, body.clone()),
            other => panic!("unexpected path {other}"),
        });

        let client = ServerClient::new(&config(server.base_url()));
        let mut display = RecordingDisplay::new();
        let mut state = PollState {
            displayed_etag: Some("\"old\"".to_string()),
            failures: 0,
        };
        let result = poll_once(
            &client,
            &mut display,
            &mut state,
            &config(server.base_url()),
        );

        assert!(matches!(result.outcome, PollOutcome::Unmodified { .. }));
        assert!(display.frames.is_empty());
        assert_eq!(state.displayed_etag.as_deref(), Some("\"old\""));
    }

    #[test]
    fn a_cold_start_fetches_the_published_frame_after_304() {
        let body = status_json(Utc::now() + chrono::Duration::seconds(60));
        let image = png();
        let server = TestServer::spawn(move |request| match request.path.as_str() {
            "/healthz" => TestResponse::new(200, b"ok\n".to_vec()),
            "/frame" => TestResponse::new(304, Vec::new()),
            "/frame?debug=true" => {
                TestResponse::new(200, image.clone()).header("ETag", "\"debug\"")
            }
            "/status" => TestResponse::new(200, body.clone()),
            other => panic!("unexpected path {other}"),
        });

        let client = ServerClient::new(&config(server.base_url()));
        let mut display = RecordingDisplay::new();
        let mut state = PollState::default();
        let result = poll_once(
            &client,
            &mut display,
            &mut state,
            &config(server.base_url()),
        );

        assert!(matches!(result.outcome, PollOutcome::Displayed { .. }));
        assert_eq!(display.frames, vec![png()]);
        assert_eq!(state.displayed_etag.as_deref(), Some("\"debug\""));
        let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            vec!["/healthz", "/frame", "/frame?debug=true", "/status"]
        );
    }

    #[test]
    fn honors_503_retry_after_and_reports_readiness() {
        let body = r#"{"version":"0.1.0","mode":"Clear","mode_select":"Auto","ready":false,"ready_reason":"frame is stale","failures":2,"frame":{"available":false},"counters":{}}"#;
        let server = TestServer::spawn(move |request| match request.path.as_str() {
            "/healthz" => TestResponse::new(200, b"ok\n".to_vec()),
            "/frame" => TestResponse::new(503, b"upstream unavailable\n".to_vec())
                .header("Retry-After", "42"),
            "/readyz" => TestResponse::new(503, b"not ready: frame is stale\n".to_vec()),
            "/status" => TestResponse::new(200, body),
            other => panic!("unexpected path {other}"),
        });

        let client = ServerClient::new(&config(server.base_url()));
        let mut display = RecordingDisplay::new();
        let mut state = PollState::default();
        let result = poll_once(
            &client,
            &mut display,
            &mut state,
            &config(server.base_url()),
        );

        match result.outcome {
            PollOutcome::NotReady {
                reason,
                retry_after,
                ..
            } => {
                assert_eq!(reason, "not ready: frame is stale");
                assert_eq!(retry_after, Some(Duration::from_secs(42)));
            }
            other => panic!("unexpected outcome {other:?}"),
        }
        assert_eq!(result.delay, Duration::from_secs(42));
        assert!(display.frames.is_empty());
        assert_eq!(state.failures, 1);
        let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
        assert!(paths.contains(&"/readyz".to_string()));
    }

    #[test]
    fn backs_off_after_500() {
        let body = r#"{"version":"0.1.0","mode":"Clear","mode_select":"Auto","ready":false,"ready_reason":"no frame published","failures":1,"frame":{"available":false},"counters":{}}"#;
        let server = TestServer::spawn(move |request| match request.path.as_str() {
            "/healthz" => TestResponse::new(200, b"ok\n".to_vec()),
            "/frame" => TestResponse::new(500, b"internal server error\n".to_vec()),
            "/status" => TestResponse::new(200, body),
            other => panic!("unexpected path {other}"),
        });

        let client = ServerClient::new(&config(server.base_url()));
        let mut display = RecordingDisplay::new();
        let mut state = PollState::default();
        let result = poll_once(
            &client,
            &mut display,
            &mut state,
            &config(server.base_url()),
        );

        match result.outcome {
            PollOutcome::ServerError { status, .. } => assert_eq!(status, 500),
            other => panic!("unexpected outcome {other:?}"),
        }
        assert_eq!(result.delay, Duration::from_secs(60));
        assert_eq!(state.failures, 1);
    }

    #[test]
    fn skips_the_frame_request_when_healthz_fails() {
        let server = TestServer::spawn(move |request| match request.path.as_str() {
            "/healthz" => TestResponse::new(503, b"down\n".to_vec()),
            other => panic!("unexpected path {other}"),
        });

        let client = ServerClient::new(&config(server.base_url()));
        let mut display = RecordingDisplay::new();
        let mut state = PollState::default();
        let result = poll_once(
            &client,
            &mut display,
            &mut state,
            &config(server.base_url()),
        );

        assert!(matches!(
            result.outcome,
            PollOutcome::NotLive { status: 503, .. }
        ));
        assert_eq!(result.delay, Duration::from_secs(60));
        assert_eq!(state.failures, 1);
    }

    #[test]
    fn a_failed_display_does_not_advance_the_etag() {
        let body = status_json(Utc::now() + chrono::Duration::seconds(60));
        let image = png();
        let server = TestServer::spawn(move |request| match request.path.as_str() {
            "/healthz" => TestResponse::new(200, b"ok\n".to_vec()),
            "/frame" => TestResponse::new(200, image.clone()).header("ETag", "\"new\""),
            "/status" => TestResponse::new(200, body.clone()),
            other => panic!("unexpected path {other}"),
        });

        let client = ServerClient::new(&config(server.base_url()));
        let mut display = RecordingDisplay {
            frames: Vec::new(),
            fail: true,
        };
        let mut state = PollState::default();
        let result = poll_once(
            &client,
            &mut display,
            &mut state,
            &config(server.base_url()),
        );

        assert!(matches!(result.outcome, PollOutcome::DisplayFailed { .. }));
        assert!(state.displayed_etag.is_none());
        assert_eq!(state.failures, 1);
    }
}
