use std::{fmt, net::IpAddr, str::FromStr, time::Duration};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{DesktopLaunchInfo, GuardError};

pub const MANAGED_NODE_SCHEMA_VERSION: u32 = 1;
pub const BENCHMARK_SCHEMA_VERSION: u32 = 1;
pub const SUBSCRIPTION_SCHEMA_VERSION: u32 = 1;

/// Fixed allowed exit regions for Managed Mode. Selection is lexicographically
/// JP > SG > US, never a global score maximum.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CodexRegion {
    JP,
    SG,
    US,
}

impl CodexRegion {
    pub const PREFERENCE: [Self; 3] = [Self::JP, Self::SG, Self::US];

    #[must_use]
    pub const fn priority(self) -> u8 {
        match self {
            Self::JP => 0,
            Self::SG => 1,
            Self::US => 2,
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::JP => "JP",
            Self::SG => "SG",
            Self::US => "US",
        }
    }

    #[must_use]
    pub fn from_country_code(code: &str) -> Option<Self> {
        match code {
            "JP" => Some(Self::JP),
            "SG" => Some(Self::SG),
            "US" => Some(Self::US),
            _ => None,
        }
    }
}

impl fmt::Display for CodexRegion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.display_name())
    }
}

/// Stable identifier for one managed node.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(Uuid);

impl NodeId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for NodeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for NodeId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// Stable identifier for one subscription. Its credential is stored separately.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubscriptionId(Uuid);

impl SubscriptionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SubscriptionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SubscriptionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SubscriptionId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// One opaque sing-box outbound validated for use as a remote node.
///
/// The `tag` and lifecycle-owned outbound types are rejected so only a single
/// remote proxy node can ever be wrapped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SingBoxOutbound(Value);

impl SingBoxOutbound {
    /// Wrap one outbound object while rejecting lifecycle-owned or bypass types.
    ///
    /// # Errors
    ///
    /// Returns [`GuardError::Managed`] when the document is not an object, lacks
    /// a type, supplies its own tag, or is not a remote node.
    pub fn new(document: Value) -> Result<Self, GuardError> {
        let outbound = Self(document);
        outbound.validate()?;
        Ok(outbound)
    }

    #[must_use]
    pub const fn document(&self) -> &Value {
        &self.0
    }

    /// Validate the structural boundary that can be checked without duplicating
    /// the pinned sing-box release's complete outbound schema.
    ///
    /// # Errors
    ///
    /// Returns a typed configuration error for unsafe or incomplete documents.
    pub fn validate(&self) -> Result<(), GuardError> {
        let object = self
            .0
            .as_object()
            .ok_or_else(|| invalid_outbound("the outbound root must be a JSON object"))?;
        let outbound_type = object
            .get("type")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| invalid_outbound("the outbound must contain a non-empty string type"))?;
        if object.contains_key("tag") {
            return Err(invalid_outbound(
                "the outbound tag is owned by the config builder",
            ));
        }
        if matches!(
            outbound_type.to_ascii_lowercase().as_str(),
            "direct" | "block" | "dns" | "selector" | "urltest"
        ) {
            return Err(invalid_outbound(
                "the outbound type must represent one remote proxy node",
            ));
        }
        Ok(())
    }
}

fn invalid_outbound(reason: impl Into<String>) -> GuardError {
    GuardError::Managed(format!("invalid sing-box outbound: {}", reason.into()))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedNodeState {
    #[default]
    Active,
    Stale,
}

/// Persistent description of one JP/SG/US node imported from a subscription.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManagedNode {
    pub schema_version: u32,
    pub id: NodeId,
    pub subscription_id: SubscriptionId,
    pub name: String,
    pub region_hint: CodexRegion,
    pub outbound: SingBoxOutbound,
    pub remote_key: String,
    pub state: ManagedNodeState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ManagedNode {
    /// Create a managed node with a generated ID and normalized display name.
    ///
    /// # Errors
    ///
    /// Returns a typed validation error when the name or outbound is invalid.
    pub fn new(
        name: impl Into<String>,
        subscription_id: SubscriptionId,
        region_hint: CodexRegion,
        outbound: SingBoxOutbound,
        remote_key: impl Into<String>,
    ) -> Result<Self, GuardError> {
        let now = Utc::now();
        let node = Self {
            schema_version: MANAGED_NODE_SCHEMA_VERSION,
            id: NodeId::new(),
            subscription_id,
            name: normalize_name(&name.into())?,
            region_hint,
            outbound,
            remote_key: remote_key.into(),
            state: ManagedNodeState::Active,
            created_at: now,
            updated_at: now,
        };
        node.validate()?;
        Ok(node)
    }

    /// Validate persisted node invariants before use or config generation.
    ///
    /// # Errors
    ///
    /// Returns a typed error for unsupported schemas, malformed metadata, or an
    /// unsafe outbound.
    pub fn validate(&self) -> Result<(), GuardError> {
        if self.schema_version != MANAGED_NODE_SCHEMA_VERSION {
            return Err(GuardError::Managed(format!(
                "unsupported node schema version {}",
                self.schema_version
            )));
        }
        normalize_name(&self.name)?;
        if self.remote_key.len() != 64
            || !self.remote_key.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(GuardError::Managed(
                "node remote_key must be a 64-character hex digest".into(),
            ));
        }
        self.outbound.validate()
    }
}

