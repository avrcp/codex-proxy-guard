use crate::{DesktopAppInfo, DesktopProcessState, GuardConfig, LaunchReceipt, ProxyField};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserIntent {
    Launch,
    Refresh,
    EditProxy,
    UpdateProxyField { field: ProxyField, value: String },
    ToggleProxyField,
    SaveProxy,
    CancelProxyEdit,
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
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum TaskResult {
    LocalStateRefreshed {
        desktop_app: Result<DesktopAppInfo, String>,
        process: DesktopProcessState,
    },
    LaunchCompleted(Result<(DesktopAppInfo, LaunchReceipt), String>),
    ConfigSaved(Result<GuardConfig, String>),
}

#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    pub launch_process: bool,
    pub save_config: bool,
    pub quit: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            launch_process: true,
            save_config: true,
            quit: true,
        }
    }
}

impl Capabilities {
    pub fn authorize(&self, effect: &AppEffect) -> Result<(), String> {
        let allowed = match effect {
            AppEffect::RefreshLocalState => true,
            AppEffect::LaunchDesktop => self.launch_process,
            AppEffect::SaveConfig(_) => self.save_config,
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
            save_config: true,
            quit: true,
        };
        assert!(capabilities.authorize(&AppEffect::LaunchDesktop).is_err());
        assert!(
            capabilities
                .authorize(&AppEffect::RefreshLocalState)
                .is_ok()
        );
    }
}
