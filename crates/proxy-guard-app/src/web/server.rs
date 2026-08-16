use std::{path::PathBuf, sync::Arc, time::Duration};

use base64::Engine;
use proxy_guard_core::{GuardConfig, ManagerInfo, TaskResult};
use rand::TryRngCore;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{routes, state::ManagerState};

pub const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// A started Local Web Manager bound to one loopback ephemeral port.
pub struct ManagerServer {
    pub display_url: String,
    pub open_url: String,
    pub shutdown: CancellationToken,
    pub task: tokio::task::JoinHandle<()>,
}

impl ManagerServer {
    /// Bind `127.0.0.1:0`, mint a per-session 256-bit token, and serve the
    /// embedded manager until the shutdown token is cancelled.
    ///
    /// # Errors
    ///
    /// Returns a redacted bind or route construction error.
    pub async fn start(
        config: GuardConfig,
        config_path: PathBuf,
        tx: mpsc::Sender<TaskResult>,
        parent_shutdown: &CancellationToken,
    ) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("manager bind failed: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("manager resolve port failed: {error}"))?
            .port();
        let token = generate_token();
        let display_url = format!("http://127.0.0.1:{port}");
        let open_url = format!("{display_url}/#token={token}");
        let shutdown = parent_shutdown.child_token();
        let state = Arc::new(ManagerState::new(
            tx,
            config,
            config_path,
            token,
            port,
            shutdown.clone(),
        ));
        let app = routes::build_router(state.clone());
        let serve_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(serve_shutdown.cancelled_owned())
                .await;
        });
        spawn_idle_timeout(state.clone());
        Ok(Self {
            display_url,
            open_url,
            shutdown,
            task,
        })
    }

    /// Open the bootstrap URL in the default browser. Callers abort the manager
    /// when this fails so the capability token never reaches a log.
    ///
    /// # Errors
    ///
    /// Returns an error when the browser cannot be launched.
    pub fn open_browser(&self) -> Result<(), String> {
        webbrowser::open(&self.open_url)
            .map_err(|error| format!("could not open the browser manager: {error}"))
    }
}

fn generate_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS random source must be available");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Stop the server when the browser stops calling for 15 minutes, unless a
/// Web-initiated operation is still actively running.
fn spawn_idle_timeout(state: Arc<ManagerState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(IDLE_CHECK_INTERVAL).await;
            if state.shutdown.is_cancelled() {
                return;
            }
            let busy = matches!(
                *state.operation.lock().await,
                super::state::ManagerOperation::Benchmarking { .. }
                    | super::state::ManagerOperation::Syncing { .. }
            );
            if !busy {
                let idle = state.last_activity.lock().await.elapsed() >= IDLE_TIMEOUT;
                if idle {
                    state.shutdown.cancel();
                    return;
                }
            }
        }
    });
}

/// Reopen the browser tab for an already-running manager.
///
/// # Errors
///
/// Returns an error when the browser cannot be launched.
pub fn reopen_browser(open_url: &str) -> Result<(), String> {
    webbrowser::open(open_url)
        .map_err(|error| format!("could not reopen the browser manager: {error}"))
}

/// The minimal display payload sent back to the TUI.
pub fn manager_info(display_url: &str) -> ManagerInfo {
    ManagerInfo {
        display_url: display_url.to_owned(),
    }
}
