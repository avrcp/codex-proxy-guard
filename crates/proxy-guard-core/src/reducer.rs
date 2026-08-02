use crate::{
    AppAction, AppEffect, AppState, DesktopAppDiscovery, DesktopProcessState, ForegroundOperation,
    LaunchState, ProxyEditor, ProxyField, TaskResult, UserIntent, redact_text,
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
        state.status_message = "A launch operation is already in progress".into();
        return Vec::new();
    }

    match intent {
        UserIntent::Launch => {
            state.foreground = Some(ForegroundOperation::Launch);
            state.launch = LaunchState::Launching;
            state.status_message = "Launching Desktop with proxy environment…".into();
            vec![AppEffect::LaunchDesktop]
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
    }
}

fn reduce_result(state: &mut AppState, result: TaskResult) -> Vec<AppEffect> {
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
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DesktopDiscoverySource, DesktopLaunchInfo, DesktopProduct, GuardConfig, LaunchReceipt,
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
            vec![AppEffect::LaunchDesktop]
        );
        assert!(reduce(&mut state, AppAction::Intent(UserIntent::Refresh)).is_empty());
        assert_eq!(state.foreground, Some(ForegroundOperation::Launch));
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
