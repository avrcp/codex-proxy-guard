use std::{sync::Arc, time::Duration};

use proxy_guard_core::{
    AppEffect, AppState, DesktopAppInfo, DesktopProcessState, GuardConfig, LaunchOptions,
    LaunchReceipt, TaskResult,
};
use proxy_guard_windows::{desktop_process_state, discover_desktop_app, launch_codex};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

/// Bounded wait for the single foreground task to finish its cleanup when
/// Guard shuts down.
const SHUTDOWN_JOIN_BUDGET: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub struct EffectDispatcher {
    tx: mpsc::Sender<TaskResult>,
    cancellation: CancellationToken,
    cached_app: Arc<Mutex<Option<DesktopAppInfo>>>,
    foreground_task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl EffectDispatcher {
    pub fn new(tx: mpsc::Sender<TaskResult>, cancellation: CancellationToken) -> Self {
        Self {
            tx,
            cancellation,
            cached_app: Arc::new(Mutex::new(None)),
            foreground_task: Arc::new(Mutex::new(None)),
        }
    }

    pub fn dispatch(&self, effect: AppEffect, state: &AppState) {
        if matches!(effect, AppEffect::Shutdown) {
            self.cancellation.cancel();
            return;
        }
        let tx = self.tx.clone();
        let cancellation = self.cancellation.child_token();
        let config = state.config.clone();
        let config_path = state.config_path.clone();
        let cached_app = Arc::clone(&self.cached_app);
        let handle = tokio::spawn(async move {
            let result = match effect {
                AppEffect::RefreshLocalState => {
                    let desktop_app = discover_desktop_app(&config, None, &cancellation).await;
                    let process = desktop_app
                        .as_ref()
                        .map_or(DesktopProcessState::Unknown, |info| {
                            desktop_process_state(info)
                        });
                    if let Ok(info) = &desktop_app {
                        *cached_app.lock().await = Some(info.clone());
                    }
                    TaskResult::LocalStateRefreshed {
                        desktop_app,
                        process,
                    }
                }
                AppEffect::LaunchDesktop(options) => {
                    let result = launch_pipeline(&config, options, &cancellation).await;
                    if let Ok((info, _)) = &result {
                        *cached_app.lock().await = Some(info.clone());
                    }
                    TaskResult::LaunchCompleted(result)
                }
                AppEffect::SaveConfig(updated) => {
                    let result = tokio::task::spawn_blocking(move || {
                        updated.save(&config_path).map(|()| updated)
                    })
                    .await
                    .map_err(|error| format!("configuration save task failed: {error}"))
                    .and_then(|result| result.map_err(|error| error.to_string()));
                    TaskResult::ConfigSaved(result)
                }
                AppEffect::Shutdown => return,
            };
            let _ = tx.send(result).await;
        });
        // Only one foreground operation exists by design; if a stale handle is
        // somehow still present, stop it before tracking the new task.
        if let Ok(mut slot) = self.foreground_task.try_lock()
            && let Some(previous) = slot.replace(handle)
        {
            previous.abort();
        }
    }

    /// Waits (bounded) for the in-flight foreground task to finish its
    /// cleanup. Call after cancelling; never terminates Desktop or any other
    /// external process.
    pub async fn shutdown(&self) {
        let task = self.foreground_task.lock().await.take();
        if let Some(task) = task {
            let _ = timeout(SHUTDOWN_JOIN_BUDGET, task).await;
        }
    }
}

pub async fn launch_pipeline(
    config: &GuardConfig,
    options: LaunchOptions,
    cancellation: &CancellationToken,
) -> Result<(DesktopAppInfo, LaunchReceipt), String> {
    config.validate().map_err(|error| error.to_string())?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    // Launch always re-discovers the Desktop instead of trusting the cached
    // entry the TUI displays; an app update may have replaced the entry point.
    let info = discover_desktop_app(config, None, cancellation).await?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    let receipt = launch_codex(&info, config, options, cancellation).await?;
    Ok((info, receipt))
}
