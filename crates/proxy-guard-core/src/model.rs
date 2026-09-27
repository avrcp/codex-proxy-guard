use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::GuardConfig;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DesktopProduct {
    ChatGpt,
    ChatGptClassic,
    ExecutableOverride,
}

impl DesktopProduct {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ChatGpt => "ChatGPT Desktop",
            Self::ChatGptClassic => "ChatGPT Classic",
            Self::ExecutableOverride => "Configured Desktop executable",
        }
    }

    pub const fn selection_reason(self) -> &'static str {
        match self {
            Self::ChatGpt => "current ChatGPT desktop app",
            Self::ChatGptClassic => "ChatGPT Classic fallback",
            Self::ExecutableOverride => "configured executable override",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DesktopDiscoverySource {
    AppxManifest,
    ExecutableOverride,
}

/// Whether a launch target is a registered Windows package application or an
/// ordinary executable. A path beneath WindowsApps alone is not sufficient to
/// recreate the application's package identity; registered targets retain the
/// exact application metadata Windows registered for that package.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum DesktopTargetKind {
    RegisteredPackage(PackageApplication),
    UnpackagedExecutable,
}

/// Runtime classification obtained from the registered manifest. Unknown is
/// intentional: omitted manifest evidence must not be treated as FullTrust.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageRuntimeKind {
    FullTrustDesktop,
    AppContainer,
    Unknown,
}

/// Stable identity of the exact registered package application selected for a
/// Desktop launch. `manifest_executable` preserves the original relative text
/// rather than deriving identity from a canonicalized filesystem path.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PackageApplication {
    pub package_full_name: String,
    pub package_family_name: String,
    pub application_id: String,
    pub app_user_model_id: String,
    pub manifest_executable: String,
    pub runtime_kind: PackageRuntimeKind,
}

impl DesktopDiscoverySource {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::AppxManifest => "APPX manifest",
            Self::ExecutableOverride => "configuration override",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DesktopAppInfo {
    pub product: DesktopProduct,
    pub package_name: String,
    pub package_version: String,
    pub architecture: String,
    pub discovery_source: DesktopDiscoverySource,
    pub target_kind: DesktopTargetKind,
    pub install_location: PathBuf,
    pub executable: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DesktopLaunchInfo {
    pub product: DesktopProduct,
    pub package_name: String,
    pub package_version: String,
    pub architecture: String,
    pub discovery_source: DesktopDiscoverySource,
}

impl From<&DesktopAppInfo> for DesktopLaunchInfo {
    fn from(info: &DesktopAppInfo) -> Self {
        Self {
            product: info.product,
            package_name: info.package_name.clone(),
            package_version: info.package_version.clone(),
            architecture: info.architecture.clone(),
            discovery_source: info.discovery_source,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DesktopAppDiscovery {
    #[default]
    Unknown,
    Searching,
    Found(Box<DesktopAppInfo>),
    NotFound(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DesktopProcessState {
    #[default]
    Unknown,
    Stopped,
    Running {
        pid: u32,
    },
}

/// Outcome of the daemon compatibility step for one launch. Normal launches
/// never touch the shared daemon (`Skipped`); the stop command runs only in the
/// explicitly authorized repair launch, and even then Guard only invokes the
/// public `codex app-server daemon stop` lifecycle command.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonPreparation {
    /// No daemon was running when the explicit stop was requested.
    NotNeeded,
    /// The shared daemon was stopped through the official lifecycle command.
    Stopped,
    /// Normal launch: the daemon was not inspected or touched at all.
    Skipped,
}

impl DaemonPreparation {
    pub fn status_detail(self) -> &'static str {
        match self {
            Self::NotNeeded => "the shared Codex background server was not running",
            Self::Stopped => "the shared Codex background server was stopped before launch",
            Self::Skipped => "",
        }
    }
}

/// Single-use launch intent; never persisted to configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    pub refresh_codex_daemon: bool,
    /// Diagnostic activation without any proxy arguments or backend
    /// configuration. The receipt must report `proxy_delivery = not_established`;
    /// this is never a silent downgrade of a normal proxy launch.
    pub activation_only: bool,
}

/// Which launch backend produced the Desktop process. Registered package
/// applications are activated through the Windows application model
/// (`IApplicationActivationManager::ActivateApplication`); ordinary
/// executables are created as a new process with an injected environment.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMethod {
    NativeProcess,
    AppmodelActivation,
}

impl LaunchMethod {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::NativeProcess => "process environment injection",
            Self::AppmodelActivation => "Windows registered application activation",
        }
    }
}

/// How far the activation request travelled. A successful receipt always
/// carries `Returned`; `OutcomeUnknown` is reported as a launch error because
/// the application may already have been created.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivationState {
    NotSubmitted,
    Returned,
    OutcomeUnknown,
}

