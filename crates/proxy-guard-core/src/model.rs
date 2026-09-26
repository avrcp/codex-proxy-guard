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
#[serde(rename_all = "snake_case")]
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
    pub package_context_compat: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMethod {
    NativeProcess,
    PackagedContextCompat,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageIdentityObservation {
    NotApplicable,
    Verified,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LaunchReceipt {
    pub pid: u32,
    pub proxy_endpoint: String,
    pub daemon_preparation: DaemonPreparation,
    pub launch_method: LaunchMethod,
    pub package_identity: PackageIdentityObservation,
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
    pub launch: LaunchState,
    pub foreground: Option<ForegroundOperation>,
    pub daemon_repair_prompt: bool,
    pub package_context_prompt: bool,
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
            launch: LaunchState::Idle,
            foreground: None,
            daemon_repair_prompt: false,
            package_context_prompt: false,
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

    #[test]
    fn launch_receipt_serializes_selected_desktop_metadata() {
        let receipt = LaunchReceipt {
            pid: 42,
            proxy_endpoint: "http://127.0.0.1:10808".into(),
            daemon_preparation: DaemonPreparation::Stopped,
            launch_method: LaunchMethod::PackagedContextCompat,
            package_identity: PackageIdentityObservation::Verified,
            desktop: DesktopLaunchInfo {
                product: DesktopProduct::ChatGpt,
                package_name: "OpenAI.Codex".into(),
                package_version: "26.727.6591.0".into(),
                architecture: "X64".into(),
                discovery_source: DesktopDiscoverySource::AppxManifest,
            },
        };
        let value = serde_json::to_value(receipt).unwrap();
        assert_eq!(value["desktop"]["product"], "chat_gpt");
        assert_eq!(value["desktop"]["architecture"], "X64");
        assert_eq!(value["desktop"]["discovery_source"], "appx_manifest");
        assert_eq!(value["daemon_preparation"], "stopped");
        assert_eq!(value["launch_method"], "packaged_context_compat");
        assert_eq!(value["package_identity"], "verified");
        assert_eq!(
            DaemonPreparation::Skipped.status_detail(),
            "",
            "normal launches make no claim about the daemon"
        );
        assert_eq!(
            DaemonPreparation::NotNeeded.status_detail(),
            "the shared Codex background server was not running"
        );
    }
}
