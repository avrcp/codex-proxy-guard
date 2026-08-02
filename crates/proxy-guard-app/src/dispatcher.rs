use std::sync::Arc;

use proxy_guard_core::{
    AppEffect, AppState, DesktopAppInfo, DesktopProcessState, GuardConfig, LaunchReceipt,
    TaskResult,
};
use proxy_guard_windows::{desktop_process_state, discover_desktop_app, launch_codex};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct EffectDispatcher {
    tx: mpsc::Sender<TaskResult>,
    cancellation: CancellationToken,
    cached_app: Arc<Mutex<Option<DesktopAppInfo>>>,
}

impl EffectDispatcher {
    pub fn new(tx: mpsc::Sender<TaskResult>, cancellation: CancellationToken) -> Self {
        Self {
            tx,
            cancellation,
            cached_app: Arc::new(Mutex::new(None)),
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
        tokio::spawn(async move {
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
                AppEffect::LaunchDesktop => {
                    let cached = cached_app.lock().await.clone();
                    let result = launch_pipeline(&config, cached.as_ref(), &cancellation).await;
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
    }
}

pub async fn launch_pipeline(
    config: &GuardConfig,
    cached: Option<&DesktopAppInfo>,
    cancellation: &CancellationToken,
) -> Result<(DesktopAppInfo, LaunchReceipt), String> {
    config.validate().map_err(|error| error.to_string())?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    let info = discover_desktop_app(config, cached, cancellation).await?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    let receipt = launch_codex(&info, config)?;
    Ok((info, receipt))
}
