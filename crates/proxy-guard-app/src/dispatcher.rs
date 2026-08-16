use std::sync::Arc;

use chrono::Utc;
use proxy_guard_core::{
    AppEffect, AppState, DesktopAppInfo, DesktopProcessState, GuardConfig, LaunchReceipt,
    ManagedLaunchReceipt, ManagedView, NodeId, SubscriptionId, TaskResult,
};
use proxy_guard_network::{LoopbackProxyEndpoint, SingBoxProcess};
use proxy_guard_windows::{
    desktop_process_state, discover_desktop_app, launch_codex, launch_codex_with_proxy,
};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::managed_services::{benchmark_service, node_store};
use crate::web::{ManagerServer, manager_info, reopen_browser};

struct ManagedSidecar {
    process: SingBoxProcess,
    endpoint: LoopbackProxyEndpoint,
}

/// One running Local Web Manager owned by the dispatcher.
struct ManagerHandle {
    open_url: String,
    shutdown: CancellationToken,
}

#[derive(Clone)]
pub struct EffectDispatcher {
    tx: mpsc::Sender<TaskResult>,
    cancellation: CancellationToken,
    benchmark_cancel: Arc<Mutex<CancellationToken>>,
    cached_app: Arc<Mutex<Option<DesktopAppInfo>>>,
    sidecar: Arc<Mutex<Option<ManagedSidecar>>>,
    manager: Arc<Mutex<Option<ManagerHandle>>>,
}

impl EffectDispatcher {
    pub fn new(tx: mpsc::Sender<TaskResult>, cancellation: CancellationToken) -> Self {
        Self {
            tx,
            benchmark_cancel: Arc::new(Mutex::new(cancellation.child_token())),
            cancellation,
            cached_app: Arc::new(Mutex::new(None)),
            sidecar: Arc::new(Mutex::new(None)),
            manager: Arc::new(Mutex::new(None)),
        }
    }

    pub fn dispatch(&self, effect: AppEffect, state: &AppState) {
        if matches!(effect, AppEffect::Shutdown) {
            self.cancellation.cancel();
            return;
        }
        let tx = self.tx.clone();
        let cancellation = self.cancellation.child_token();
        let benchmark_cancel = Arc::clone(&self.benchmark_cancel);
        let config = state.config.clone();
        let config_path = state.config_path.clone();
        let cached_app = Arc::clone(&self.cached_app);
        let sidecar = Arc::clone(&self.sidecar);
        let manager = Arc::clone(&self.manager);
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
                    let managed = load_managed_view(&config, &sidecar).await;
                    TaskResult::LocalStateRefreshed {
                        desktop_app,
                        process,
                        managed,
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
                AppEffect::SyncSubscription(id) => {
                    let result = tokio::task::spawn_blocking(move || {
                        crate::managed_services::subscription_service()
                            .map_err(|error| error.to_string())
                            .and_then(|service| {
                                service
                                    .sync(&id.to_string())
                                    .map_err(|error| error.to_string())
                            })
                    })
                    .await
                    .map_err(|error| format!("subscription sync task failed: {error}"))
                    .and_then(|result| result);
                    TaskResult::SubscriptionSynced(result)
                }
                AppEffect::BenchmarkNodes => {
                    *benchmark_cancel.lock().await = cancellation.child_token();
                    let token = benchmark_cancel.lock().await.clone();
                    let result = run_benchmark(&config, &token).await;
                    TaskResult::BenchmarkCompleted(result)
                }
                AppEffect::CancelBenchmark => {
                    benchmark_cancel.lock().await.cancel();
                    return;
                }
                AppEffect::LaunchManaged(node_id) => {
                    let result =
                        managed_launch(&config, node_id, &sidecar, &cached_app, &cancellation, &tx)
                            .await;
                    TaskResult::ManagedLaunchCompleted(result)
                }
                AppEffect::StopManagedProxy => {
                    let result = stop_managed_proxy(&sidecar).await;
                    TaskResult::ManagedProxyStopped(result)
                }
                AppEffect::OpenManager => {
                    run_manager(config, config_path, tx.clone(), cancellation, manager).await;
                    return;
                }
                AppEffect::ReopenManager => {
                    reopen_manager(manager).await;
                    return;
                }
                AppEffect::CloseManager => {
                    close_manager(manager, tx.clone()).await;
                    return;
                }
                AppEffect::Shutdown => return,
            };
            let _ = tx.send(result).await;
        });
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.cancellation.cancel();
        self.benchmark_cancel.lock().await.cancel();
        stop_managed_proxy(&self.sidecar).await
    }
}

/// Bind the loopback manager, open the browser, then own the server until it is
/// closed or Guard shuts down. Reports open/close back to the TUI event loop.
async fn run_manager(
    config: GuardConfig,
    config_path: std::path::PathBuf,
    tx: mpsc::Sender<TaskResult>,
    parent_cancellation: CancellationToken,
    manager: Arc<Mutex<Option<ManagerHandle>>>,
) {
    let server =
        match ManagerServer::start(config, config_path, tx.clone(), &parent_cancellation).await {
            Ok(server) => server,
            Err(error) => {
                let _ = tx.send(TaskResult::ManagerOpened(Err(error))).await;
                return;
            }
        };
    if let Err(error) = server.open_browser() {
        server.shutdown.cancel();
        let _ = server.task.await;
        let _ = tx.send(TaskResult::ManagerOpened(Err(error))).await;
        return;
    }
    let display_url = server.display_url.clone();
    let open_url = server.open_url.clone();
    let shutdown = server.shutdown.clone();
    let task = server.task;
    *manager.lock().await = Some(ManagerHandle {
        open_url,
        shutdown: shutdown.clone(),
    });
    let _ = tx
        .send(TaskResult::ManagerOpened(Ok(manager_info(&display_url))))
        .await;
    shutdown.cancelled().await;
    let _ = task.await;
    manager.lock().await.take();
    let _ = tx.send(TaskResult::ManagerClosed).await;
}

