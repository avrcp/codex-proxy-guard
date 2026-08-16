use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{GuardConfig, NodeSelection, RegionBenchmarkCounts};

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
    KnownExecutableFallback,
    ExecutableOverride,
}

impl DesktopDiscoverySource {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::AppxManifest => "APPX manifest",
            Self::KnownExecutableFallback => "known APPX executable",
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
    Found(DesktopAppInfo),
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LaunchReceipt {
    pub pid: u32,
    pub proxy_endpoint: String,
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
    SyncSubscription,
    Benchmark,
    ManagedLaunch,
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

/// UI-facing view of Managed Mode state, kept free of credentials and outbounds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagedView {
    pub subscription_name: Option<String>,
    pub regions: RegionBenchmarkCounts,
    pub selected: Option<NodeSelection>,
    pub proxy_endpoint: Option<String>,
    pub proxy_lost: bool,
    /// Session-only manual override chosen from the Local Web Manager. Cleared on
    /// Guard restart, active-subscription change, or a fresh benchmark that no
    /// longer reports the node healthy.
    pub manual_selection: Option<NodeSelection>,
}

/// TUI-facing view of the on-demand Local Web Manager.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagerView {
    pub active: bool,
    pub display_url: Option<String>,
}

/// Result of a successful manager start, safe to display and store in TUI state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagerInfo {
    pub display_url: String,
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub config: GuardConfig,
    pub config_path: PathBuf,
    pub desktop_app: DesktopAppDiscovery,
    pub desktop_process: DesktopProcessState,
    pub launch: LaunchState,
    pub managed: ManagedView,
    pub manager: ManagerView,
    pub foreground: Option<ForegroundOperation>,
    pub status_message: String,
    pub error_message: Option<String>,
    pub show_help: bool,
    pub proxy_editor: Option<ProxyEditor>,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(config: GuardConfig, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            desktop_app: DesktopAppDiscovery::Unknown,
            desktop_process: DesktopProcessState::Unknown,
            launch: LaunchState::Idle,
            managed: ManagedView::default(),
            manager: ManagerView::default(),
            foreground: None,
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
    }
}
