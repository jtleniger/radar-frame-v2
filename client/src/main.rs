//! Polls the radar-frame server and refreshes the Waveshare panel.

use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;

use clap::Parser;
use radar_frame_client::config::{Config, ConfigError};
use radar_frame_client::display::{DisplayError, EpdDisplay, FrameDisplay, NoopDisplay};
use radar_frame_client::http::ServerClient;
use radar_frame_client::logging;
use radar_frame_client::poll::{poll_once, PollOutcome, PollResult, PollState};
use radar_frame_client::status::ServerStatus;
use thiserror::Error;

#[derive(Debug, Parser)]
#[command(name = "radar-frame-client", version, about)]
struct Args {
    /// Path to the client INI file.
    #[arg(short, long, default_value = "config/config.ini")]
    config: PathBuf,
    /// Do not touch SPI/GPIO; decode and log frames instead.
    #[arg(long)]
    noop: bool,
    /// Run exactly one poll and exit.
    #[arg(long)]
    once: bool,
}

#[derive(Debug, Error)]
enum ClientError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("logging: {0}")]
    Logging(String),
    #[error(transparent)]
    Display(#[from] DisplayError),
    #[error("poll failed")]
    PollFailed,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("radar-frame-client: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), ClientError> {
    let args = Args::parse();
    let config = Config::load(&args.config)?;
    logging::init(&config.logging.file, &config.logging.level).map_err(ClientError::Logging)?;

    let noop = args.noop || config.epaper.use_noop;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        base_url = %config.server.base_url,
        poll_seconds = config.server.poll_interval.as_secs(),
        display = if noop { "noop" } else { "epd7in3f" },
        "starting radar-frame-client"
    );

    let client = ServerClient::new(&config.server);
    let mut display: Box<dyn FrameDisplay> = if noop {
        Box::new(NoopDisplay)
    } else {
        Box::new(EpdDisplay::new()?)
    };

    let mut state = PollState::default();
    loop {
        let result = poll_once(&client, display.as_mut(), &mut state, &config.server);
        log_result(&result, &state);

        if args.once {
            return if result.outcome.is_success() {
                Ok(())
            } else {
                Err(ClientError::PollFailed)
            };
        }

        thread::sleep(result.delay);
    }
}

fn log_result(result: &PollResult, state: &PollState) {
    match &result.outcome {
        PollOutcome::Displayed { bytes, status } => {
            tracing::info!(bytes, "displayed new frame");
            log_status(status.as_ref());
        }
        PollOutcome::Unmodified { status } => {
            tracing::info!("server reports no new frame");
            log_status(status.as_ref());
        }
        PollOutcome::NotLive { status, body } => {
            tracing::error!(status, body, "server liveness check failed");
        }
        PollOutcome::NotReady {
            reason,
            retry_after,
            status,
        } => {
            tracing::warn!(
                reason,
                retry_after_seconds = retry_after.map(|delay| delay.as_secs()),
                "server is not ready"
            );
            log_status(status.as_ref());
        }
        PollOutcome::ServerError {
            status,
            body,
            retry_after,
            status_report,
        } => {
            tracing::error!(
                status,
                body,
                retry_after_seconds = retry_after.map(|delay| delay.as_secs()),
                "server returned an error"
            );
            log_status(status_report.as_ref());
        }
        PollOutcome::TransportError { message } => {
            tracing::error!(message, "request failed");
        }
        PollOutcome::DisplayFailed { message, status } => {
            tracing::error!(message, "display refresh failed");
            log_status(status.as_ref());
        }
    }

    tracing::debug!(
        failures = state.failures,
        delay_seconds = result.delay.as_secs(),
        "scheduling next poll"
    );
}

fn log_status(status: Option<&ServerStatus>) {
    let Some(status) = status else {
        return;
    };

    tracing::info!(
        version = %status.version,
        mode = %status.mode,
        mode_select = %status.mode_select,
        ready = status.ready,
        ready_reason = %status.ready_reason,
        failures = status.failures,
        frame_available = status.frame.available,
        frame_age_seconds = ?status.frame.age_seconds,
        frame_etag = ?status.frame.etag,
        radar_up = ?status.radar_status.as_ref().map(|radar| radar.up),
        next_attempt = ?status.next_attempt,
        "server status"
    );
}
