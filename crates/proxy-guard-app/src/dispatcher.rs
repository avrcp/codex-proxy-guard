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

use crate::commands::{benchmark_service, node_store, subscription_service};

struct ManagedSidecar {
    process: SingBoxProcess,
    endpoint: LoopbackProxyEndpoint,
}

#[derive(Clone)]
pub struct EffectDispatcher {
    tx: mpsc::Sender<TaskResult>,
    cancellation: CancellationToken,
    benchmark_cancel: Arc<Mutex<CancellationToken>>,
    cached_app: Arc<Mutex<Option<DesktopAppInfo>>>,
    sidecar: Arc<Mutex<Option<ManagedSidecar>>>,
}

impl EffectDispatcher {
    pub fn new(tx: mpsc::Sender<TaskResult>, cancellation: CancellationToken) -> Self {
        Self {
            tx,
            benchmark_cancel: Arc::new(Mutex::new(cancellation.child_token())),
            cancellation,
            cached_app: Arc::new(Mutex::new(None)),
            sidecar: Arc::new(Mutex::new(None)),
        }
    }

    pub fn dispatch(&self, effect: AppEffect, state: &AppState) {
        if matches!(effect, AppEffect::Shutdown) {
            self.cancellation.cancel();
            let sidecar = Arc::clone(&self.sidecar);
            tokio::spawn(async move {
                let _ = stop_managed_proxy(&sidecar).await;
            });
            return;
        }
        let tx = self.tx.clone();
        let cancellation = self.cancellation.child_token();
        let benchmark_cancel = Arc::clone(&self.benchmark_cancel);
        let config = state.config.clone();
        let config_path = state.config_path.clone();
        let cached_app = Arc::clone(&self.cached_app);
        let sidecar = Arc::clone(&self.sidecar);
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
                        subscription_service()
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
                AppEffect::Shutdown => return,
            };
            let _ = tx.send(result).await;
        });
    }
}

async fn load_managed_view(
    config: &GuardConfig,
    sidecar: &Arc<Mutex<Option<ManagedSidecar>>>,
) -> Result<ManagedView, String> {
    if !config.is_managed() {
        return Ok(ManagedView::default());
    }
    let subscription_id = config
        .managed
        .subscription_id
        .parse::<SubscriptionId>()
        .ok();
    let subscription_name = subscription_service()
        .map_err(|error| error.to_string())?
        .list()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|stored| Some(stored.source.id) == subscription_id)
        .map(|stored| stored.source.name);
    let (regions, selected) = benchmark_service(config)
        .map_err(|error| error.to_string())?
        .snapshot(subscription_id, Utc::now())
        .map_err(|error| error.to_string())?;
    let proxy_endpoint = sidecar
        .lock()
        .await
        .as_ref()
        .map(|sidecar| sidecar.endpoint.proxy_url());
    Ok(ManagedView {
        subscription_name,
        regions,
        selected,
        proxy_endpoint,
        proxy_lost: false,
    })
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
        let (process, endpoint) = service
            .start_sidecar(&node)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((process, endpoint, selection))
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
    let mut guard = sidecar.lock().await;
    let Some(managed) = guard.take() else {
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