fn normalize_name(name: &str) -> Result<String, GuardError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(GuardError::Managed(
            "node name must contain 1 to 100 printable characters".into(),
        ));
    }
    Ok(name.to_owned())
}

/// A single observed exit identity, obtained through the managed mixed proxy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExitObservation {
    pub ip: IpAddr,
    pub country: CodexRegion,
    pub observed_at: DateTime<Utc>,
}

/// One lightweight HTTPS-path observation against `chatgpt.com`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathSample {
    pub header_latency: Duration,
    pub http_status: u16,
}

/// Outcome of a single-node quick scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickVerdict {
    Candidate,
    ConfigRejected,
    SidecarFailed,
    CountryMismatch,
    NetworkFailed,
}

/// Why a benchmarked node failed one of the hard health gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkRejection {
    CountryMismatch,
    UnstableCountry,
    LowSuccessRate,
    HighP95,
    SidecarExited,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BenchmarkVerdict {
    Healthy,
    Rejected { reason: BenchmarkRejection },
}

impl BenchmarkVerdict {
    #[must_use]
    pub const fn is_healthy(self) -> bool {
        matches!(self, Self::Healthy)
    }
}

/// A cached, fingerprint-bound health report for one node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub node_id: NodeId,
    pub node_fingerprint: String,

    pub expected_region: CodexRegion,
    pub verified_region: CodexRegion,

    pub first_exit_ip: IpAddr,
    pub second_exit_ip: IpAddr,
    pub exit_ip_stable: bool,

    pub attempts: u8,
    pub successes: u8,

    pub median_header_ms: u64,
    pub p95_header_ms: u64,
    pub jitter_ms: u64,

    pub score: u16,
    pub verdict: BenchmarkVerdict,
    pub measured_at: DateTime<Utc>,
}

impl BenchmarkReport {
    pub fn validate(&self) -> Result<(), GuardError> {
        if self.schema_version != BENCHMARK_SCHEMA_VERSION {
            return Err(GuardError::Managed(format!(
                "unsupported benchmark schema version {}",
                self.schema_version
            )));
        }
        if self.node_fingerprint.len() != 64
            || !self
                .node_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(GuardError::Managed(
                "benchmark node_fingerprint must be a 64-character hex digest".into(),
            ));
        }
        if self.attempts == 0 || self.successes > self.attempts {
            return Err(GuardError::Managed(
                "benchmark attempts/successes are inconsistent".into(),
            ));
        }
        if self.score > 100 {
            return Err(GuardError::Managed(
                "benchmark score must be between 0 and 100".into(),
            ));
        }
        Ok(())
    }
}

/// The JP > SG > US winner chosen by [`crate::NodeSelection`]-style selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSelection {
    pub node_id: NodeId,
    pub name: String,
    pub region: CodexRegion,
    pub score: u16,
    pub success_percent: u8,
    pub median_ms: u64,
    pub p95_ms: u64,
    pub exit_stable: bool,
}

