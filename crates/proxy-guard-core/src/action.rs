use crate::{
    BenchmarkRunSummary, DesktopAppInfo, DesktopProcessState, GuardConfig, LaunchReceipt,
    ManagedLaunchReceipt, ManagedView, ManagerInfo, NodeId, NodeSelection, ProxyField,
    SubscriptionId, SubscriptionSyncSummary,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserIntent {
    Launch,
    Refresh,
    EditProxy,
    UpdateProxyField { field: ProxyField, value: String },
    ToggleProxyField,
    SaveProxy,
    CancelProxyEdit,
    SyncSubscription,
    BenchmarkNodes,
    CancelBenchmark,
    ToggleManager,
    ReopenManager,
    ToggleHelp,
    Dismiss,
    Quit,
}

#[derive(Clone, Debug)]
pub enum AppAction {
    Intent(UserIntent),
    TaskComplete(Box<TaskResult>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEffect {
    RefreshLocalState,
    LaunchDesktop,
    SaveConfig(GuardConfig),
    SyncSubscription(SubscriptionId),
    BenchmarkNodes,
    CancelBenchmark,
    LaunchManaged(NodeId),
    StopManagedProxy,
    OpenManager,
    ReopenManager,
    CloseManager,
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum TaskResult {
    LocalStateRefreshed {
        desktop_app: Result<DesktopAppInfo, String>,
        process: DesktopProcessState,
        managed: Result<ManagedView, String>,
    },
    LaunchCompleted(Result<(DesktopAppInfo, LaunchReceipt), String>),
    ConfigSaved(Result<GuardConfig, String>),
    SubscriptionSynced(Result<SubscriptionSyncSummary, String>),
    BenchmarkCompleted(Result<BenchmarkRunSummary, String>),
    ManagedLaunchCompleted(Result<ManagedLaunchReceipt, String>),
    ManagedProxyStopped(Result<(), String>),
    ManagedProxyLost(String),
    ManagerOpened(Result<ManagerInfo, String>),
    ManagerClosed,
    ManagerConfigUpdated(Result<GuardConfig, String>),
    ManagerManagedViewUpdated(Result<ManagedView, String>),
    ManagerSelectionChanged(Result<Option<NodeSelection>, String>),
}

#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    pub launch_process: bool,
    pub save_config: bool,
    pub manage_subscription: bool,
    pub benchmark_network: bool,
    pub manage_sidecar: bool,
    pub quit: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            launch_process: true,
            save_config: true,
            manage_subscription: true,
            benchmark_network: true,
            manage_sidecar: true,
            quit: true,
        }
    }
}

impl Capabilities {
    pub fn authorize(&self, effect: &AppEffect) -> Result<(), String> {
        let allowed = match effect {
            AppEffect::RefreshLocalState | AppEffect::CancelBenchmark => true,
            AppEffect::LaunchDesktop => self.launch_process,
            AppEffect::SaveConfig(_) => self.save_config,
            AppEffect::SyncSubscription(_) => self.manage_subscription,
            AppEffect::BenchmarkNodes => self.benchmark_network,
            AppEffect::LaunchManaged(_) | AppEffect::StopManagedProxy => self.manage_sidecar,
            AppEffect::OpenManager | AppEffect::ReopenManager | AppEffect::CloseManager => true,
            AppEffect::Shutdown => self.quit,
        };
        allowed
            .then_some(())
            .ok_or_else(|| format!("capability denied for {effect:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launching_requires_launch_capability() {
        let capabilities = Capabilities {
            launch_process: false,
            ..Capabilities::default()
        };
        assert!(capabilities.authorize(&AppEffect::LaunchDesktop).is_err());
        assert!(
            capabilities
                .authorize(&AppEffect::RefreshLocalState)
                .is_ok()
        );
    }

    #[test]
    fn benchmark_and_sidecar_are_independently_gated() {
        let capabilities = Capabilities {
            benchmark_network: false,
            manage_sidecar: false,
            ..Capabilities::default()
        };
        assert!(capabilities.authorize(&AppEffect::BenchmarkNodes).is_err());
        assert!(
            capabilities
                .authorize(&AppEffect::LaunchManaged(crate::NodeId::new()))
                .is_err()
        );
        assert!(capabilities.authorize(&AppEffect::Shutdown).is_ok());
    }
}
