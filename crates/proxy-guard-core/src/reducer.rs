use crate::{
    AppAction, AppEffect, AppState, ConfigReadiness, DesktopAppDiscovery, DesktopProcessState,
    ForegroundOperation, LaunchOptions, LaunchReceipt, LaunchState, ProxyEditor, ProxyField,
    TaskResult, UserIntent, redact_text,
};

pub fn reduce(state: &mut AppState, action: AppAction) -> Vec<AppEffect> {
    match action {
        AppAction::Intent(intent) => reduce_intent(state, intent),
        AppAction::TaskComplete(result) => reduce_result(state, *result),
    }
}

fn reduce_intent(state: &mut AppState, intent: UserIntent) -> Vec<AppEffect> {
    if intent == UserIntent::Quit {
        state.should_quit = true;
        return vec![AppEffect::Shutdown];
    }

    // The backend proxy consent is modal: only Y / N / Esc act on it, and
    // refresh or proxy editing simply closes it. Y authorizes exactly the
    // displayed home.
    if state.backend_proxy_prompt {
        match intent {
            UserIntent::ConfirmBackendProxyConsent => {
                state.backend_proxy_prompt = false;
                let Some(home) = consent_home(state) else {
                    state.status_message =
                        "Cannot resolve the Codex Home to bind; set an absolute path first".into();
                    return Vec::new();
                };
                if state.config_readiness != ConfigReadiness::Ready {
                    state.status_message =
                        "Configuration must be repaired before launch (press C)".into();
                    return Vec::new();
                }
                if state.foreground.is_some() {
                    state.status_message =
                        "A configuration operation is already in progress".into();
                    return Vec::new();
                }
                let enable = !state.config.codex.manage_codex_proxy_env;
                state.foreground = Some(ForegroundOperation::UpdateBackendProxyConsent);
                state.status_message = if enable {
                    "Recording the backend proxy consent…".into()
                } else {
                    "Revoking the backend proxy consent and removing Guard's block…".into()
                };
                return vec![AppEffect::UpdateBackendProxyConsent { enable, home }];
            }
            UserIntent::CancelBackendProxyConsent => {
                state.backend_proxy_prompt = false;
                state.status_message =
                    "Backend proxy consent unchanged; no Codex Home file was touched".into();
                return Vec::new();
            }
            UserIntent::Refresh | UserIntent::EditProxy => {
                state.backend_proxy_prompt = false;
            }
            _ => return Vec::new(),
        }
    } else {
        // The repair confirmation is modal: only Y / N / Esc act on it, and
        // refresh or proxy editing simply closes it. Y authorizes exactly one
        // launch. Package identity problems are never a reason to stop the
        // shared daemon; the stop only happens for its own sake.
        if state.daemon_repair_prompt {
            match intent {
                UserIntent::ConfirmDaemonRepairLaunch => {
                    state.daemon_repair_prompt = false;
                    if state.config_readiness != ConfigReadiness::Ready {
                        state.status_message =
                            "Configuration must be repaired before launch (press C)".into();
                        return Vec::new();
                    }
                    if state.foreground.is_some() {
                        state.status_message = "A launch operation is already in progress".into();
                        return Vec::new();
                    }
                    state.foreground = Some(ForegroundOperation::Launch);
                    state.launch = LaunchState::Launching;
                    state.status_message = "Stopping the shared Codex background server, then launching… (you can cancel)".into();
                    return vec![AppEffect::LaunchDesktop(LaunchOptions {
                        refresh_codex_daemon: true,
                        activation_only: false,
                    })];
                }
                UserIntent::CancelDaemonRepairLaunch => {
                    state.daemon_repair_prompt = false;
                    state.status_message =
                        "Repair launch cancelled; the shared Codex background server was not touched"
                            .into();
                    return Vec::new();
                }
                UserIntent::RequestDaemonRepairLaunch => return Vec::new(),
                UserIntent::Refresh | UserIntent::EditProxy => {
                    state.daemon_repair_prompt = false;
                }
                _ => return Vec::new(),
            }
        }
    }

    if intent == UserIntent::EditProxy {
        if state.foreground.is_none() {
            state.proxy_editor = Some(ProxyEditor {
                host: state.config.proxy.host.clone(),
                port: state.config.proxy.port.to_string(),
                active_field: ProxyField::Port,
                error: None,
            });
            state.error_message = None;
            state.status_message = "Edit the local HTTP/Mixed proxy endpoint".into();
        }
        return Vec::new();
    }
    if state.proxy_editor.is_some() && state.foreground.is_some() {
        return Vec::new();
    }
    if let Some(editor) = &mut state.proxy_editor {
        match intent {
            UserIntent::UpdateProxyField { field, value } => {
                match field {
                    ProxyField::Host => editor.host = value,
                    ProxyField::Port => editor.port = value,
                }
                editor.active_field = field;
                editor.error = None;
            }
            UserIntent::ToggleProxyField => {
                editor.active_field = match editor.active_field {
                    ProxyField::Host => ProxyField::Port,
                    ProxyField::Port => ProxyField::Host,
                };
                editor.error = None;
            }
            UserIntent::CancelProxyEdit => {
                state.proxy_editor = None;
                state.status_message = "Proxy configuration unchanged".into();
            }
            UserIntent::SaveProxy => {
                let port = match editor.port.trim().parse::<u16>() {
                    Ok(0) | Err(_) => {
                        editor.error = Some("Port must be a number between 1 and 65535".into());
                        return Vec::new();
                    }
                    Ok(port) => port,
                };
                let mut updated = state.config.clone();
                updated.proxy.host = editor.host.trim().into();
                updated.proxy.port = port;
                if let Err(error) = updated.validate() {
                    editor.error = Some(error.to_string());
                    return Vec::new();
                }
                state.foreground = Some(ForegroundOperation::SaveConfig);
                state.status_message = "Saving proxy configuration…".into();
                return vec![AppEffect::SaveConfig(updated)];
            }
            _ => {}
        }
        return Vec::new();
    }
    if intent == UserIntent::ToggleHelp {
        state.show_help = !state.show_help;
        return Vec::new();
    }
    if intent == UserIntent::Dismiss {
        state.show_help = false;
        state.error_message = None;
        return Vec::new();
    }
    if state.show_help {
        return Vec::new();
    }

    // An unrepaired configuration keeps blocking every launch entry. Clearing
    // or dismissing the visible error must never turn the in-memory default
    // substitute into a launchable configuration.
    if state.config_readiness != ConfigReadiness::Ready
        && matches!(
            intent,
            UserIntent::Launch
                | UserIntent::RequestDaemonRepairLaunch
                | UserIntent::RequestBackendProxyConsent
        )
    {
        state.status_message = "Configuration must be repaired before launch (press C)".into();
        return Vec::new();
    }
    if state.error_message.is_some() {
        state.error_message = None;
        return Vec::new();
    }
    if state.foreground.is_some() {
        state.status_message = "A launch operation is already in progress".into();
        return Vec::new();
    }

    match intent {
        UserIntent::Launch => {
            state.foreground = Some(ForegroundOperation::Launch);
            state.launch = LaunchState::Launching;
            state.status_message = if registered_package_found(state) {
                "Activating the registered Desktop application…".into()
            } else {
                "Launching Desktop with the proxy environment…".into()
            };
            vec![AppEffect::LaunchDesktop(LaunchOptions::default())]
        }
        UserIntent::RequestDaemonRepairLaunch => {
            state.daemon_repair_prompt = true;
            state.status_message = "Confirm the shared-daemon repair launch".into();
            Vec::new()
        }
        UserIntent::RequestBackendProxyConsent => {
            if consent_home(state).is_none() {
                state.status_message = "Cannot resolve a Codex Home candidate to ask about".into();
                return Vec::new();
            }
            state.backend_proxy_prompt = true;
            state.status_message = "Confirm the backend proxy configuration consent".into();
            Vec::new()
        }
        UserIntent::Refresh => {
            state.foreground = Some(ForegroundOperation::Refresh);
            state.desktop_app = DesktopAppDiscovery::Searching;
            state.status_message = "Refreshing Desktop status…".into();
            vec![AppEffect::RefreshLocalState]
        }
        UserIntent::EditProxy
        | UserIntent::UpdateProxyField { .. }
        | UserIntent::ToggleProxyField
        | UserIntent::SaveProxy
        | UserIntent::CancelProxyEdit
        | UserIntent::ToggleHelp
        | UserIntent::Dismiss
        | UserIntent::Quit => unreachable!(),
        // A stale confirmation may arrive after its modal was consumed.
        UserIntent::ConfirmDaemonRepairLaunch
        | UserIntent::CancelDaemonRepairLaunch
        | UserIntent::ConfirmBackendProxyConsent
        | UserIntent::CancelBackendProxyConsent => Vec::new(),
    }
}

