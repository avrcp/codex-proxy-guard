use std::{io::Write, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use proxy_guard_core::{GuardConfig, SubscriptionId};
use proxy_guard_network::{
    HttpsSubscriptionFetcher, KeyringSecretStore, ManagedPaths, NodeBenchmarkService, NodeStore,
    ReqwestCodexPathProbe, SingBoxInstallation, SingBoxLocator, SubscriptionService,
};

pub fn managed_paths() -> anyhow::Result<ManagedPaths> {
    let paths = ManagedPaths::discover().context("resolve managed data directory")?;
    paths
        .ensure_layout()
        .context("create managed data directory")?;
    Ok(paths)
}

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

pub fn node_store() -> anyhow::Result<NodeStore> {
    let paths = managed_paths()?;
    NodeStore::open(&paths).context("open node store")
}

fn configured_subscription(config: &GuardConfig) -> Option<SubscriptionId> {
    config
        .managed
        .subscription_id
        .parse::<SubscriptionId>()
        .ok()
}

pub fn cmd_subscription_add(name: &str, url: &str) -> anyhow::Result<()> {
    let service = subscription_service()?;
    let stored = service.add(name, url).context("add subscription")?;
    println!(
        "Added subscription \"{}\" ({})",
        stored.source.name, stored.source.id
    );
    println!(
        "Run `codex-proxy-guard subscription sync \"{}\"` to import nodes.",
        stored.source.name
    );
    println!("To use it in Managed Mode, set in config.toml:");
    println!("  proxy.mode = \"managed\"");
    println!("  managed.subscription_id = \"{}\"", stored.source.id);
    Ok(())
}

pub fn cmd_subscription_list() -> anyhow::Result<()> {
    let service = subscription_service()?;
    let subscriptions = service.list().context("list subscriptions")?;
    if subscriptions.is_empty() {
        println!("No subscriptions.");
        return Ok(());
    }
    for stored in &subscriptions {
        let active = stored
            .bindings
            .iter()
            .filter(|binding| binding.state == proxy_guard_core::SubscriptionNodeState::Active)
            .count();
        println!(
            "{:<24} {:<12} {} active nodes  ({})",
            stored.source.name,
            format!("{:?}", stored.source.last_sync_status),
            active,
            stored.source.id
        );
    }
    Ok(())
}

pub fn cmd_subscription_sync(reference: &str) -> anyhow::Result<()> {
    let service = subscription_service()?;
    let summary = service.sync(reference).context("sync subscription")?;
    println!(
        "Fetched       {}\nImported      {}\nUpdated       {}\nStale         {}\nUnsupported   {}\nIgnoredRegion {}\nFailed        {}",
        summary.fetched,
        summary.imported,
        summary.updated,
        summary.stale,
        summary.unsupported,
        summary.ignored_region,
        summary.failed,
    );
    Ok(())
}

pub fn cmd_subscription_delete(reference: &str, yes: bool) -> anyhow::Result<()> {
    if !yes {
        print!("Delete subscription \"{reference}\" and its credential? [y/N] ");
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).ok();
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }
    let service = subscription_service()?;
    let id = service.delete(reference).context("delete subscription")?;
    println!("Deleted subscription {id}.");
    Ok(())
}

pub fn cmd_node_list(region: Option<proxy_guard_core::CodexRegion>) -> anyhow::Result<()> {
    let paths = managed_paths()?;
    let store = NodeStore::open(&paths).context("open node store")?;
    let mut nodes = store.list().context("list nodes")?;
    nodes.retain(|stored| region.is_none_or(|region| stored.node.region_hint == region));
    if nodes.is_empty() {
        println!("No nodes imported. Run `codex-proxy-guard subscription sync` first.");
        return Ok(());
    }
    for stored in &nodes {
        println!(
            "{:<3} {:<6} {:<24} ({})",
            stored.node.region_hint,
            format!("{:?}", stored.node.state),
            stored.node.name,
            stored.node.id
        );
    }
    Ok(())
}

pub async fn cmd_benchmark(config: &GuardConfig, json: bool) -> anyhow::Result<()> {
    let service = benchmark_service(config)?;
    let subscription = configured_subscription(config);
    let cancellation = tokio_util::sync::CancellationToken::new();
    let summary = service
        .run(subscription, &cancellation)
        .await
        .context("benchmark")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        print_summary(&summary);
    }
    Ok(())
}

pub fn cmd_best_node(config: &GuardConfig) -> anyhow::Result<()> {
    let service = benchmark_service(config)?;
    let subscription = configured_subscription(config);
    let Some((selection, _node)) = service
        .select_cached(subscription, chrono::Utc::now())
        .context("read cached selection")?
    else {
        bail!("NoHealthyManagedNode: no fresh JP/SG/US benchmark cache. Run `benchmark` first.");
    };
    println!("{}", selection.name);
    println!("Region: {}", selection.region);
    println!("Score: {}", selection.score);
    println!("Success: {}%", selection.success_percent);
    println!("Median: {} ms", selection.median_ms);
    println!("P95: {} ms", selection.p95_ms);
    println!(
        "Exit: {}",
        if selection.exit_stable {
            "stable"
        } else {
            "changed"
        }
    );
    Ok(())
}

fn print_summary(summary: &proxy_guard_core::BenchmarkRunSummary) {
    println!(
        "Scanned {} active nodes, {} quick-rejected, {} deep-scanned, {} healthy",
        summary.scanned, summary.quick_rejected, summary.deep_scanned, summary.healthy
    );
    println!(
        "JP  {} active  {} healthy",
        summary.regions.jp_active, summary.regions.jp_healthy
    );
    println!(
        "SG  {} active  {} healthy",
        summary.regions.sg_active, summary.regions.sg_healthy
    );
    println!(
        "US  {} active  {} healthy",
        summary.regions.us_active, summary.regions.us_healthy
    );
    match &summary.selected {
        Some(selection) => {
            println!();
            println!(
                "Selected: {} ({}) score {}",
                selection.name, selection.region, selection.score
            );
        }
        None => {
            bail_no_healthy();
        }
    }
}

fn bail_no_healthy() {
    eprintln!("NoHealthyManagedNode: no JP/SG/US node passed the health gates.");
}