/// `ActivateApplication` returns a PID, which may belong to an existing
/// instance; a returned PID is never on its own evidence of a fresh process.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstanceObservation {
    Created,
    Reused,
    Unknown,
}

/// Package identity of the actually observed target process, distinct from a
/// query failure: `Missing` is an OS answer, not an error.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageIdentityObservation {
    NotApplicable,
    Matched,
    Missing,
    Mismatch,
    QueryFailed,
}

/// Application identity (AUMID) of the observed target. Package identity
/// matching alone does not prove the right application entry was activated.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AumidObservation {
    NotApplicable,
    Matched,
    Missing,
    Mismatch,
    QueryFailed,
}

/// How the configured proxy was actually delivered for this launch. The layers
/// are separate facts: Chromium arguments reach the Electron shell, the
/// authorized home configuration reaches Codex backend processes started from
/// that home, and neither proves the other.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProxyDelivery {
    /// Unpackaged launch: HTTP(S)_PROXY/NO_PROXY in the process environment.
    ProcessEnvironment,
    /// Registered launch: validated Chromium proxy arguments submitted through
    /// the activation arguments.
    ActivationArguments,
    /// Registered launch: activation arguments plus an authorized, prepared
    /// proxy block in the authorized Codex Home `.env`.
    ActivationArgumentsAndHomeConfig,
    /// No proxy was delivered (activation-only diagnostics).
    NotEstablished,
}

/// State of the Codex Home backend proxy configuration for this launch.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendProxyConfig {
    /// Unpackaged launch or activation-only diagnostics: no home configuration
    /// is involved.
    NotApplicable,
    /// The user has not authorized Guard to manage the `.env` proxy block.
    NotAuthorized,
    /// The authorized block was prepared in the confirmed home before the
    /// activation. Preparation is a file fact, not a network verification.
    Prepared,
}

impl BackendProxyConfig {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::NotApplicable => "not applicable",
            Self::NotAuthorized => "not authorized",
            Self::Prepared => "prepared",
        }
    }
}