impl NodeSelection {
    #[must_use]
    pub fn from_report(report: &BenchmarkReport, name: &str) -> Self {
        let success_percent =
            u8::try_from((u32::from(report.successes) * 100 / u32::from(report.attempts)).min(100))
                .unwrap_or(100);
        Self {
            node_id: report.node_id,
            name: name.to_owned(),
            region: report.verified_region,
            score: report.score,
            success_percent,
            median_ms: report.median_header_ms,
            p95_ms: report.p95_header_ms,
            exit_stable: report.exit_ip_stable,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionSyncStatus {
    #[default]
    Never,
    Succeeded,
    Failed,
}

/// Persistent subscription metadata. The raw URL is intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionSource {
    pub schema_version: u32,
    pub id: SubscriptionId,
    pub name: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_sync_status: SubscriptionSyncStatus,
    pub created_at: DateTime<Utc>,
}

impl SubscriptionSource {
    /// Create metadata without retaining the associated URL.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the display name is invalid.
    pub fn new(name: impl Into<String>) -> Result<Self, GuardError> {
        Ok(Self {
            schema_version: SUBSCRIPTION_SCHEMA_VERSION,
            id: SubscriptionId::new(),
            name: normalize_subscription_name(&name.into())?,
            enabled: true,
            last_sync_at: None,
            last_sync_status: SubscriptionSyncStatus::Never,
            created_at: Utc::now(),
        })
    }

    /// Validate persisted metadata before runtime use.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for an unsupported schema or invalid name.
    pub fn validate(&self) -> Result<(), GuardError> {
        if self.schema_version != SUBSCRIPTION_SCHEMA_VERSION {
            return Err(GuardError::Managed(
                "unsupported subscription schema".into(),
            ));
        }
        normalize_subscription_name(&self.name).map(|_| ())
    }

    /// Normalize and validate a display name without creating metadata.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the name is empty, too long, or
    /// contains control characters.
    pub fn normalize_name(name: &str) -> Result<String, GuardError> {
        normalize_subscription_name(name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionProtocol {
    Vless,
    Trojan,
    Shadowsocks,
    Socks,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionNodeState {
    Active,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionNodeBinding {
    pub subscription_id: SubscriptionId,
    /// Opaque digest of the remote share link; never the link or its credentials.
    pub remote_key: String,
    pub node_id: NodeId,
    pub state: SubscriptionNodeState,
}

/// Per-protocol node counts for a parsed or synchronized subscription.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionProtocolCounts {
    pub vless: usize,
    pub trojan: usize,
    pub shadowsocks: usize,
    pub socks: usize,
}

impl SubscriptionProtocolCounts {
    #[must_use]
    pub const fn total(&self) -> usize {
        self.vless + self.trojan + self.shadowsocks + self.socks
    }
}

/// A protocol-aware preview of one subscription URL before anything is persisted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionPreview {
    pub fetched: usize,
    pub supported: usize,
    pub unsupported: usize,
    pub ignored_region: usize,
    pub failed: usize,
    pub protocols: SubscriptionProtocolCounts,
}

/// Result of one subscription synchronization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionSyncSummary {
    pub fetched: usize,
    pub imported: usize,
    pub updated: usize,
    pub stale: usize,
    pub unsupported: usize,
    pub ignored_region: usize,
    pub failed: usize,
    pub protocols: SubscriptionProtocolCounts,
}

fn normalize_subscription_name(name: &str) -> Result<String, GuardError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(GuardError::Managed(
            "subscription name must contain 1 to 128 printable characters".into(),
        ));
    }
    Ok(name.to_owned())
}

/// Per-region counts observed after a full benchmark run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RegionBenchmarkCounts {
    pub jp_active: usize,
    pub jp_healthy: usize,
    pub sg_active: usize,
    pub sg_healthy: usize,
    pub us_active: usize,
    pub us_healthy: usize,
}

impl RegionBenchmarkCounts {
    #[must_use]
    pub const fn active(self, region: CodexRegion) -> usize {
        match region {
            CodexRegion::JP => self.jp_active,
            CodexRegion::SG => self.sg_active,
            CodexRegion::US => self.us_active,
        }
    }

    #[must_use]
    pub const fn healthy(self, region: CodexRegion) -> usize {
        match region {
            CodexRegion::JP => self.jp_healthy,
            CodexRegion::SG => self.sg_healthy,
            CodexRegion::US => self.us_healthy,
        }
    }
}

/// Result of one benchmark run, including the JP > SG > US winner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkRunSummary {
    pub scanned: usize,
    pub quick_rejected: usize,
    pub deep_scanned: usize,
    pub healthy: usize,
    pub regions: RegionBenchmarkCounts,
    pub selected: Option<NodeSelection>,
}

/// Receipt for a Managed Mode launch through the Guard-owned sing-box sidecar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManagedLaunchReceipt {
    pub pid: u32,
    pub proxy_endpoint: String,
    pub node: NodeSelection,
    pub desktop: DesktopLaunchInfo,
}