async fn reopen_manager(manager: Arc<Mutex<Option<ManagerHandle>>>) {
    let open_url = manager
        .lock()
        .await
        .as_ref()
        .map(|handle| handle.open_url.clone());
    let Some(open_url) = open_url else {
        return;
    };
    let _ = reopen_browser(&open_url);
}

async fn close_manager(manager: Arc<Mutex<Option<ManagerHandle>>>, tx: mpsc::Sender<TaskResult>) {
    let Some(handle) = manager.lock().await.take() else {
        let _ = tx.send(TaskResult::ManagerClosed).await;
        return;
    };
    handle.shutdown.cancel();
}

async fn load_managed_view(
    config: &GuardConfig,
    sidecar: &Arc<Mutex<Option<ManagedSidecar>>>,
) -> Result<ManagedView, String> {
    let mut view =
        crate::managed_services::load_managed_view(config).map_err(|error| error.to_string())?;
    view.proxy_endpoint = sidecar
        .lock()
        .await
        .as_ref()
        .map(|sidecar| sidecar.endpoint.proxy_url());
    Ok(view)
}

async fn run_benchmark(
    config: &GuardConfig,
    cancellation: &CancellationToken,
) -> Result<proxy_guard_core::BenchmarkRunSummary, String> {
    let config = config.clone();
    let subscription_id = config
        .managed
        .subscription_id
        .parse::<SubscriptionId>()
        .ok();
    let service = tokio::task::spawn_blocking(move || {
        benchmark_service(&config).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("benchmark service task failed: {error}"))??;
    service
        .run(subscription_id, cancellation)
        .await
        .map_err(|error| error.to_string())
}

async fn managed_launch(
    config: &GuardConfig,
    node_id: NodeId,
    sidecar: &Arc<Mutex<Option<ManagedSidecar>>>,
    cached_app: &Arc<Mutex<Option<DesktopAppInfo>>>,
    cancellation: &CancellationToken,
    tx: &mpsc::Sender<TaskResult>,
) -> Result<ManagedLaunchReceipt, String> {
    let config = config.clone();
    let config_for_blocking = config.clone();
    let verification_cancellation = cancellation.clone();
    let (process, endpoint, selection) = tokio::task::spawn_blocking(move || {
        let store = node_store().map_err(|error| error.to_string())?;
        let node = store
            .get(node_id)
            .map(|stored| stored.node)
            .map_err(|error| error.to_string())?;
        let service = benchmark_service(&config_for_blocking).map_err(|error| error.to_string())?;
        let selection = service
            .node_selection(&node, Utc::now())
            .map_err(|error| error.to_string())?;
        let verified = service
            .start_verified_sidecar(&node, &verification_cancellation)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((verified.process, verified.endpoint, selection))
    })
    .await
    .map_err(|error| format!("sidecar start task failed: {error}"))??;

    let proxy_url = endpoint.proxy_url();
    let launch = launch_pipeline_with_proxy(&config, &proxy_url, cached_app, cancellation).await;
    let (_info, receipt) = match launch {
        Ok(ok) => ok,
        Err(error) => {
            let _ = tokio::task::spawn_blocking(move || process.terminate()).await;
            return Err(error);
        }
    };

    *sidecar.lock().await = Some(ManagedSidecar { process, endpoint });
    spawn_sidecar_monitor(sidecar, tx.clone());
    Ok(ManagedLaunchReceipt {
        pid: receipt.pid,
        proxy_endpoint: receipt.proxy_endpoint,
        node: selection,
        desktop: receipt.desktop,
    })
}

fn spawn_sidecar_monitor(
    sidecar: &Arc<Mutex<Option<ManagedSidecar>>>,
    tx: mpsc::Sender<TaskResult>,
) {
    let sidecar = Arc::clone(sidecar);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let mut guard = sidecar.lock().await;
            let Some(managed) = guard.as_mut() else {
                break;
            };
            match managed.process.try_wait() {
                Ok(Some(_)) => {
                    *guard = None;
                    let _ = tx
                        .send(TaskResult::ManagedProxyLost(
                            "MANAGED_PROXY_LOST: the managed proxy stopped unexpectedly; Desktop remains open"
                                .into(),
                        ))
                        .await;
                    break;
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }
    });
}

async fn launch_pipeline_with_proxy(
    config: &GuardConfig,
    proxy_url: &str,
    cached_app: &Arc<Mutex<Option<DesktopAppInfo>>>,
    cancellation: &CancellationToken,
) -> Result<(DesktopAppInfo, LaunchReceipt), String> {
    config.validate().map_err(|error| error.to_string())?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    let cached = cached_app.lock().await.clone();
    let info = discover_desktop_app(config, cached.as_ref(), cancellation).await?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    let receipt = launch_codex_with_proxy(&info, config, proxy_url)?;
    Ok((info, receipt))
}

async fn stop_managed_proxy(sidecar: &Arc<Mutex<Option<ManagedSidecar>>>) -> Result<(), String> {
    let managed = sidecar.lock().await.take();
    let Some(managed) = managed else {
        return Ok(());
    };
    tokio::task::spawn_blocking(move || managed.process.terminate())
        .await
        .map_err(|error| format!("sidecar termination task failed: {error}"))?
        .map_err(|error| error.to_string())?;
    Ok(())
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
