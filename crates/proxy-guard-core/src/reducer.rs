use crate::{
    AppAction, AppEffect, AppState, DesktopAppDiscovery, DesktopProcessState, ForegroundOperation,
    LaunchState, ProxyEditor, ProxyField, SubscriptionId, TaskResult, UserIntent, redact_text,
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
    if intent == UserIntent::CancelBenchmark {
        if state.foreground == Some(ForegroundOperation::Benchmark) {
            state.status_message = "Cancelling benchmark…".into();
            return vec![AppEffect::CancelBenchmark];
        }
        return Vec::new();
    }
    if intent == UserIntent::EditProxy {
        if state.foreground.is_none() && !state.manager.active {
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
            | UserIntent::Refresh
            | UserIntent::SyncSubscription
            | UserIntent::BenchmarkNodes
            | UserIntent::CancelBenchmark
            | UserIntent::ToggleManager
            | UserIntent::ReopenManager
            | UserIntent::ToggleHelp
            | UserIntent::Dismiss
            | UserIntent::Quit
            | UserIntent::EditProxy => {}
        }
        return Vec::new();
    }
    if intent == UserIntent::ToggleManager {
        if state.manager.active {
            state.manager.active = false;
            state.manager.display_url = None;
            state.status_message = "Closing the browser manager…".into();
            return vec![AppEffect::CloseManager];
        }
        if state.foreground.is_none()
            && !matches!(state.desktop_process, DesktopProcessState::Running { .. })
            && state.managed.proxy_endpoint.is_none()
        {
            state.manager.active = true;
            state.status_message = "Opening the browser manager…".into();
            return vec![AppEffect::OpenManager];
        }
        state.status_message = "Close Desktop and wait for the current operation first".into();
        state.error_message = Some(
            "MANAGER_BUSY: quit the running Desktop or finish the operation, then open the Manager"
                .into(),
        );
        return Vec::new();
    }
    if intent == UserIntent::ReopenManager {
        if state.manager.active {
            return vec![AppEffect::ReopenManager];
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
    if state.error_message.is_some() {
        state.error_message = None;
        return Vec::new();
    }
    if state.foreground.is_some() {
        state.status_message = "An operation is already in progress".into();
        return Vec::new();
    }
    if state.manager.active {
        state.status_message = "Close the browser manager before using runtime controls".into();
        state.error_message =
            Some("MANAGER_ACTIVE: close the Local Web Manager, then retry".into());
        return Vec::new();
    }

    match intent {
        UserIntent::Launch => reduce_launch(state),
        UserIntent::Refresh => {
            state.foreground = Some(ForegroundOperation::Refresh);
            state.desktop_app = DesktopAppDiscovery::Searching;
            state.status_message = "Refreshing status…".into();
            vec![AppEffect::RefreshLocalState]
        }
        UserIntent::SyncSubscription => {
            let Some(id) = configured_subscription_id(state) else {
                return Vec::new();
            };
            state.foreground = Some(ForegroundOperation::SyncSubscription);
            state.status_message = "Syncing subscription…".into();
            vec![AppEffect::SyncSubscription(id)]
        }
        UserIntent::BenchmarkNodes => {
            state.foreground = Some(ForegroundOperation::Benchmark);
            state.status_message = "Benchmarking nodes (JP > SG > US)…".into();
            vec![AppEffect::BenchmarkNodes]
        }
        UserIntent::EditProxy
        | UserIntent::UpdateProxyField { .. }
        | UserIntent::ToggleProxyField
        | UserIntent::SaveProxy
        | UserIntent::CancelProxyEdit
        | UserIntent::CancelBenchmark
        | UserIntent::ToggleManager
        | UserIntent::ReopenManager
        | UserIntent::ToggleHelp
        | UserIntent::Dismiss
        | UserIntent::Quit => unreachable!(),
    }
}

fn reduce_launch(state: &mut AppState) -> Vec<AppEffect> {
    if state.config.is_managed() {
        let selection = state
            .managed
            .manual_selection
            .as_ref()
            .or(state.managed.selected.as_ref());
        if let Some(selection) = selection {
            state.foreground = Some(ForegroundOperation::ManagedLaunch);
            state.status_message = if state.managed.manual_selection.is_some() {
                "Launching Desktop through the manually selected node…".into()
            } else {
                "Launching Desktop through the managed proxy…".into()
            };
            return vec![AppEffect::LaunchManaged(selection.node_id)];
        }
        state.status_message = "No healthy managed node is selected".into();
        state.error_message =
            Some("NoHealthyManagedNode: run a benchmark first, then relaunch".to_string());
        return Vec::new();
    }
    state.foreground = Some(ForegroundOperation::Launch);
    state.launch = LaunchState::Launching;
    state.status_message = "Launching Desktop with proxy environment…".into();
    vec![AppEffect::LaunchDesktop]
}

fn configured_subscription_id(state: &mut AppState) -> Option<SubscriptionId> {
    match state
        .config
        .managed
        .subscription_id
        .parse::<SubscriptionId>()
    {
        Ok(id) => Some(id),
        Err(_) => {
            state.error_message = Some(
                "SUBSCRIPTION_MISSING: set managed.subscription_id or add a subscription first"
                    .into(),
            );
            None
        }
    }
}

fn reduce_result(state: &mut AppState, result: TaskResult) -> Vec<AppEffect> {
    match result {
        TaskResult::LocalStateRefreshed {
            desktop_app,
            process,
            managed,
        } => {
            if state.foreground != Some(ForegroundOperation::Refresh) {
                return Vec::new();
            }
            state.foreground = None;
            state.desktop_process = process;
            match desktop_app {
                Ok(info) => {
                    state.desktop_app = DesktopAppDiscovery::Found(info);
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
            if let Ok(mut view) = managed {
                view.manual_selection = state.managed.manual_selection.clone();
                state.managed = view;
            }
        }
        TaskResult::LaunchCompleted(result) => {
            if state.foreground != Some(ForegroundOperation::Launch) {
                return Vec::new();
            }
            state.foreground = None;
            match result {
                Ok((info, receipt)) => {
                    state.desktop_app = DesktopAppDiscovery::Found(info);
                    state.desktop_process = DesktopProcessState::Running { pid: receipt.pid };
                    state.status_message = format!("Desktop launched (PID {})", receipt.pid);
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
        TaskResult::SubscriptionSynced(result) => {
            if state.foreground != Some(ForegroundOperation::SyncSubscription) {
                return Vec::new();
            }
            state.foreground = None;
            match result {
                Ok(summary) => {
                    state.status_message = format!(
                        "Subscription synced: {} imported, {} updated, {} ignored, {} stale",
                        summary.imported, summary.updated, summary.ignored_region, summary.stale
                    );
                }
                Err(message) => {
                    state.error_message = Some(redact_text(&message));
                    state.status_message = "Subscription sync failed".into();
                }
            }
        }
        TaskResult::BenchmarkCompleted(result) => {
            if state.foreground != Some(ForegroundOperation::Benchmark) {
                return Vec::new();
            }
            state.foreground = None;
            match result {
                Ok(summary) => {
                    state.managed.regions = summary.regions;
                    state.managed.selected = summary.selected.clone();
                    state.managed.proxy_lost = false;
                    if let Some(manual) = &state.managed.manual_selection
                        && !summary.healthy_ids.contains(&manual.node_id)
                    {
                        state.managed.manual_selection = None;
                    }
                    state.status_message = match &summary.selected {
                        Some(selection) => format!(
                            "Selected {} ({}) score {}",
                            selection.name, selection.region, selection.score
                        ),
                        None => {
                            "No healthy managed node: benchmark found no JP/SG/US candidate".into()
                        }
                    };
                    if summary.selected.is_none() {
                        state.error_message = Some(
                            "NoHealthyManagedNode: no JP/SG/US node passed the health gates".into(),
                        );
                    }
                }
                Err(message) => {
                    state.error_message = Some(redact_text(&message));
                    state.status_message = "Benchmark failed".into();
                }
            }
        }
        TaskResult::ManagedLaunchCompleted(result) => {
            if state.foreground != Some(ForegroundOperation::ManagedLaunch) {
                return Vec::new();
            }
            state.foreground = None;
            match result {
                Ok(receipt) => {
                    state.desktop_process = DesktopProcessState::Running { pid: receipt.pid };
                    state.managed.proxy_endpoint = Some(receipt.proxy_endpoint.clone());
                    state.managed.proxy_lost = false;
                    state.status_message = format!(
                        "Desktop launched through managed node {} (PID {})",
                        receipt.node.name, receipt.pid
                    );
                }
                Err(message) => {
                    let message = redact_text(&message);
                    state.managed.selected = None;
                    state.managed.proxy_endpoint = None;
                    state.status_message = "Managed launch was blocked".into();
                    state.error_message = Some(message);
                }
            }
        }
        TaskResult::ManagedProxyStopped(_) => {
            state.managed.proxy_endpoint = None;
            state.managed.proxy_lost = false;
        }
        TaskResult::ManagedProxyLost(reason) => {
            state.managed.proxy_endpoint = None;
            state.managed.proxy_lost = true;
            if state.foreground.is_none() {
                state.status_message = "Managed proxy lost".into();
                state.error_message = Some(redact_text(&reason));
            }
        }
        TaskResult::ManagerOpened(result) => match result {
            Ok(info) => {
                state.manager.active = true;
                state.manager.display_url = Some(info.display_url.clone());
                state.error_message = None;
                state.status_message = format!("Browser Manager open at {}", info.display_url);
            }
            Err(message) => {
                state.manager.active = false;
                state.manager.display_url = None;
                state.status_message = "Browser Manager could not be opened".into();
                state.error_message = Some(redact_text(&message));
            }
        },
        TaskResult::ManagerClosed => {
            state.manager.active = false;
            state.manager.display_url = None;
            state.foreground = None;
            state.status_message = "Browser Manager closed".into();
            return vec![AppEffect::RefreshLocalState];
        }
        TaskResult::ManagerConfigUpdated(result) => match result {
            Ok(config) => {
                let subscription_changed =
                    state.config.managed.subscription_id != config.managed.subscription_id;
                state.config = config;
                if subscription_changed {
                    state.managed.manual_selection = None;
                }
                state.status_message = "Configuration updated by the Browser Manager".into();
            }
            Err(message) => {
                state.status_message = "Browser Manager configuration update failed".into();
                state.error_message = Some(redact_text(&message));
            }
        },
        TaskResult::ManagerManagedViewUpdated(result) => match result {
            Ok(mut view) => {
                view.manual_selection = state.managed.manual_selection.clone();
                state.managed = view;
            }
            Err(message) => {
                state.status_message = "Browser Manager state refresh failed".into();
                state.error_message = Some(redact_text(&message));
            }
        },
        TaskResult::ManagerSelectionChanged(result) => match result {
            Ok(selection) => {
                state.managed.manual_selection = selection.clone();
                state.status_message = match selection {
                    Some(selection) => format!(
                        "Manual override {} ({}) set for next launch",
                        selection.name, selection.region
                    ),
                    None => "Manual override cleared; using AUTO JP > SG > US".into(),
                };
            }
            Err(message) => {
                state.status_message = "Manual selection was not accepted".into();
                state.error_message = Some(redact_text(&message));
            }
        },
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CodexRegion, DesktopDiscoverySource, DesktopLaunchInfo, DesktopProduct, GuardConfig,
        LaunchReceipt, NodeSelection, ProxyMode, RegionBenchmarkCounts,
    };
    use std::path::PathBuf;

    fn state() -> AppState {
        AppState::new(GuardConfig::default(), PathBuf::from("config.toml"))
    }

    fn managed_state() -> AppState {
        let mut config = GuardConfig::default();
        config.proxy.mode = ProxyMode::Managed;
        config.managed.subscription_id = "11111111-1111-1111-1111-111111111111".into();
        AppState::new(config, PathBuf::from("config.toml"))
    }

    fn selection() -> NodeSelection {
        NodeSelection {
            node_id: crate::NodeId::new(),
            name: "JP Tokyo 01".into(),
            region: CodexRegion::JP,
            score: 93,
            success_percent: 100,
            median_ms: 84,
            p95_ms: 121,
            exit_stable: true,
        }
    }

    #[test]
    fn only_one_foreground_operation_is_allowed() {
        let mut state = state();
        assert_eq!(
            reduce(&mut state, AppAction::Intent(UserIntent::Launch)),
            vec![AppEffect::LaunchDesktop]
        );
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::Refresh)).is_empty());
        assert_eq!(state.foreground, Some(ForegroundOperation::Launch));
    }

    #[test]
    fn managed_launch_requires_a_selected_node() {
        let mut state = managed_state();
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::Launch)).is_empty());
        assert!(
            state
                .error_message
                .as_deref()
                .is_some_and(|e| e.contains("NoHealthyManagedNode"))
        );

        state.error_message = None;
        state.managed.selected = Some(selection());
        assert_eq!(
            reduce(&mut state, AppAction::Intent(UserIntent::Launch)),
            vec![AppEffect::LaunchManaged(
                state.managed.selected.unwrap().node_id
            )]
        );
        assert_eq!(state.foreground, Some(ForegroundOperation::ManagedLaunch));
    }

    #[test]
    fn sync_and_benchmark_map_to_their_effects() {
        let mut state = managed_state();
        let effects = reduce(&mut state, AppAction::Intent(UserIntent::SyncSubscription));
        assert_eq!(effects.len(), 1);
        assert!(matches!(effects[0], AppEffect::SyncSubscription(_)));

        let mut state = managed_state();
        assert_eq!(
            reduce(&mut state, AppAction::Intent(UserIntent::BenchmarkNodes)),
            vec![AppEffect::BenchmarkNodes]
        );
    }

    #[test]
    fn benchmark_result_selects_the_winner() {
        let mut state = managed_state();
        reduce(&mut state, AppAction::Intent(UserIntent::BenchmarkNodes));
        let summary = crate::BenchmarkRunSummary {
            scanned: 10,
            quick_rejected: 2,
            deep_scanned: 8,
            healthy: 6,
            regions: RegionBenchmarkCounts {
                jp_active: 4,
                jp_healthy: 3,
                sg_active: 3,
                sg_healthy: 2,
                us_active: 3,
                us_healthy: 1,
            },
            selected: Some(selection()),
            healthy_ids: Vec::new(),
        };
        assert!(
            reduce(
                &mut state,
                AppAction::TaskComplete(Box::new(TaskResult::BenchmarkCompleted(Ok(
                    summary.clone()
                ))))
            )
            .is_empty()
        );
        assert_eq!(state.managed.regions, summary.regions);
        assert!(state.managed.selected.is_some());
    }

    #[test]
    fn failed_managed_launch_clears_the_cached_ui_selection() {
        let mut state = managed_state();
        state.managed.selected = Some(selection());
        reduce(&mut state, AppAction::Intent(UserIntent::Launch));

        reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::ManagedLaunchCompleted(Err(
                "launch-time verification failed".into(),
            )))),
        );

        assert!(state.managed.selected.is_none());
        assert!(state.managed.proxy_endpoint.is_none());
        assert_eq!(state.status_message, "Managed launch was blocked");
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
            install_location: PathBuf::from("app"),
            executable: PathBuf::from("app/Codex.exe"),
        };
        let receipt = LaunchReceipt {
            pid: 42,
            proxy_endpoint: "http://127.0.0.1:10808".into(),
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

    #[test]
    fn toggle_manager_opens_only_when_the_runtime_is_idle() {
        let mut state = managed_state();
        assert_eq!(
            reduce(&mut state, AppAction::Intent(UserIntent::ToggleManager)),
            vec![AppEffect::OpenManager]
        );
        assert!(state.manager.active);

        reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::ManagerOpened(Ok(
                crate::ManagerInfo {
                    display_url: "http://127.0.0.1:43210".into(),
                },
            )))),
        );
        assert_eq!(
            state.manager.display_url.as_deref(),
            Some("http://127.0.0.1:43210")
        );
    }

    #[test]
    fn manager_is_rejected_while_desktop_or_an_operation_runs() {
        let mut running = managed_state();
        running.desktop_process = DesktopProcessState::Running { pid: 9 };
        assert!(reduce(&mut running, AppAction::Intent(UserIntent::ToggleManager)).is_empty());
        assert!(!running.manager.active);
        assert!(
            running
                .error_message
                .as_deref()
                .is_some_and(|message| message.contains("MANAGER_BUSY"))
        );

        let mut operating = managed_state();
        reduce(
            &mut operating,
            AppAction::Intent(UserIntent::BenchmarkNodes),
        );
        assert!(reduce(&mut operating, AppAction::Intent(UserIntent::ToggleManager)).is_empty());
        assert!(!operating.manager.active);
    }

    #[test]
    fn manager_active_locks_runtime_mutations() {
        let mut state = managed_state();
        reduce(&mut state, AppAction::Intent(UserIntent::ToggleManager));
        state.managed.selected = Some(selection());

        for intent in [
            UserIntent::Launch,
            UserIntent::SyncSubscription,
            UserIntent::BenchmarkNodes,
            UserIntent::EditProxy,
        ] {
            let mut candidate = state.clone();
            let effects = reduce(&mut candidate, AppAction::Intent(intent.clone()));
            assert!(effects.is_empty(), "no effect for {intent:?}");
            assert!(candidate.foreground.is_none());
        }
    }

    #[test]
    fn toggle_manager_while_active_closes_it() {
        let mut state = managed_state();
        reduce(&mut state, AppAction::Intent(UserIntent::ToggleManager));
        assert_eq!(
            reduce(&mut state, AppAction::Intent(UserIntent::ToggleManager)),
            vec![AppEffect::CloseManager]
        );
        assert!(!state.manager.active);
    }

    #[test]
    fn manager_close_refreshes_local_state() {
        let mut state = managed_state();
        state.manager.active = true;
        let effects = reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::ManagerClosed)),
        );
        assert_eq!(effects, vec![AppEffect::RefreshLocalState]);
        assert!(!state.manager.active);
    }

    #[test]
    fn config_update_clears_manual_override_on_subscription_change() {
        let mut state = managed_state();
        state.managed.manual_selection = Some(selection());
        let mut updated = state.config.clone();
        updated.managed.subscription_id = "22222222-2222-2222-2222-222222222222".into();
        reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::ManagerConfigUpdated(Ok(
                updated.clone()
            )))),
        );
        assert_eq!(
            state.config.managed.subscription_id,
            updated.managed.subscription_id
        );
        assert!(state.managed.manual_selection.is_none());
    }

    #[test]
    fn manual_selection_syncs_into_managed_view_and_launch_prefers_it() {
        let mut state = managed_state();
        let manual = selection();
        reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::ManagerSelectionChanged(Ok(Some(
                manual.clone(),
            ))))),
        );
        assert_eq!(state.managed.manual_selection, Some(manual.clone()));
        assert_eq!(
            reduce(&mut state, AppAction::Intent(UserIntent::Launch)),
            vec![AppEffect::LaunchManaged(manual.node_id)]
        );
    }

    #[test]
    fn fresh_benchmark_invalidates_a_rejected_manual_override() {
        let mut state = managed_state();
        let manual = selection();
        state.managed.manual_selection = Some(manual.clone());
        reduce(&mut state, AppAction::Intent(UserIntent::BenchmarkNodes));
        let summary = crate::BenchmarkRunSummary {
            scanned: 3,
            quick_rejected: 1,
            deep_scanned: 2,
            healthy: 1,
            regions: RegionBenchmarkCounts {
                jp_active: 3,
                jp_healthy: 1,
                sg_active: 0,
                sg_healthy: 0,
                us_active: 0,
                us_healthy: 0,
            },
            selected: Some(selection()),
            healthy_ids: vec![crate::NodeId::new()],
        };
        reduce(
            &mut state,
            AppAction::TaskComplete(Box::new(TaskResult::BenchmarkCompleted(Ok(summary)))),
        );
        assert!(state.managed.manual_selection.is_none());
    }
}
