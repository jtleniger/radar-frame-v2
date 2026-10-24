//! The HTTP surface: `/`, `/settings`, `/frame`, `/status`, and probes.

pub mod handlers;
pub mod pages;
pub mod status;
pub mod updater;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State as AxumState;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::{mpsc, oneshot, watch};

use crate::config::Paths;
use crate::config::Settings;
use crate::error::{Error, Result};
use crate::service::clock::Clock;
use crate::service::frame::PublishedFrame;
use crate::service::renderer::Renderer;
use crate::sources::{RadarSource, Sources};
use crate::state::{Mode, ModeSelect, State};
use crate::telemetry::metrics::Metrics;

use self::updater::{Command, Snapshot, Update, Updater};

/// Upper bound for one update cycle. Each upstream call is bounded by the
/// HTTP client timeouts; this caps the whole sequence plus rendering so the
/// updater always becomes responsive again.
const CYCLE_DEADLINE: Duration = Duration::from_secs(180);

/// How often the updater wakes without a poll, so the schedule keeps running
/// when the display client is offline.
const TICK: Duration = Duration::from_secs(60);

/// How long a `/frame` poll waits for the updater to finish the work it woke.
const UPDATE_DEADLINE: Duration = Duration::from_secs(185);

pub struct AppState {
    pub settings: Settings,
    pub paths: Paths,
    pub clock: Arc<dyn Clock>,
    pub sources: Sources,
    pub radar: Arc<dyn RadarSource>,
    pub renderer: Arc<dyn Renderer>,
    pub metrics: Arc<Metrics>,
    snapshot: watch::Receiver<Arc<Snapshot>>,
    commands: mpsc::UnboundedSender<Command>,
}

impl AppState {
    /// Build the shared state and start the updater task.
    ///
    /// Must be called inside a Tokio runtime: the updater runs as its own
    /// task and exits when this value (and with it the command sender) drops.
    pub fn new(
        settings: Settings,
        paths: Paths,
        clock: Arc<dyn Clock>,
        sources: Sources,
        radar: Arc<dyn RadarSource>,
        renderer: Arc<dyn Renderer>,
    ) -> Self {
        let started_at = clock.now();

        // A frame left by the previous process is still the last good one;
        // publish it so the first poll survives an upstream outage. The
        // persisted schedule is restored with it, so a restart does not
        // immediately repeat upstream work.
        let mut state = State::load(&paths.state_file());
        state.frame = PublishedFrame::load(&paths.output_img()).map(Arc::new);

        let (snapshots, snapshot) = watch::channel(Arc::new(Snapshot {
            state: state.clone(),
        }));
        let (commands, receiver) = mpsc::unbounded_channel();
        let metrics = Arc::new(Metrics::new(started_at));

        let updater = Updater {
            settings: settings.clone(),
            paths: paths.clone(),
            clock: Arc::clone(&clock),
            sources: sources.clone(),
            radar: Arc::clone(&radar),
            renderer: Arc::clone(&renderer),
            metrics: Arc::clone(&metrics),
            state,
            snapshots,
        };
        tokio::spawn(updater.run(receiver));

        AppState {
            settings,
            paths,
            clock,
            sources,
            radar,
            renderer,
            metrics,
            snapshot,
            commands,
        }
    }

    /// The schedule and frame published by the last completed update.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.borrow().clone()
    }

    /// Ask the updater to run whatever is due and return the outcome.
    ///
    /// Concurrent polls queue behind the single updater, so only one of them
    /// performs the status probe and the cycle; the rest see the refreshed
    /// schedule and report [`Update::Skipped`].
    async fn update(&self) -> Update {
        let (reply, response) = oneshot::channel();
        if self.commands.send(Command::Update { reply }).is_err() {
            return Update::Failed(Error::new("updater stopped"));
        }
        match tokio::time::timeout(UPDATE_DEADLINE, response).await {
            Ok(Ok(update)) => update,
            _ => Update::Failed(Error::new(format!(
                "update exceeded the {} s deadline",
                UPDATE_DEADLINE.as_secs()
            ))),
        }
    }

    /// Apply a `/settings` change through the updater. Returns false when the
    /// updater has stopped.
    async fn apply_settings(&self, mode_select: ModeSelect, mode: Option<Mode>) -> bool {
        let (reply, response) = oneshot::channel();
        if self
            .commands
            .send(Command::Settings {
                mode_select,
                mode,
                reply,
            })
            .is_err()
        {
            return false;
        }
        response.await.is_ok()
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(handlers::index))
        .route("/settings", post(handlers::settings))
        .route("/frame", get(handlers::frame))
        .route("/healthz", get(handlers::healthz))
        .route("/readyz", get(handlers::readyz))
        .route("/status", get(status::status))
        .route("/static/style.css", get(handlers::stylesheet))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            count_request,
        ))
        .with_state(state)
}

async fn count_request(
    AxumState(app): AxumState<Arc<AppState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    app.metrics.request();
    next.run(request).await
}

/// Bind and serve until the process is stopped.
pub async fn serve(state: Arc<AppState>, address: std::net::SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|e| Error::io(format!("{address}: {e}")))?;
    tracing::info!("listening on {address}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| Error::new(format!("server: {e}")))?;
    tracing::info!(event = "shutdown", "server stopped");
    Ok(())
}

/// Resolves on SIGTERM or SIGINT so `axum` drains in-flight requests instead
/// of dropping connections mid-response.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        tracing::info!(event = "shutdown", signal = "SIGINT", "stopping");
                    }
                    _ = terminate.recv() => {
                        tracing::info!(event = "shutdown", signal = "SIGTERM", "stopping");
                    }
                }
            }
            Err(error) => {
                tracing::error!(
                    event = "shutdown",
                    signal = "SIGTERM",
                    error = %error,
                    "signal handler unavailable; waiting for SIGINT"
                );
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!(event = "shutdown", signal = "SIGINT", "stopping");
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!(event = "shutdown", signal = "SIGINT", "stopping");
    }
}
