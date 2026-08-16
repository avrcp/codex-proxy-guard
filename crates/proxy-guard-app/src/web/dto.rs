use chrono::{DateTime, Utc};
use proxy_guard_core::{
    BenchmarkRunSummary, BenchmarkVerdict, CodexRegion, ManagedNodeState, NodeSelection,
    RegionBenchmarkCounts, SubscriptionNodeState, SubscriptionSyncStatus,
};
use serde::Serialize;

use proxy_guard_network::NodeStatusView;

#[derive(Debug, Serialize)]
pub struct StateDto {
    pub mode: &'static str,
    pub subscription_name: Option<String>,
    pub regions: RegionBenchmarkCounts,
    pub auto_selected: Option<NodeSelection>,
    pub manual_selected: Option<NodeSelection>,
    /// Manual override when present, otherwise the AUTO JP > SG > US winner.
    pub selection: Option<NodeSelection>,
    /// True when the selection that will be used for next launch is a manual override.
    pub manual_active: bool,
}

#[derive(Debug, Serialize)]
pub struct OperationDto {
    pub state: &'static str,
    pub subscription_id: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub message: Option<String>,
    pub last_benchmark: Option<BenchmarkRunSummary>,
}

#[derive(Debug, Serialize)]
pub struct SubscriptionDto {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_sync_status: &'static str,
    pub active: bool,
    pub active_nodes: usize,
    pub stale_nodes: usize,
}

#[derive(Debug, Serialize)]
pub struct SubscriptionPreviewDto {
    pub fetched: usize,
    pub supported: usize,
    pub unsupported: usize,
    pub ignored_region: usize,
    pub failed: usize,
    pub protocols: ProtocolCountsDto,
}

#[derive(Debug, Serialize)]
pub struct ProtocolCountsDto {
    pub vless: usize,
    pub trojan: usize,
    pub shadowsocks: usize,
    pub socks: usize,
}

#[derive(Debug, Serialize)]
pub struct NodeDto {
    pub id: String,
    pub name: String,
    pub region: CodexRegion,
    pub state: &'static str,
    pub subscription_id: String,
    pub status: &'static str,
    pub score: Option<u16>,
    pub success_percent: Option<u8>,
    pub median_ms: Option<u64>,
    pub p95_ms: Option<u64>,
    pub exit_stable: Option<bool>,
    pub verified_region: Option<CodexRegion>,
    pub measured_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct SelectionResultDto {
    pub selection: Option<NodeSelection>,
}

#[derive(Debug, Serialize)]
pub struct ActivateResultDto {
    pub subscription_id: String,
    pub subscription_name: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorDto {
    pub code: String,
    pub message: String,
}

pub fn subscription_dto(
    stored: &proxy_guard_network::StoredSubscription,
    active_id: Option<proxy_guard_core::SubscriptionId>,
) -> SubscriptionDto {
    let active_nodes = stored
        .bindings
        .iter()
        .filter(|binding| binding.state == SubscriptionNodeState::Active)
        .count();
    let stale_nodes = stored
        .bindings
        .iter()
        .filter(|binding| binding.state == SubscriptionNodeState::Stale)
        .count();
    SubscriptionDto {
        id: stored.source.id.to_string(),
        name: stored.source.name.clone(),
        enabled: stored.source.enabled,
        last_sync_at: stored.source.last_sync_at,
        last_sync_status: sync_status_key(stored.source.last_sync_status),
        active: active_id == Some(stored.source.id),
        active_nodes,
        stale_nodes,
    }
}

fn sync_status_key(status: SubscriptionSyncStatus) -> &'static str {
    match status {
        SubscriptionSyncStatus::Never => "never",
        SubscriptionSyncStatus::Succeeded => "succeeded",
        SubscriptionSyncStatus::Failed => "failed",
    }
}

pub fn preview_dto(preview: &proxy_guard_core::SubscriptionPreview) -> SubscriptionPreviewDto {
    SubscriptionPreviewDto {
        fetched: preview.fetched,
        supported: preview.supported,
        unsupported: preview.unsupported,
        ignored_region: preview.ignored_region,
        failed: preview.failed,
        protocols: ProtocolCountsDto {
            vless: preview.protocols.vless,
            trojan: preview.protocols.trojan,
            shadowsocks: preview.protocols.shadowsocks,
            socks: preview.protocols.socks,
        },
    }
}

pub fn node_dto(view: &NodeStatusView) -> NodeDto {
    let report = view.report.as_ref();
    let status = match &view.node.state {
        ManagedNodeState::Stale => "stale",
        ManagedNodeState::Active => match report {
            None => "not-tested",
            Some(report) => match report.verdict {
                BenchmarkVerdict::Healthy => "healthy",
                BenchmarkVerdict::Rejected { .. } => "rejected",
            },
        },
    };
    NodeDto {
        id: view.node.id.to_string(),
        name: view.node.name.clone(),
        region: view.node.region_hint,
        state: match view.node.state {
            ManagedNodeState::Active => "active",
            ManagedNodeState::Stale => "stale",
        },
        subscription_id: view.node.subscription_id.to_string(),
        status,
        score: report.map(|report| report.score),
        success_percent: report.map(|report| {
            u8::try_from((u32::from(report.successes) * 100 / u32::from(report.attempts)).min(100))
                .unwrap_or(100)
        }),
        median_ms: report.map(|report| report.median_header_ms),
        p95_ms: report.map(|report| report.p95_header_ms),
        exit_stable: report.map(|report| report.exit_ip_stable),
        verified_region: report.map(|report| report.verified_region),
        measured_at: report.map(|report| report.measured_at),
    }
}