/// The home the consent prompt binds to: an already-bound absolute home, or
/// the default `.codex` candidate under the user profile. Guard never derives
/// this from its own CODEX_HOME.
fn consent_home(state: &AppState) -> Option<std::path::PathBuf> {
    let bound = &state.config.codex.proxy_env_home;
    if bound.as_os_str().is_empty() {
        crate::GuardConfig::default_codex_home()
    } else if bound.is_absolute() {
        Some(bound.clone())
    } else {
        None
    }
}

fn registered_package_found(state: &AppState) -> bool {
    matches!(
        &state.desktop_app,
        DesktopAppDiscovery::Found(info)
            if matches!(info.target_kind, crate::DesktopTargetKind::RegisteredPackage(_))
    )
}

fn reduce_result(state: &mut AppState, result: TaskResult) -> Vec<AppEffect> {
    // Guard is shutting down: never consume a late task result into UI state,
    // and never schedule new effects during shutdown.
    if state.should_quit {
        return Vec::new();
    }
    match result {
        TaskResult::LocalStateRefreshed {
            desktop_app,
            process,
        } => {
            if state.foreground != Some(ForegroundOperation::Refresh) {
                return Vec::new();
            }
            state.foreground = None;
            state.desktop_process = process;
            match desktop_app {
                Ok(info) => {
                    state.desktop_app = DesktopAppDiscovery::Found(Box::new(info));
                    state.status_message = match state.desktop_process {
                        DesktopProcessState::Running { .. } => "Desktop is already running".into(),
                        _ => "Ready to launch through the configured proxy".into(),
                    };
                }
                Err(message) => {
                    let message = redact_text(&message);
                    state.desktop_app = DesktopAppDiscovery::NotFound(message.clone());
                    state.status_message = "Desktop executable was not found".into();
                }
            }
        }
        TaskResult::LaunchCompleted(result) => {
            if state.foreground != Some(ForegroundOperation::Launch) {
                return Vec::new();
            }
            state.foreground = None;
            match result {
                Ok((info, receipt)) => {
                    state.desktop_app = DesktopAppDiscovery::Found(Box::new(info));
                    state.desktop_process = DesktopProcessState::Running { pid: receipt.pid };
                    state.status_message = launch_status_message(&receipt);
                    state.launch = LaunchState::Running(receipt);
                }
                Err(message) => {
                    let message = redact_text(&message);
                    state.status_message = "Desktop launch was blocked".into();
                    state.error_message = Some(message.clone());
                    state.launch = LaunchState::Blocked(message);
                }
            }
        }
        TaskResult::ConfigSaved(result) => {
            if state.foreground != Some(ForegroundOperation::SaveConfig) {
                return Vec::new();
            }
            state.foreground = None;
            match result {
                Ok(config) => {
                    state.config = config;
                    state.proxy_editor = None;
                    state.config_readiness = ConfigReadiness::Ready;
                    state.status_message = "Proxy configuration saved".into();
                }
                Err(message) => {
                    if let Some(editor) = &mut state.proxy_editor {
                        editor.error = Some(redact_text(&message));
                    }
                    state.status_message = "Proxy configuration was not saved".into();
                }
            }
        }
        TaskResult::BackendProxyConsentUpdated(result) => {
            if state.foreground != Some(ForegroundOperation::UpdateBackendProxyConsent) {
                return Vec::new();
            }
            state.foreground = None;
            match result {
                Ok(config) => {
                    state.config = config;
                    if state.config.codex.manage_codex_proxy_env {
                        state.status_message =
                            "Backend proxy consent recorded for the bound Codex Home; the block is prepared by the next launch"
                                .into();
                    } else {
                        state.status_message =
                            "Backend proxy consent revoked; Guard's managed block was removed when it was unmodified"
                                .into();
                    }
                }
                Err(message) => {
                    let message = redact_text(&message);
                    state.status_message = "Backend proxy consent was not changed".into();
                    state.error_message = Some(message);
                }
            }
        }
    }
    Vec::new()
}

