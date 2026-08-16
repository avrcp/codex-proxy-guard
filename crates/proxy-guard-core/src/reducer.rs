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
            | UserIntent::Refresh
            | UserIntent::SyncSubscription
            | UserIntent::BenchmarkNodes
            | UserIntent::CancelBenchmark
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
    if state.error_message.is_some() {
        state.error_message = None;
        return Vec::new();
    }
    if state.foreground.is_some() {
        state.status_message = "An operation is already in progress".into();
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
        | UserIntent::ToggleHelp
        | UserIntent::Dismiss
        | UserIntent::Quit => unreachable!(),
    }
}

fn reduce_launch(state: &mut AppState) -> Vec<AppEffect> {
    if state.config.is_managed() {
        if let Some(selection) = &state.managed.selected {
            state.foreground = Some(ForegroundOperation::ManagedLaunch);
            state.status_message = "Launching Desktop through the managed proxy…".into();
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
            if let Ok(view) = managed {
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
}