/// What the authorized Codex Home `.env` proxy block actually looks like on
/// disk right now, versus the current proxy configuration. Pure UI/domain
/// state: no paths, file handles, or platform types live here. The state is
/// refreshed from real disk inspection (`Refresh`) and from deterministic
/// transitions (a proxy edit makes the block stale; a prepared launch makes
/// it current); it is never a claim about observed network traffic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BackendProxyRuntimeState {
    /// Not inspected yet.
    #[default]
    Unknown,
    /// The launch target is not a registered application; no home
    /// configuration is involved.
    NotApplicable,
    /// The user has not authorized Guard to manage the `.env` proxy block.
    NotAuthorized,
    /// Authorized, but Guard's block is not on disk yet (consent recorded;
    /// it is written by the next launch).
    Pending,
    /// Guard's block exists and matches the current proxy configuration.
    Current,
    /// Guard's block exists but its values differ from the current proxy
    /// configuration; the next launch syncs it.
    Stale,
    /// The `.env` holds proxy keys outside Guard's block; Guard refuses to
    /// override them (named by key).
    Conflict { keys: Vec<String> },
    /// Guard's block is structurally damaged or of an unknown version;
    /// automatic edits are refused.
    Invalid,
    /// The state could not be determined (unreadable file, encoding, or an
    /// unusable authorized home).
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LaunchReceipt {
    pub pid: u32,
    /// The proxy endpoint this launch planned to use. `None` for the
    /// activation-only diagnostic path, which must not imply delivered proxying.
    pub proxy_endpoint: Option<String>,
    pub daemon_preparation: DaemonPreparation,
    pub launch_method: LaunchMethod,
    pub activation_state: ActivationState,
    pub instance: InstanceObservation,
    pub package_identity: PackageIdentityObservation,
    pub aumid: AumidObservation,
    pub proxy_delivery: ProxyDelivery,
    pub backend_proxy_config: BackendProxyConfig,
    /// Token elevation of the observed target process (`None` when the query
    /// failed). Recorded for diagnostics only; it never gates the launch.
    pub target_elevation: Option<bool>,
    pub desktop: DesktopLaunchInfo,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LaunchState {
    #[default]
    Idle,
    Launching,
    Running(LaunchReceipt),
    Blocked(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForegroundOperation {
    Refresh,
    Launch,
    SaveConfig,
    UpdateBackendProxyConsent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProxyField {
    #[default]
    Host,
    Port,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyEditor {
    pub host: String,
    pub port: String,
    pub active_field: ProxyField,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub config: GuardConfig,
    pub config_path: PathBuf,
    pub config_readiness: ConfigReadiness,
    pub desktop_app: DesktopAppDiscovery,
    pub desktop_process: DesktopProcessState,
    /// Real disk state of the authorized Codex Home proxy block (see
    /// [`BackendProxyRuntimeState`]); refreshed with `R` and updated by the
    /// deterministic consent/save/launch transitions.
    pub backend_proxy_state: BackendProxyRuntimeState,
    pub launch: LaunchState,
    pub foreground: Option<ForegroundOperation>,
    pub daemon_repair_prompt: bool,
    pub backend_proxy_prompt: bool,
    pub status_message: String,
    pub error_message: Option<String>,
    pub show_help: bool,
    pub proxy_editor: Option<ProxyEditor>,
    pub should_quit: bool,
}

/// Runtime-only configuration gate. An invalid configuration keeps blocking
/// launches until the user successfully saves (or reloads) a valid one; closing
/// the error message must never make the in-memory default substitute
/// launchable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConfigReadiness {
    #[default]
    Ready,
    RepairRequired,
}

impl AppState {
    pub fn new(config: GuardConfig, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            config_readiness: ConfigReadiness::Ready,
            desktop_app: DesktopAppDiscovery::Unknown,
            desktop_process: DesktopProcessState::Unknown,
            backend_proxy_state: BackendProxyRuntimeState::Unknown,
            launch: LaunchState::Idle,
            foreground: None,
            daemon_repair_prompt: false,
            backend_proxy_prompt: false,
            status_message: "Ready to launch through the configured proxy".into(),
            error_message: None,
            show_help: false,
            proxy_editor: None,
            should_quit: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> LaunchReceipt {
        LaunchReceipt {
            pid: 42,
            proxy_endpoint: Some("http://127.0.0.1:10808".into()),
            daemon_preparation: DaemonPreparation::Skipped,
            launch_method: LaunchMethod::AppmodelActivation,
            activation_state: ActivationState::Returned,
            instance: InstanceObservation::Created,
            package_identity: PackageIdentityObservation::Matched,
            aumid: AumidObservation::Matched,
            proxy_delivery: ProxyDelivery::ActivationArgumentsAndHomeConfig,
            backend_proxy_config: BackendProxyConfig::Prepared,
            target_elevation: Some(false),
            desktop: DesktopLaunchInfo {
                product: DesktopProduct::ChatGpt,
                package_name: "OpenAI.Codex".into(),
                package_version: "26.924.2738.0".into(),
                architecture: "X64".into(),
                discovery_source: DesktopDiscoverySource::AppxManifest,
            },
        }
    }

    #[test]
    fn launch_receipt_serializes_layered_activation_and_proxy_facts() {
        let value = serde_json::to_value(receipt()).unwrap();
        assert_eq!(value["desktop"]["product"], "chat_gpt");
        assert_eq!(value["launch_method"], "appmodel_activation");
        assert_eq!(value["activation_state"], "returned");
        assert_eq!(value["instance"], "created");
        assert_eq!(value["package_identity"], "matched");
        assert_eq!(value["aumid"], "matched");
        assert_eq!(
            value["proxy_delivery"],
            "activation_arguments_and_home_config"
        );
        assert_eq!(value["backend_proxy_config"], "prepared");
        assert_eq!(value["target_elevation"], false);
        assert_eq!(
            DaemonPreparation::Skipped.status_detail(),
            "",
            "normal launches make no claim about the daemon"
        );
    }

    #[test]
    fn identity_and_delivery_facts_stay_distinguishable() {
        // A single boolean could not represent these separate observations;
        // the enums make every combination explicit and serializable.
        assert_ne!(
            PackageIdentityObservation::Missing,
            PackageIdentityObservation::QueryFailed
        );
        assert_ne!(
            ProxyDelivery::ActivationArguments,
            ProxyDelivery::ActivationArgumentsAndHomeConfig
        );
        assert_ne!(AumidObservation::Missing, AumidObservation::Mismatch);
        assert_eq!(
            LaunchMethod::AppmodelActivation.display_name(),
            "Windows registered application activation"
        );
    }
}
