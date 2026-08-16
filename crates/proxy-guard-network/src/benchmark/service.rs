use std::{
    collections::HashMap,
    fs,
    net::TcpStream,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::Utc;
use proxy_guard_core::{
    BenchmarkReport, BenchmarkRunSummary, CodexRegion, ExitObservation, ManagedNode,
    ManagedNodeState, NodeId, NodeSelection, PathSample, QuickVerdict, RegionBenchmarkCounts,
    SubscriptionId,
};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::fingerprint::node_fingerprint;
use super::probe::CodexPathProbe;
use super::scoring::{DeepScanInput, build_report};
use super::selector::NodeSelector;
use crate::storage::{BenchmarkStore, NodeStore};
use crate::{
    GeoResolver, LoopbackPortReservation, LoopbackProxyEndpoint, ManagedPaths, NetworkError,
    SingBoxConfigBuilder, SingBoxConfigFile, SingBoxInstallation, SingBoxProcess, SingBoxRuntime,
};

pub const SIDECAR_READY_TIMEOUT: Duration = Duration::from_secs(10);
pub const QUICK_SCAN_CONCURRENCY: usize = 3;
const DEEP_PROBE_INTERVAL: Duration = Duration::from_millis(400);

fn deep_limit(region: CodexRegion) -> usize {
    match region {
        CodexRegion::JP => 6,
        CodexRegion::SG => 3,
        CodexRegion::US => 3,
    }
}

/// Full two-stage JP/SG/US benchmark and cache-backed selection service.
#[derive(Clone)]
pub struct NodeBenchmarkService {
    runtime: SingBoxRuntime,
    resolver: GeoResolver,
    probe: Arc<dyn CodexPathProbe>,
    benchmarks: BenchmarkStore,
    nodes: NodeStore,
}

/// A launch-time verified sidecar whose exact process is retained for Desktop use.
#[derive(Debug)]
pub struct VerifiedSidecar {
    pub process: SingBoxProcess,
    pub endpoint: LoopbackProxyEndpoint,
    pub exit: ExitObservation,
    pub sample: PathSample,
}

/// One node and its current fingerprint-bound benchmark report, when present.
#[derive(Clone, Debug)]
pub struct NodeStatusView {
    pub node: ManagedNode,
    pub report: Option<BenchmarkReport>,
}

impl NodeBenchmarkService {
    /// Build the service from a discovered sing-box installation.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the managed layout cannot be opened.
    pub fn new(
        paths: &ManagedPaths,
        installation: SingBoxInstallation,
        cache_ttl: Duration,
        probe: Arc<dyn CodexPathProbe>,
    ) -> Result<Self, NetworkError> {
        Ok(Self {
            runtime: SingBoxRuntime::new(installation),
            resolver: GeoResolver::ipwhois(),
            probe,
            benchmarks: BenchmarkStore::open(paths, cache_ttl)?,
            nodes: NodeStore::open(paths)?,
        })
    }

    /// List active JP/SG/US nodes, optionally scoped to one subscription.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the node repository cannot be read.
    pub fn active_nodes(
        &self,
        subscription_id: Option<SubscriptionId>,
    ) -> Result<Vec<ManagedNode>, NetworkError> {
        let nodes = self
            .nodes
            .list()?
            .into_iter()
            .filter(|stored| stored.node.state == ManagedNodeState::Active)
            .filter(|stored| subscription_id.is_none_or(|id| stored.node.subscription_id == id))
            .map(|stored| stored.node)
            .collect::<Vec<_>>();
        Ok(nodes)
    }

    /// Select the best JP > SG > US node from fresh healthy cache only.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the cache or node store cannot be read.
    pub fn select_cached(
        &self,
        subscription_id: Option<SubscriptionId>,
        now: chrono::DateTime<Utc>,
    ) -> Result<Option<(NodeSelection, ManagedNode)>, NetworkError> {
        let nodes = self.active_nodes(subscription_id)?;
        let names = names_by_id(&nodes);
        let reports = self.fresh_reports(&nodes, now)?;
        let healthy: Vec<BenchmarkReport> = reports
            .into_values()
            .filter(|report| report.verdict.is_healthy())
            .collect();
        let Some(selection) = NodeSelector::select_best(&healthy, &names) else {
            return Ok(None);
        };
        let node = nodes
            .iter()
            .find(|node| node.id == selection.node_id)
            .cloned()
            .ok_or_else(|| NetworkError::Node("selected node is missing".into()))?;
        Ok(Some((selection, node)))
    }

    /// Snapshot the per-region counts and the current cached selection for the TUI.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the node store or cache cannot be read.
    pub fn snapshot(
        &self,
        subscription_id: Option<SubscriptionId>,
        now: chrono::DateTime<Utc>,
    ) -> Result<(RegionBenchmarkCounts, Option<NodeSelection>), NetworkError> {
        let nodes = self.active_nodes(subscription_id)?;
        let names = names_by_id(&nodes);
        let regions = self.region_counts(&nodes, now)?;
        let healthy: Vec<BenchmarkReport> = self
            .fresh_reports(&nodes, now)?
            .into_values()
            .filter(|report| report.verdict.is_healthy())
            .collect();
        let selected = NodeSelector::select_best(&healthy, &names);
        Ok((regions, selected))
    }

    /// Run the full benchmark: Quick Scan on every active node, then a Deep Scan on
    /// the best candidates per region, then persist and select.
    ///
    /// # Errors
    ///
    /// Returns a cancellation or storage error. Individual node failures are
    /// reflected in the summary, not returned as an overall error.
    pub async fn run(
        &self,
        subscription_id: Option<SubscriptionId>,
        cancellation: &CancellationToken,
    ) -> Result<BenchmarkRunSummary, NetworkError> {
        let nodes = self.active_nodes(subscription_id)?;
        if nodes.is_empty() {
            return Err(NetworkError::Benchmark(
                "no active JP/SG/US nodes to benchmark; sync a subscription first".into(),
            ));
        }

        let quick = self.quick_scan_all(&nodes, cancellation).await?;

        let mut deep_nodes = Vec::new();
        let mut candidates = nodes
            .iter()
            .filter_map(|node| {
                quick
                    .get(&node.id)
                    .filter(|(verdict, _)| *verdict == QuickVerdict::Candidate)
                    .map(|(_, latency)| (node.clone(), *latency))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(_, latency)| *latency);
        for region in CodexRegion::PREFERENCE {
            let limit = deep_limit(region);
            deep_nodes.extend(
                candidates
                    .iter()
                    .filter(|(node, _)| node.region_hint == region)
                    .take(limit)
                    .cloned(),
            );
        }

        let deep_ids = deep_nodes
            .iter()
            .map(|(node, _)| node.id)
            .collect::<std::collections::HashSet<_>>();
        for node in &nodes {
            if !deep_ids.contains(&node.id) {
                self.benchmarks.delete(node.id)?;
            }
        }

        let names = names_by_id(&nodes);
        for (node, _) in &deep_nodes {
            if cancellation.is_cancelled() {
                return Err(cancelled());
            }
            self.benchmarks.delete(node.id)?;
            let service = self.clone();
            let node = node.clone();
            let input = tokio::task::spawn_blocking(move || service.deep_scan_node(&node))
                .await
                .map_err(|source| NetworkError::Benchmark(format!("deep scan task: {source}")))?;
            if let Ok(input) = input {
                let report = build_report(input);
                self.benchmarks.save(&report)?;
            }
        }

        let quick_rejected = quick
            .values()
            .filter(|(verdict, _)| *verdict != QuickVerdict::Candidate)
            .count();
        let now = Utc::now();
        let regions = self.region_counts(&nodes, now)?;
        let healthy: Vec<BenchmarkReport> = self
            .fresh_reports(&nodes, now)?
            .into_values()
            .filter(|report| report.verdict.is_healthy())
            .collect();
        let healthy_ids = healthy.iter().map(|report| report.node_id).collect();
        let selected = NodeSelector::select_best(&healthy, &names);

        Ok(BenchmarkRunSummary {
            scanned: nodes.len(),
            quick_rejected,
            deep_scanned: deep_nodes.len(),
            healthy: regions.jp_healthy + regions.sg_healthy + regions.us_healthy,
            regions,
            selected,
            healthy_ids,
        })
    }

    /// Start a long-lived sidecar for one node and return the owned process handle
    /// plus the loopback endpoint to inject into the Desktop environment.
    ///
    /// # Errors
    ///
    /// Returns a config, spawn, or readiness error.
    pub fn start_sidecar(
        &self,
        node: &ManagedNode,
    ) -> Result<(SingBoxProcess, LoopbackProxyEndpoint), NetworkError> {
        let (process, endpoint) = self.prepare_and_launch(node)?;
        if !wait_ready(endpoint, SIDECAR_READY_TIMEOUT) {
            let _ = process.terminate();
            return Err(NetworkError::SingBox(
                "sidecar mixed endpoint did not become ready".into(),
            ));
        }
        Ok((process, endpoint))
    }

    /// Start one sidecar, verify its live exit and ChatGPT path, and retain that
    /// exact process for Desktop use.
    ///
    /// # Errors
    ///
    /// Returns an error when the node lacks a fresh healthy report, startup is
    /// cancelled, the live country differs from the hint, the HTTPS path fails,
    /// or the sidecar exits during verification. Node failures invalidate cache.
    pub fn start_verified_sidecar(
        &self,
        node: &ManagedNode,
        cancellation: &CancellationToken,
    ) -> Result<VerifiedSidecar, NetworkError> {
        self.require_fresh_healthy_report(node, Utc::now())?;
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }

        let (mut process, endpoint) = match self.prepare_and_launch(node) {
            Ok(sidecar) => sidecar,
            Err(error) => return self.reject_verified_launch(node.id, error),
        };
        if !wait_ready_cancellable(endpoint, SIDECAR_READY_TIMEOUT, cancellation) {
            if cancellation.is_cancelled() {
                return Err(cancelled());
            }
            return self.reject_verified_launch(
                node.id,
                NetworkError::SingBox("sidecar mixed endpoint did not become ready".into()),
            );
        }
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }

        let exit = match self.resolver.resolve_live(endpoint) {
            Ok(exit) if exit.country == node.region_hint => exit,
            Ok(_) => {
                return self.reject_verified_launch(
                    node.id,
                    NetworkError::Benchmark(
                        "launch-time exit country did not match the node region hint".into(),
                    ),
                );
            }
            Err(error) => return self.reject_verified_launch(node.id, error),
        };
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let sample = match self.probe.probe(endpoint) {
            Ok(sample) => sample,
            Err(error) => return self.reject_verified_launch(node.id, error),
        };
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        if process.try_wait()?.is_some() {
            return self.reject_verified_launch(
                node.id,
                NetworkError::SingBox("sidecar exited during launch-time verification".into()),
            );
        }

        Ok(VerifiedSidecar {
            process,
            endpoint,
            exit,
            sample,
        })
    }

    /// One quick recheck of a cached winner: config + launch + geo + a single probe.
    ///
    /// # Errors
    ///
    /// Returns the quick verdict; `QuickVerdict::Candidate` means the node is usable.
    pub fn quick_recheck(&self, node: &ManagedNode) -> QuickVerdict {
        self.quick_scan_node(node).0
    }

    /// Build the display selection for one node from a fresh healthy cached report.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the cache cannot be read.
    pub fn node_selection(
        &self,
        node: &ManagedNode,
        now: chrono::DateTime<Utc>,
    ) -> Result<NodeSelection, NetworkError> {
        let fingerprint = node_fingerprint(node.outbound.document());
        let report = self
            .benchmarks
            .get_fresh(node.id, &fingerprint, node.region_hint, now)?
            .filter(|report| report.verdict.is_healthy())
            .ok_or_else(|| {
                NetworkError::Benchmark(
                    "node has no fresh healthy benchmark report; run benchmark again".into(),
                )
            })?;
        Ok(NodeSelection::from_report(&report, &node.name))
    }

    /// Return every node of one subscription (active and stale) with its current
    /// fresh, fingerprint-bound report. Used for read-only Web display; never a
    /// selection gate.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the node store or cache cannot be read.
    pub fn node_status(
        &self,
        subscription_id: Option<SubscriptionId>,
        now: chrono::DateTime<Utc>,
    ) -> Result<Vec<NodeStatusView>, NetworkError> {
        let nodes = self
            .nodes
            .list()?
            .into_iter()
            .map(|stored| stored.node)
            .filter(|node| subscription_id.is_none_or(|id| node.subscription_id == id))
            .collect::<Vec<_>>();
        let reports = self.fresh_reports(&nodes, now)?;
        Ok(nodes
            .into_iter()
            .map(|node| NodeStatusView {
                report: reports.get(&node.id).cloned(),
                node,
            })
            .collect())
    }

    /// Return the selection for one node only when every manual-override gate passes:
    /// the node is Active, belongs to the active subscription, holds a fresh healthy
    /// report, and its verified exit region still matches its hint. Any other node
    /// yields `Ok(None)`; storage failures surface as errors.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the node store or benchmark cache cannot be read.
    pub fn healthy_selection_for(
        &self,
        node_id: NodeId,
        subscription_id: SubscriptionId,
        now: chrono::DateTime<Utc>,
    ) -> Result<Option<NodeSelection>, NetworkError> {
        let Ok(stored) = self.nodes.get(node_id) else {
            return Ok(None);
        };
        let node = &stored.node;
        if node.state != ManagedNodeState::Active || node.subscription_id != subscription_id {
            return Ok(None);
        }
        let fingerprint = node_fingerprint(node.outbound.document());
        let Some(report) =
            self.benchmarks
                .get_fresh(node.id, &fingerprint, node.region_hint, now)?
        else {
            return Ok(None);
        };
        if !report.verdict.is_healthy() || report.verified_region != node.region_hint {
            return Ok(None);
        }
        Ok(Some(NodeSelection::from_report(&report, &node.name)))
    }

    async fn quick_scan_all(
        &self,
        nodes: &[ManagedNode],
        cancellation: &CancellationToken,
    ) -> Result<HashMap<NodeId, (QuickVerdict, u64)>, NetworkError> {
        let semaphore = Arc::new(Semaphore::new(QUICK_SCAN_CONCURRENCY));
        let mut set = JoinSet::new();
        for node in nodes {
            if cancellation.is_cancelled() {
                set.abort_all();
                return Err(cancelled());
            }
            let service = self.clone();
            let node = node.clone();
            let permit = semaphore.clone().acquire_owned().await.map_err(|source| {
                NetworkError::Benchmark(format!("acquire concurrency slot: {source}"))
            })?;
            set.spawn_blocking(move || {
                let _permit = permit;
                (node.id, service.quick_scan_node(&node))
            });
        }
        let mut results = HashMap::with_capacity(nodes.len());
        while let Some(result) = set.join_next().await {
            if cancellation.is_cancelled() {
                set.abort_all();
                return Err(cancelled());
            }
            let (id, outcome) = result
                .map_err(|source| NetworkError::Benchmark(format!("quick scan task: {source}")))?;
            results.insert(id, outcome);
        }
        Ok(results)
    }

    fn quick_scan_node(&self, node: &ManagedNode) -> (QuickVerdict, u64) {
        let Ok((process, endpoint)) = self.prepare_and_launch(node) else {
            return (QuickVerdict::SidecarFailed, 0);
        };
        if !wait_ready(endpoint, SIDECAR_READY_TIMEOUT) {
            let _ = process.terminate();
            return (QuickVerdict::SidecarFailed, 0);
        }
        let Ok(observation) = self.resolver.resolve_live(endpoint) else {
            let _ = process.terminate();
            return (QuickVerdict::NetworkFailed, 0);
        };
        if observation.country != node.region_hint {
            let _ = process.terminate();
            return (QuickVerdict::CountryMismatch, 0);
        }
        let Ok(sample) = self.probe.probe(endpoint) else {
            let _ = process.terminate();
            return (QuickVerdict::NetworkFailed, 0);
        };
        let _ = process.terminate();
        (QuickVerdict::Candidate, millis(sample.header_latency))
    }

    fn deep_scan_node(&self, node: &ManagedNode) -> Result<DeepScanInput, NetworkError> {
        let (mut process, endpoint) = self.prepare_and_launch(node)?;
        if !wait_ready(endpoint, SIDECAR_READY_TIMEOUT) {
            let _ = process.terminate();
            return Err(NetworkError::SingBox("sidecar not ready".into()));
        }
        let first = self.resolver.resolve_live(endpoint)?;

        const ATTEMPTS: u8 = 5;
        let mut successes = 0_u8;
        let mut latencies_ms = Vec::with_capacity(ATTEMPTS as usize);
        for index in 0..ATTEMPTS {
            if index > 0 {
                std::thread::sleep(DEEP_PROBE_INTERVAL);
            }
            if let Ok(sample) = self.probe.probe(endpoint) {
                successes += 1;
                latencies_ms.push(millis(sample.header_latency));
            }
        }
        let second = self.resolver.resolve_live(endpoint)?;
        let sidecar_exited = process.try_wait()?.is_some();
        let _ = process.terminate();

        Ok(DeepScanInput {
            node_id: node.id,
            node_fingerprint: node_fingerprint(node.outbound.document()),
            expected_region: node.region_hint,
            first,
            second,
            attempts: ATTEMPTS,
            successes,
            latencies_ms,
            sidecar_exited,
            measured_at: Utc::now(),
        })
    }

    fn prepare_and_launch(
        &self,
        node: &ManagedNode,
    ) -> Result<(SingBoxProcess, LoopbackProxyEndpoint), NetworkError> {
        let reservation = LoopbackPortReservation::reserve()?;
        let endpoint = reservation.endpoint();
        let document = SingBoxConfigBuilder::guard(node, endpoint).build()?;
        reservation.release();

        let dir = sidecar_dir()?;
        let config = SingBoxConfigFile::write(&dir, &document)?;
        let process = self.runtime.launch(config)?;
        Ok((process, endpoint))
    }

    fn require_fresh_healthy_report(
        &self,
        node: &ManagedNode,
        now: chrono::DateTime<Utc>,
    ) -> Result<BenchmarkReport, NetworkError> {
        let fingerprint = node_fingerprint(node.outbound.document());
        self.benchmarks
            .get_fresh(node.id, &fingerprint, node.region_hint, now)?
            .filter(|report| report.verdict.is_healthy())
            .ok_or_else(|| {
                NetworkError::Benchmark(
                    "node has no fresh healthy benchmark report; run benchmark again".into(),
                )
            })
    }

    fn reject_verified_launch<T>(
        &self,
        node_id: NodeId,
        error: NetworkError,
    ) -> Result<T, NetworkError> {
        let _ = self.benchmarks.delete(node_id);
        Err(error)
    }

    fn fresh_reports(
        &self,
        nodes: &[ManagedNode],
        now: chrono::DateTime<Utc>,
    ) -> Result<HashMap<NodeId, BenchmarkReport>, NetworkError> {
        let mut reports = HashMap::new();
        for node in nodes {
            let fingerprint = node_fingerprint(node.outbound.document());
            if let Some(report) =
                self.benchmarks
                    .get_fresh(node.id, &fingerprint, node.region_hint, now)?
            {
                reports.insert(node.id, report);
            }
        }
        Ok(reports)
    }

    fn region_counts(
        &self,
        nodes: &[ManagedNode],
        now: chrono::DateTime<Utc>,
    ) -> Result<RegionBenchmarkCounts, NetworkError> {
        let mut counts = RegionBenchmarkCounts::default();
        let healthy = self
            .fresh_reports(nodes, now)?
            .into_values()
            .filter(|report| report.verdict.is_healthy())
            .map(|report| report.node_id)
            .collect::<std::collections::HashSet<_>>();
        for node in nodes {
            match node.region_hint {
                CodexRegion::JP => {
                    counts.jp_active += 1;
                    if healthy.contains(&node.id) {
                        counts.jp_healthy += 1;
                    }
                }
                CodexRegion::SG => {
                    counts.sg_active += 1;
                    if healthy.contains(&node.id) {
                        counts.sg_healthy += 1;
                    }
                }
                CodexRegion::US => {
                    counts.us_active += 1;
                    if healthy.contains(&node.id) {
                        counts.us_healthy += 1;
                    }
                }
            }
        }
        Ok(counts)
    }
}