/// Reports only the facts Guard actually observed: activation, identity,
/// and each proxy layer separately.
fn launch_status_message(receipt: &LaunchReceipt) -> String {
    let mut parts = Vec::new();
    match receipt.launch_method {
        crate::LaunchMethod::AppmodelActivation => {
            let instance = match receipt.instance {
                crate::InstanceObservation::Created => "new instance activated",
                crate::InstanceObservation::Reused => "existing instance returned",
                crate::InstanceObservation::Unknown => "instance returned",
            };
            parts.push(format!(
                "Registered application activation returned PID {}; {instance}",
                receipt.pid
            ));
            match receipt.package_identity {
                crate::PackageIdentityObservation::Matched => {
                    parts.push("package identity matched".into())
                }
                crate::PackageIdentityObservation::NotApplicable => {}
                observation => parts.push(format!("package identity: {observation:?}")),
            }
            match receipt.aumid {
                crate::AumidObservation::Matched => {
                    parts.push("application identity matched".into())
                }
                crate::AumidObservation::NotApplicable => {}
                observation => parts.push(format!("application identity: {observation:?}")),
            }
        }
        crate::LaunchMethod::NativeProcess => {
            parts.push(format!(
                "Desktop process created with the proxy environment (PID {})",
                receipt.pid
            ));
        }
    }
    match receipt.proxy_delivery {
        crate::ProxyDelivery::ProcessEnvironment => {}
        crate::ProxyDelivery::ActivationArguments => parts
            .push("Chromium proxy arguments submitted; backend home config not authorized".into()),
        crate::ProxyDelivery::ActivationArgumentsAndHomeConfig => {
            parts.push("Chromium proxy arguments submitted; backend home config prepared".into())
        }
        crate::ProxyDelivery::NotEstablished => {
            parts.push("no proxy delivered (activation-only)".into())
        }
    }
    let detail = receipt.daemon_preparation.status_detail();
    if !detail.is_empty() {
        parts.push(detail.into());
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DaemonPreparation, DesktopDiscoverySource, DesktopLaunchInfo, DesktopProduct, GuardConfig,
    };
    use std::path::PathBuf;

    fn state() -> AppState {
        AppState::new(GuardConfig::default(), PathBuf::from("config.toml"))
    }

    #[test]
    fn only_one_foreground_operation_is_allowed() {
        let mut state = state();
        assert_eq!(
            reduce(&mut state, AppAction::Intent(UserIntent::Launch)),
            vec![AppEffect::LaunchDesktop(LaunchOptions::default())]
        );
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::Refresh)).is_empty());
        assert_eq!(state.foreground, Some(ForegroundOperation::Launch));
    }

    #[test]
    fn daemon_repair_requires_an_explicit_single_use_confirmation() {
        let mut state = state();
        // D opens the confirmation; Enter and other keys do nothing while it shows.
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::RequestDaemonRepairLaunch)
            )
            .is_empty()
        );
        assert!(state.daemon_repair_prompt);
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::Launch)).is_empty());
        // Esc cancels without any launch effect.
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::CancelDaemonRepairLaunch)
            )
            .is_empty()
        );
        assert!(!state.daemon_repair_prompt);
        // Y only acts while the prompt is shown; exactly one authorized launch
        // that never carries activation-only semantics.
        reduce(
            &mut state,
            AppAction::Intent(UserIntent::RequestDaemonRepairLaunch),
        );
        assert_eq!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::ConfirmDaemonRepairLaunch)
            ),
            vec![AppEffect::LaunchDesktop(LaunchOptions {
                refresh_codex_daemon: true,
                activation_only: false,
            })]
        );
        assert!(!state.daemon_repair_prompt);
    }

    #[test]
    fn repair_confirmation_no_longer_carries_a_package_context_choice() {
        let mut state = state();
        state.desktop_app = DesktopAppDiscovery::Found(Box::new(crate::DesktopAppInfo {
            product: DesktopProduct::ChatGpt,
            package_name: "OpenAI.Codex".into(),
            package_version: "1".into(),
            architecture: "X64".into(),
            discovery_source: DesktopDiscoverySource::AppxManifest,
            target_kind: crate::DesktopTargetKind::RegisteredPackage(crate::PackageApplication {
                package_full_name: "OpenAI.Codex_1_x64__test".into(),
                package_family_name: "OpenAI.Codex_test".into(),
                application_id: "App".into(),
                app_user_model_id: "OpenAI.Codex_test!App".into(),
                manifest_executable: "app/ChatGPT.exe".into(),
                runtime_kind: crate::PackageRuntimeKind::FullTrustDesktop,
            }),
            install_location: PathBuf::from("app"),
            executable: PathBuf::from("app/ChatGPT.exe"),
        }));
        reduce(
            &mut state,
            AppAction::Intent(UserIntent::RequestDaemonRepairLaunch),
        );
        assert_eq!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::ConfirmDaemonRepairLaunch)
            ),
            vec![AppEffect::LaunchDesktop(LaunchOptions {
                refresh_codex_daemon: true,
                activation_only: false,
            })]
        );
    }

    #[test]
    fn backend_proxy_consent_is_modal_and_binds_the_displayed_home() {
        let mut state = state();
        state.config.codex.proxy_env_home = PathBuf::from(r"C:\Users\fixture\.codex");
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::RequestBackendProxyConsent)
            )
            .is_empty()
        );
        assert!(state.backend_proxy_prompt);
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::Launch)).is_empty());
        // Esc cancels without touching any Codex Home file.
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::CancelBackendProxyConsent)
            )
            .is_empty()
        );
        assert!(!state.backend_proxy_prompt);
        // A stale confirm after the modal closed does nothing.
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::ConfirmBackendProxyConsent)
            )
            .is_empty()
        );
        reduce(
            &mut state,
            AppAction::Intent(UserIntent::RequestBackendProxyConsent),
        );
        assert_eq!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::ConfirmBackendProxyConsent)
            ),
            vec![AppEffect::UpdateBackendProxyConsent {
                enable: true,
                home: PathBuf::from(r"C:\Users\fixture\.codex"),
            }]
        );
        assert!(!state.backend_proxy_prompt);
        // Deliver the consent task result, then confirm again (now enabled):
        // the same prompt revokes instead.
        let mut updated = state.config.clone();
        updated.codex.manage_codex_proxy_env = true;
        updated.codex.proxy_env_home = PathBuf::from(r"C:\Users\fixture\.codex");
        reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::BackendProxyConsentUpdated(Ok(
                updated,
            )))),
        );
        assert!(state.config.codex.manage_codex_proxy_env);
        reduce(
            &mut state,
            AppAction::Intent(UserIntent::RequestBackendProxyConsent),
        );
        assert_eq!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::ConfirmBackendProxyConsent)
            ),
            vec![AppEffect::UpdateBackendProxyConsent {
                enable: false,
                home: PathBuf::from(r"C:\Users\fixture\.codex"),
            }]
        );
    }

    #[test]
    fn backend_proxy_consent_needs_a_resolvable_home() {
        let mut state = state();
        state.config.codex.proxy_env_home = PathBuf::from("relative/.codex");
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::RequestBackendProxyConsent)
            )
            .is_empty()
        );
        assert!(!state.backend_proxy_prompt);
    }

    #[test]
    fn refresh_closes_the_repair_confirmation_without_launching() {
        let mut state = state();
        reduce(
            &mut state,
            AppAction::Intent(UserIntent::RequestDaemonRepairLaunch),
        );
        let effects = reduce(&mut state, AppAction::Intent(UserIntent::Refresh));
        assert!(
            effects
                .iter()
                .all(|effect| !matches!(effect, AppEffect::LaunchDesktop(_)))
        );
        assert!(!state.daemon_repair_prompt);
    }

    #[test]
    fn unrepaired_configuration_blocks_every_launch_entry() {
        let mut state = state();
        state.config_readiness = ConfigReadiness::RepairRequired;
        state.error_message = Some("CONFIG_INVALID: broken".into());
        // Launch and repair requests are blocked before any effect is produced,
        // even after the visible error has been dismissed.
        assert!(
            reduce(&mut state, AppAction::Intent(UserIntent::Launch)).is_empty(),
            "launch must be blocked while the configuration is unrepaired"
        );
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::RequestDaemonRepairLaunch)
            )
            .is_empty()
        );
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::RequestBackendProxyConsent)
            )
            .is_empty()
        );
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::Dismiss)).is_empty());
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::Launch)).is_empty());
        assert_eq!(state.config_readiness, ConfigReadiness::RepairRequired);
        // A failed save keeps the block; only a successful save lifts it. The
        // save results arrive as if a SaveConfig operation was in flight.
        state.foreground = Some(ForegroundOperation::SaveConfig);
        reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::ConfigSaved(Err("disk full".into())))),
        );
        assert_eq!(state.config_readiness, ConfigReadiness::RepairRequired);
        state.foreground = Some(ForegroundOperation::SaveConfig);
        reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::ConfigSaved(
                Ok(GuardConfig::default()),
            ))),
        );
        assert_eq!(state.config_readiness, ConfigReadiness::Ready);
        assert_eq!(
            reduce(&mut state, AppAction::Intent(UserIntent::Launch)),
            vec![AppEffect::LaunchDesktop(LaunchOptions::default())]
        );
    }

    #[test]
    fn backend_consent_result_updates_config_and_lifts_the_operation() {
        let mut state = state();
        state.foreground = Some(ForegroundOperation::UpdateBackendProxyConsent);
        let mut updated = GuardConfig::default();
        updated.codex.manage_codex_proxy_env = true;
        updated.codex.proxy_env_home = PathBuf::from(r"C:\Users\fixture\.codex");
        assert!(
            reduce(
                &mut state,
                AppAction::TaskComplete(Box::new(TaskResult::BackendProxyConsentUpdated(Ok(
                    updated
                ))))
            )
            .is_empty()
        );
        assert_eq!(state.foreground, None);
        assert!(state.config.codex.manage_codex_proxy_env);
        assert!(state.status_message.contains("next launch"));
        // A late result without the operation in flight is ignored.
        assert!(
            reduce(
                &mut state,
                AppAction::TaskComplete(Box::new(TaskResult::BackendProxyConsentUpdated(Ok(
                    GuardConfig::default()
                ))))
            )
            .is_empty()
        );
        assert!(state.config.codex.manage_codex_proxy_env);
    }

    #[test]
    fn launch_completion_updates_process_without_extra_effects() {
        let mut state = state();
        reduce(&mut state, AppAction::Intent(UserIntent::Launch));
        let info = crate::DesktopAppInfo {
            product: DesktopProduct::ChatGpt,
            package_name: "OpenAI.Codex".into(),
            package_version: "1".into(),
            architecture: "X64".into(),
            discovery_source: DesktopDiscoverySource::AppxManifest,
            target_kind: crate::DesktopTargetKind::UnpackagedExecutable,
            install_location: PathBuf::from("app"),
            executable: PathBuf::from("app/Codex.exe"),
        };
        let receipt = LaunchReceipt {
            pid: 42,
            proxy_endpoint: Some("http://127.0.0.1:10808".into()),
            daemon_preparation: DaemonPreparation::Skipped,
            launch_method: crate::LaunchMethod::NativeProcess,
            activation_state: crate::ActivationState::NotSubmitted,
            instance: crate::InstanceObservation::Created,
            package_identity: crate::PackageIdentityObservation::NotApplicable,
            aumid: crate::AumidObservation::NotApplicable,
            proxy_delivery: crate::ProxyDelivery::ProcessEnvironment,
            backend_proxy_config: crate::BackendProxyConfig::NotApplicable,
            target_elevation: None,
            desktop: DesktopLaunchInfo::from(&info),
        };
        assert!(
            reduce(
                &mut state,
                AppAction::TaskComplete(Box::new(TaskResult::LaunchCompleted(Ok((info, receipt)))))
            )
            .is_empty()
        );
        assert_eq!(
            state.desktop_process,
            DesktopProcessState::Running { pid: 42 }
        );
    }

    #[test]
    fn late_results_are_ignored_after_quit() {
        let mut state = state();
        reduce(&mut state, AppAction::Intent(UserIntent::Quit));
        let info = crate::DesktopAppInfo {
            product: DesktopProduct::ChatGpt,
            package_name: "OpenAI.Codex".into(),
            package_version: "1".into(),
            architecture: "X64".into(),
            discovery_source: DesktopDiscoverySource::AppxManifest,
            target_kind: crate::DesktopTargetKind::UnpackagedExecutable,
            install_location: PathBuf::from("app"),
            executable: PathBuf::from("app/Codex.exe"),
        };
        let receipt = LaunchReceipt {
            pid: 42,
            proxy_endpoint: Some("http://127.0.0.1:10808".into()),
            daemon_preparation: DaemonPreparation::NotNeeded,
            launch_method: crate::LaunchMethod::NativeProcess,
            activation_state: crate::ActivationState::NotSubmitted,
            instance: crate::InstanceObservation::Created,
            package_identity: crate::PackageIdentityObservation::NotApplicable,
            aumid: crate::AumidObservation::NotApplicable,
            proxy_delivery: crate::ProxyDelivery::ProcessEnvironment,
            backend_proxy_config: crate::BackendProxyConfig::NotApplicable,
            target_elevation: None,
            desktop: DesktopLaunchInfo::from(&info),
        };
        assert!(
            reduce(
                &mut state,
                AppAction::TaskComplete(Box::new(TaskResult::LaunchCompleted(Ok((info, receipt)))))
            )
            .is_empty()
        );
        assert_eq!(state.launch, LaunchState::Idle);
    }

    #[test]
    fn launch_status_reports_each_layer_separately() {
        let base = || LaunchReceipt {
            pid: 7,
            proxy_endpoint: Some("http://127.0.0.1:10808".into()),
            daemon_preparation: DaemonPreparation::Skipped,
            launch_method: crate::LaunchMethod::AppmodelActivation,
            activation_state: crate::ActivationState::Returned,
            instance: crate::InstanceObservation::Created,
            package_identity: crate::PackageIdentityObservation::Matched,
            aumid: crate::AumidObservation::Matched,
            proxy_delivery: crate::ProxyDelivery::ActivationArguments,
            backend_proxy_config: crate::BackendProxyConfig::NotAuthorized,
            target_elevation: Some(false),
            desktop: DesktopLaunchInfo {
                product: DesktopProduct::ChatGpt,
                package_name: "OpenAI.Codex".into(),
                package_version: "1".into(),
                architecture: "X64".into(),
                discovery_source: DesktopDiscoverySource::AppxManifest,
            },
        };
        let mut receipt = base();
        assert_eq!(
            launch_status_message(&receipt),
            "Registered application activation returned PID 7; new instance activated; \
             package identity matched; application identity matched; \
             Chromium proxy arguments submitted; backend home config not authorized"
        );
        receipt.proxy_delivery = crate::ProxyDelivery::NotEstablished;
        assert!(
            launch_status_message(&receipt).contains("no proxy delivered (activation-only)"),
            "activation-only must not imply delivered proxying"
        );
        receipt.daemon_preparation = DaemonPreparation::NotNeeded;
        assert!(
            launch_status_message(&receipt)
                .contains("the shared Codex background server was not running")
        );
        let mut native = base();
        native.launch_method = crate::LaunchMethod::NativeProcess;
        native.activation_state = crate::ActivationState::NotSubmitted;
        native.package_identity = crate::PackageIdentityObservation::NotApplicable;
        native.aumid = crate::AumidObservation::NotApplicable;
        native.proxy_delivery = crate::ProxyDelivery::ProcessEnvironment;
        assert_eq!(
            launch_status_message(&native),
            "Desktop process created with the proxy environment (PID 7)"
        );
    }

    #[test]
    fn quit_never_changes_external_process_state() {
        let mut state = state();
        state.desktop_process = DesktopProcessState::Running { pid: 7 };
        assert_eq!(
            reduce(&mut state, AppAction::Intent(UserIntent::Quit)),
            vec![AppEffect::Shutdown]
        );
        assert_eq!(
            state.desktop_process,
            DesktopProcessState::Running { pid: 7 }
        );
    }

    #[test]
    fn proxy_editor_validates_then_commits_only_after_save() {
        let mut state = state();
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::EditProxy)).is_empty());
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::UpdateProxyField {
                    field: ProxyField::Port,
                    value: "7890".into(),
                })
            )
            .is_empty()
        );
        let effects = reduce(&mut state, AppAction::Intent(UserIntent::SaveProxy));
        assert_eq!(state.config.proxy.port, 10808);
        assert_eq!(state.foreground, Some(ForegroundOperation::SaveConfig));
        let AppEffect::SaveConfig(updated) = effects.into_iter().next().unwrap() else {
            panic!("expected configuration save effect");
        };
        assert_eq!(updated.proxy.port, 7890);
        reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::ConfigSaved(Ok(updated)))),
        );
        assert_eq!(state.config.proxy.port, 7890);
        assert!(state.proxy_editor.is_none());
    }
}
