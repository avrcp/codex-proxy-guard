use std::path::PathBuf;

use crate::{
    DesktopAppInfo, DesktopProcessState, GuardConfig, LaunchOptions, LaunchReceipt, ProxyField,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserIntent {
    Launch,
    RequestDaemonRepairLaunch,
    ConfirmDaemonRepairLaunch,
    CancelDaemonRepairLaunch,
    RequestBackendProxyConsent,
    ConfirmBackendProxyConsent,
    CancelBackendProxyConsent,
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
    LaunchDesktop(LaunchOptions),
    SaveConfig(GuardConfig),
    /// Enable or disable the explicitly authorized Codex Home `.env` proxy
    /// block. Disabling additionally removes Guard's own managed block from
    /// the bound home; enabling only records the consent — the block itself is
    /// prepared by the next launch, never at consent time.
    UpdateBackendProxyConsent {
        enable: bool,
        /// Absolute Codex Home whose `.env` the consent is bound to.
        home: PathBuf,
    },
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
    BackendProxyConsentUpdated(Result<GuardConfig, String>),
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
            AppEffect::LaunchDesktop(_) => self.launch_process,
            AppEffect::SaveConfig(_) => self.save_config,
            // Managing the consent flag is a configuration change; the `.env`
            // block itself is only written for a launch under this consent.
            AppEffect::UpdateBackendProxyConsent { .. } => self.save_config,
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
    use crate::LaunchOptions;

    #[test]
    fn launching_requires_launch_capability() {
        let capabilities = Capabilities {
            launch_process: false,
            save_config: true,
            quit: true,
        };
        assert!(
            capabilities
                .authorize(&AppEffect::LaunchDesktop(LaunchOptions::default()))
                .is_err()
        );
        assert!(
            capabilities
                .authorize(&AppEffect::LaunchDesktop(LaunchOptions {
                    refresh_codex_daemon: true,
                    activation_only: false,
                }))
                .is_err()
        );
        assert!(
            capabilities
                .authorize(&AppEffect::RefreshLocalState)
                .is_ok()
        );
    }

    #[test]
    fn backend_consent_requires_save_capability() {
        let capabilities = Capabilities {
            launch_process: true,
            save_config: false,
            quit: true,
        };
        assert!(
            capabilities
                .authorize(&AppEffect::UpdateBackendProxyConsent {
                    enable: true,
                    home: PathBuf::from(r"C:\Users\example\.codex"),
                })
                .is_err()
        );
    }
}