fn names_by_id(nodes: &[ManagedNode]) -> HashMap<NodeId, String> {
    nodes
        .iter()
        .map(|node| (node.id, node.name.clone()))
        .collect()
}

fn wait_ready(endpoint: LoopbackProxyEndpoint, timeout: Duration) -> bool {
    let started = Instant::now();
    loop {
        if TcpStream::connect(endpoint.socket_addr()).is_ok() {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_ready_cancellable(
    endpoint: LoopbackProxyEndpoint,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> bool {
    let started = Instant::now();
    loop {
        if cancellation.is_cancelled() {
            return false;
        }
        if TcpStream::connect(endpoint.socket_addr()).is_ok() {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn sidecar_dir() -> Result<PathBuf, NetworkError> {
    let dir = std::env::temp_dir().join(format!(
        "cpg-sing-box-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir)
        .map_err(|source| NetworkError::Storage(format!("create sidecar dir: {source}")))?;
    Ok(dir)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn cancelled() -> NetworkError {
    NetworkError::Cancelled("benchmark cancelled".into())
}

#[cfg(test)]
mod tests {
    use std::{net::Ipv4Addr, sync::Arc};

    use proxy_guard_core::{
        BENCHMARK_SCHEMA_VERSION, BenchmarkReport, BenchmarkVerdict, CodexRegion, ManagedNode,
        ManagedNodeState, NodeId, SingBoxOutbound, SubscriptionId,
    };
    use serde_json::json;
    use tempfile::tempdir;

    use super::NodeBenchmarkService;
    use super::node_fingerprint;
    use crate::{ManagedPaths, ReqwestCodexPathProbe, SingBoxInstallation};

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn service() -> (tempfile::TempDir, NodeBenchmarkService) {
        let temporary = tempdir().expect("temporary directory");
        let paths = ManagedPaths::from_root(temporary.path().join("data"));
        let service = NodeBenchmarkService::new(
            &paths,
            SingBoxInstallation::for_tests(),
            std::time::Duration::from_secs(3600),
            Arc::new(ReqwestCodexPathProbe),
        )
        .expect("service");
        (temporary, service)
    }

    fn node(name: &str, subscription_id: SubscriptionId) -> ManagedNode {
        ManagedNode::new(
            name,
            subscription_id,
            CodexRegion::JP,
            SingBoxOutbound::new(json!({
                "type": "socks",
                "server": "proxy.example",
                "server_port": 1080
            }))
            .expect("outbound"),
            KEY,
        )
        .expect("node")
    }

    fn healthy_report(node: &ManagedNode) -> BenchmarkReport {
        BenchmarkReport {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            node_id: node.id,
            node_fingerprint: node_fingerprint(node.outbound.document()),
            expected_region: node.region_hint,
            verified_region: node.region_hint,
            first_exit_ip: Ipv4Addr::new(203, 0, 113, 1).into(),
            second_exit_ip: Ipv4Addr::new(203, 0, 113, 1).into(),
            exit_ip_stable: true,
            attempts: 5,
            successes: 5,
            median_header_ms: 80,
            p95_header_ms: 120,
            jitter_ms: 10,
            score: 92,
            verdict: BenchmarkVerdict::Healthy,
            measured_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn healthy_selection_accepts_only_active_fresh_current_nodes() {
        let (_temporary, service) = service();
        let subscription = SubscriptionId::new();
        let node = node("JP Tokyo", subscription);
        service.nodes.create(node.clone()).expect("create node");
        service
            .benchmarks
            .save(&healthy_report(&node))
            .expect("save");

        let selection = service
            .healthy_selection_for(node.id, subscription, chrono::Utc::now())
            .expect("read selection");
        assert!(selection.is_some());
        assert_eq!(selection.unwrap().node_id, node.id);
    }

    #[test]
    fn healthy_selection_rejects_stale_nodes() {
        let (_temporary, service) = service();
        let subscription = SubscriptionId::new();
        let mut node = node("JP Tokyo", subscription);
        node.state = ManagedNodeState::Stale;
        service.nodes.create(node.clone()).expect("create node");
        service
            .benchmarks
            .save(&healthy_report(&node))
            .expect("save");

        assert!(
            service
                .healthy_selection_for(node.id, subscription, chrono::Utc::now())
                .expect("read selection")
                .is_none()
        );
    }

    #[test]
    fn healthy_selection_rejects_other_subscriptions_and_missing_nodes() {
        let (_temporary, service) = service();
        let subscription = SubscriptionId::new();
        let node = node("JP Tokyo", subscription);
        service.nodes.create(node.clone()).expect("create node");
        service
            .benchmarks
            .save(&healthy_report(&node))
            .expect("save");

        let other = SubscriptionId::new();
        assert!(
            service
                .healthy_selection_for(node.id, other, chrono::Utc::now())
                .expect("read selection")
                .is_none()
        );
        assert!(
            service
                .healthy_selection_for(NodeId::new(), subscription, chrono::Utc::now())
                .expect("read selection")
                .is_none()
        );
    }

    #[test]
    fn healthy_selection_requires_fresh_healthy_region_matching_report() {
        let (_temporary, service) = service();
        let subscription = SubscriptionId::new();
        let node = node("JP Tokyo", subscription);
        service.nodes.create(node.clone()).expect("create node");

        assert!(
            service
                .healthy_selection_for(node.id, subscription, chrono::Utc::now())
                .expect("no cache")
                .is_none(),
            "no fresh report must be rejected"
        );

        let mut rejected = healthy_report(&node);
        rejected.verdict = BenchmarkVerdict::Rejected {
            reason: proxy_guard_core::BenchmarkRejection::LowSuccessRate,
        };
        service.benchmarks.save(&rejected).expect("save rejected");
        assert!(
            service
                .healthy_selection_for(node.id, subscription, chrono::Utc::now())
                .expect("rejected")
                .is_none(),
            "rejected report must be rejected"
        );

        let mut mismatch = healthy_report(&node);
        mismatch.verified_region = CodexRegion::SG;
        service.benchmarks.save(&mismatch).expect("save mismatch");
        assert!(
            service
                .healthy_selection_for(node.id, subscription, chrono::Utc::now())
                .expect("region mismatch")
                .is_none(),
            "verified region mismatch must be rejected"
        );

        let mut stale = healthy_report(&node);
        stale.measured_at = chrono::Utc::now() - chrono::Duration::hours(2);
        service.benchmarks.save(&stale).expect("save stale");
        assert!(
            service
                .healthy_selection_for(node.id, subscription, chrono::Utc::now())
                .expect("stale cache")
                .is_none(),
            "stale cache must be rejected"
        );
    }
}
