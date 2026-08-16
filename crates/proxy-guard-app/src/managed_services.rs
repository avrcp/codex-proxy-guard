use std::{sync::Arc, time::Duration};

use anyhow::Context;
use chrono::Utc;
use proxy_guard_core::{GuardConfig, ManagedView, SubscriptionId};
use proxy_guard_network::{
    HttpsSubscriptionFetcher, KeyringSecretStore, ManagedPaths, NodeBenchmarkService, NodeStore,
    ReqwestCodexPathProbe, SingBoxInstallation, SingBoxLocator, SubscriptionService,
};

/// Resolve and prepare the managed data directory.
///
/// # Errors
///
/// Returns a context error when the layout cannot be resolved or created.
pub fn managed_paths() -> anyhow::Result<ManagedPaths> {
    let paths = ManagedPaths::discover().context("resolve managed data directory")?;
    paths
        .ensure_layout()
        .context("create managed data directory")?;
    Ok(paths)
}

/// Open the credential-managed subscription service used by CLI, TUI, and Web.
///
/// # Errors
///
/// Returns a context error when the service cannot be opened.
pub fn subscription_service()
-> anyhow::Result<SubscriptionService<KeyringSecretStore, HttpsSubscriptionFetcher>> {
    let paths = managed_paths()?;
    SubscriptionService::open(&paths, KeyringSecretStore, HttpsSubscriptionFetcher::new()?)
        .context("open subscription service")
}

fn sing_box_installation(config: &GuardConfig) -> anyhow::Result<SingBoxInstallation> {
    let paths = managed_paths()?;
    let explicit = (!config.managed.sing_box_path.as_os_str().is_empty())
        .then_some(config.managed.sing_box_path.as_path());
    SingBoxLocator::resolve(&paths, explicit).context("resolve sing-box runtime")
}

/// Open the benchmark service bound to the configured sing-box installation.
///
/// # Errors
///
/// Returns a context error when the runtime cannot be resolved or the cache opened.
pub fn benchmark_service(config: &GuardConfig) -> anyhow::Result<NodeBenchmarkService> {
    let paths = managed_paths()?;
    let installation = sing_box_installation(config)?;
    let cache_ttl = Duration::from_secs(u64::from(config.managed.benchmark_cache_hours) * 3600);
    NodeBenchmarkService::new(
        &paths,
        installation,
        cache_ttl,
        Arc::new(ReqwestCodexPathProbe),
    )
    .context("open benchmark service")
}

/// Open the managed node repository.
///
/// # Errors
///
/// Returns a context error when the repository cannot be opened.
pub fn node_store() -> anyhow::Result<NodeStore> {
    let paths = managed_paths()?;
    NodeStore::open(&paths).context("open node store")
}

/// Resolve the configured subscription ID, if valid.
#[must_use]
pub fn configured_subscription(config: &GuardConfig) -> Option<SubscriptionId> {
    config
        .managed
        .subscription_id
        .parse::<SubscriptionId>()
        .ok()
}

/// Build the URL-free Managed Mode view shown to the TUI and Web.
///
/// # Errors
///
/// Returns a context error when the subscription metadata or benchmark cache
/// cannot be read.
pub fn load_managed_view(config: &GuardConfig) -> anyhow::Result<ManagedView> {
    if !config.is_managed() {
        return Ok(ManagedView::default());
    }
    let subscription_id = configured_subscription(config);
    let subscription_name = subscription_service()?
        .list()?
        .into_iter()
        .find(|stored| Some(stored.source.id) == subscription_id)
        .map(|stored| stored.source.name);
    let (regions, selected) = benchmark_service(config)?
        .snapshot(subscription_id, Utc::now())
        .context("read benchmark snapshot")?;
    Ok(ManagedView {
        subscription_name,
        regions,
        selected,
        proxy_endpoint: None,
        proxy_lost: false,
        manual_selection: None,
    })
}
