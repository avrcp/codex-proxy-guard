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

    if state.package_context_prompt {
        match intent {
            UserIntent::ConfirmPackageContextLaunch => {
                state.package_context_prompt = false;
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
                state.status_message = "Launching the package-context candidate…".into();
                return vec![AppEffect::LaunchDesktop(LaunchOptions {
                    refresh_codex_daemon: false,
                    package_context_compat: true,
                })];
            }
            UserIntent::CancelPackageContextLaunch => {
                state.package_context_prompt = false;
                state.status_message = "Package-context launch cancelled".into();
                return Vec::new();
            }
            UserIntent::Refresh | UserIntent::EditProxy => {
                state.package_context_prompt = false;
            }
            _ => return Vec::new(),
        }
    }

    // The repair confirmation is modal: only Y / N / Esc act on it, and refresh
    // or proxy editing simply closes it. Y authorizes exactly one launch.
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
                let package_context_compat = matches!(
                    &state.desktop_app,
                    DesktopAppDiscovery::Found(info)
                        if matches!(info.target_kind, crate::DesktopTargetKind::RegisteredPackage(_))
                );
                state.status_message =
                    "Stopping the shared Codex background server, then launching… (you can cancel)"
                        .into();
                return vec![AppEffect::LaunchDesktop(LaunchOptions {
                    refresh_codex_daemon: true,
                    package_context_compat,
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
            UserIntent::Launch
            | UserIntent::RequestPackageContextLaunch
            | UserIntent::ConfirmPackageContextLaunch
            | UserIntent::CancelPackageContextLaunch
            | UserIntent::UpdateProxyField { .. }
            | UserIntent::ToggleProxyField
            | UserIntent::SaveProxy
            | UserIntent::CancelProxyEdit
            | UserIntent::ToggleHelp
            | UserIntent::Dismiss
            | UserIntent::Quit => return Vec::new(),
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
            UserIntent::Launch
            | UserIntent::RequestPackageContextLaunch
            | UserIntent::ConfirmPackageContextLaunch
            | UserIntent::CancelPackageContextLaunch
            | UserIntent::RequestDaemonRepairLaunch
            | UserIntent::ConfirmDaemonRepairLaunch
            | UserIntent::CancelDaemonRepairLaunch
            | UserIntent::Refresh
            | UserIntent::ToggleHelp
            | UserIntent::Dismiss
            | UserIntent::Quit
            | UserIntent::EditProxy => {}
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
                | UserIntent::RequestPackageContextLaunch
                | UserIntent::RequestDaemonRepairLaunch
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
            state.status_message = "Launching Desktop with the proxy environment…".into();
            vec![AppEffect::LaunchDesktop(LaunchOptions::default())]
        }
        UserIntent::RequestPackageContextLaunch => {
            state.package_context_prompt = true;
            state.status_message = "Confirm the package-context candidate launch".into();
            Vec::new()
        }
        UserIntent::RequestDaemonRepairLaunch => {
            state.daemon_repair_prompt = true;
            state.status_message = "Confirm the shared-daemon repair launch".into();
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
        | UserIntent::ConfirmPackageContextLaunch
        | UserIntent::CancelPackageContextLaunch => Vec::new(),
    }
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
                    let packaged = matches!(
                        &info.target_kind,
                        crate::DesktopTargetKind::RegisteredPackage(_)
                    );
                    state.desktop_app = DesktopAppDiscovery::Found(Box::new(info));
                    state.status_message = match state.desktop_process {
                        DesktopProcessState::Running { .. } => "Desktop is already running".into(),
                        _ if packaged => {
                            "Registered Desktop: press P for the one-shot candidate; Enter blocks"
                                .into()
                        }
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
    }
    Vec::new()
}

/// Reports only the creation and identity facts observed by Guard.
fn launch_status_message(receipt: &LaunchReceipt) -> String {
    let base = match receipt.package_identity {
        crate::PackageIdentityObservation::Verified => format!(
            "Desktop created; package identity verified; proxy environment supplied (PID {})",
            receipt.pid
        ),
        crate::PackageIdentityObservation::NotApplicable => format!(
            "Desktop process created with the proxy environment (PID {})",
            receipt.pid
        ),
    };
    match receipt.daemon_preparation.status_detail() {
        "" => base,
        detail => format!("{base}; {detail}"),
    }
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
        // Y only acts while the prompt is shown; exactly one authorized launch.
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
                package_context_compat: false,
            })]
        );
        assert!(!state.daemon_repair_prompt);
    }

    #[test]
    fn package_context_candidate_requires_single_use_confirmation() {
        let mut state = state();
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::RequestPackageContextLaunch)
            )
            .is_empty()
        );
        assert!(state.package_context_prompt);
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::Launch)).is_empty());
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::CancelPackageContextLaunch)
            )
            .is_empty()
        );
        assert!(!state.package_context_prompt);
        assert!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::ConfirmPackageContextLaunch)
            )
            .is_empty()
        );
        reduce(
            &mut state,
            AppAction::Intent(UserIntent::RequestPackageContextLaunch),
        );
        assert_eq!(
            reduce(
                &mut state,
                AppAction::Intent(UserIntent::ConfirmPackageContextLaunch)
            ),
            vec![AppEffect::LaunchDesktop(LaunchOptions {
                refresh_codex_daemon: false,
                package_context_compat: true,
            })]
        );
        assert!(!state.package_context_prompt);
    }

    #[test]
    fn registered_package_repair_confirmation_includes_package_context_choice() {
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
                package_context_compat: true,
            })]
        );
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
            proxy_endpoint: "http://127.0.0.1:10808".into(),
            daemon_preparation: DaemonPreparation::Skipped,
            launch_method: crate::LaunchMethod::NativeProcess,
            package_identity: crate::PackageIdentityObservation::NotApplicable,
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
            proxy_endpoint: "http://127.0.0.1:10808".into(),
            daemon_preparation: DaemonPreparation::NotNeeded,
            launch_method: crate::LaunchMethod::NativeProcess,
            package_identity: crate::PackageIdentityObservation::NotApplicable,
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
    fn launch_status_reports_the_daemon_preparation_outcome() {
        let receipt = |preparation| LaunchReceipt {
            pid: 7,
            proxy_endpoint: "http://127.0.0.1:10808".into(),
            daemon_preparation: preparation,
            launch_method: crate::LaunchMethod::NativeProcess,
            package_identity: crate::PackageIdentityObservation::NotApplicable,
            desktop: DesktopLaunchInfo {
                product: DesktopProduct::ChatGpt,
                package_name: "OpenAI.Codex".into(),
                package_version: "1".into(),
                architecture: "X64".into(),
                discovery_source: DesktopDiscoverySource::AppxManifest,
            },
        };
        assert_eq!(
            launch_status_message(&receipt(DaemonPreparation::Skipped)),
            "Desktop process created with the proxy environment (PID 7)"
        );
        assert_eq!(
            launch_status_message(&receipt(DaemonPreparation::NotNeeded)),
            "Desktop process created with the proxy environment (PID 7); \
             the shared Codex background server was not running"
        );
        assert_eq!(
            launch_status_message(&receipt(DaemonPreparation::Stopped)),
            "Desktop process created with the proxy environment (PID 7); \
             the shared Codex background server was stopped before launch"
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
