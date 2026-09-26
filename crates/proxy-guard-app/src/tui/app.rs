use anyhow::Context;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use proxy_guard_core::{
    AppAction, AppState, Capabilities, ProxyField, TaskResult, UserIntent, reduce,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::dispatcher::EffectDispatcher;

use super::{renderer, terminal::TerminalManager};

struct CancellationGuard(CancellationToken);

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub async fn run(mut state: AppState) -> anyhow::Result<()> {
    let mut terminal =
        TerminalManager::enter(state.config.tui.alternate_screen).context("enter terminal")?;
    let (task_tx, mut task_rx) = mpsc::channel::<TaskResult>(8);
    let cancellation = CancellationToken::new();
    let _cancellation_guard = CancellationGuard(cancellation.clone());
    let dispatcher = EffectDispatcher::new(task_tx, cancellation.clone());
    let capabilities = Capabilities::default();
    let mut events = EventStream::new();

    handle_action(
        &mut state,
        AppAction::Intent(UserIntent::Refresh),
        &capabilities,
        &dispatcher,
    );

    loop {
        terminal
            .terminal_mut()
            .draw(|frame| renderer::draw(frame, &state))
            .context("draw TUI")?;
        if state.should_quit {
            break;
        }

        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    handle_key(&mut state, key, &capabilities, &dispatcher);
                }
                Some(Ok(Event::Resize(_, _))) => {}
                Some(Err(error)) => {
                    state.error_message = Some(format!("TERMINAL_EVENT_FAILED: {error}"));
                }
                None => state.should_quit = true,
                _ => {}
            },
            Some(result) = task_rx.recv() => {
                handle_action(
                    &mut state,
                    AppAction::TaskComplete(Box::new(result)),
                    &capabilities,
                    &dispatcher,
                );
            }
            _ = cancellation.cancelled() => state.should_quit = true,
            result = tokio::signal::ctrl_c() => {
                if result.is_err() {
                    state.error_message = Some("CTRL_C_FAILED: signal handler failed".into());
                }
                handle_action(
                    &mut state,
                    AppAction::Intent(UserIntent::Quit),
                    &capabilities,
                    &dispatcher,
                );
            }
        }
    }
    // Cancel first, then wait (bounded) for the foreground task to finish its
    // cleanup — the launched Desktop and all external processes keep running.
    cancellation.cancel();
    dispatcher.shutdown().await;
    Ok(())
}

fn handle_key(
    state: &mut AppState,
    key: KeyEvent,
    capabilities: &Capabilities,
    dispatcher: &EffectDispatcher,
) {
    if let Some(intent) = key_to_intent(state, key) {
        handle_action(state, AppAction::Intent(intent), capabilities, dispatcher);
    }
}

fn key_to_intent(state: &AppState, key: KeyEvent) -> Option<UserIntent> {
    if matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(UserIntent::Quit);
    }
    if state.daemon_repair_prompt {
        // The repair confirmation is modal; only Y / N / Esc act on it.
        return match key.code {
            KeyCode::Char('y' | 'Y') => Some(UserIntent::ConfirmDaemonRepairLaunch),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(UserIntent::CancelDaemonRepairLaunch),
            _ => None,
        };
    }
    if state.package_context_prompt {
        return match key.code {
            KeyCode::Char('y' | 'Y') => Some(UserIntent::ConfirmPackageContextLaunch),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(UserIntent::CancelPackageContextLaunch),
            _ => None,
        };
    }
    if state.proxy_editor.is_some() {
        return proxy_editor_intent(state, key);
    }
    if matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
        return Some(UserIntent::Quit);
    }
    if state.show_help || state.error_message.is_some() {
        return match key.code {
            KeyCode::Char('c' | 'C') => Some(UserIntent::EditProxy),
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => Some(UserIntent::Dismiss),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Enter | KeyCode::Char('l' | 'L') => Some(UserIntent::Launch),
        KeyCode::Char('p' | 'P') => Some(UserIntent::RequestPackageContextLaunch),
        KeyCode::Char('d' | 'D') => Some(UserIntent::RequestDaemonRepairLaunch),
        KeyCode::Char('r' | 'R') => Some(UserIntent::Refresh),
        KeyCode::Char('c' | 'C') => Some(UserIntent::EditProxy),
        KeyCode::Char('?') => Some(UserIntent::ToggleHelp),
        KeyCode::Esc => Some(UserIntent::Dismiss),
        _ => None,
    }
}

fn proxy_editor_intent(state: &AppState, key: KeyEvent) -> Option<UserIntent> {
    let editor = state.proxy_editor.as_ref()?;
    match key.code {
        KeyCode::Esc => Some(UserIntent::CancelProxyEdit),
        KeyCode::Enter => Some(UserIntent::SaveProxy),
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
            Some(UserIntent::ToggleProxyField)
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UserIntent::UpdateProxyField {
                field: editor.active_field,
                value: String::new(),
            })
        }
        KeyCode::Backspace => {
            let mut value = match editor.active_field {
                ProxyField::Host => editor.host.clone(),
                ProxyField::Port => editor.port.clone(),
            };
            value.pop();
            Some(UserIntent::UpdateProxyField {
                field: editor.active_field,
                value,
            })
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let mut value = match editor.active_field {
                ProxyField::Host => editor.host.clone(),
                ProxyField::Port => editor.port.clone(),
            };
            value.push(character);
            Some(UserIntent::UpdateProxyField {
                field: editor.active_field,
                value,
            })
        }
        _ => None,
    }
}

fn handle_action(
    state: &mut AppState,
    action: AppAction,
    capabilities: &Capabilities,
    dispatcher: &EffectDispatcher,
) {
    let mut candidate = state.clone();
    let effects = reduce(&mut candidate, action);
    if let Some(error) = effects
        .iter()
        .find_map(|effect| capabilities.authorize(effect).err())
    {
        state.error_message = Some(error);
        return;
    }
    *state = candidate;
    for effect in effects {
        dispatcher.dispatch(effect, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxy_guard_core::GuardConfig;

    #[test]
    fn proxy_editor_maps_typing_and_save_keys_to_intents() {
        let state = AppState::new(GuardConfig::default(), "config.toml".into());
        assert!(proxy_editor_intent(&state, KeyEvent::from(KeyCode::Enter)).is_none());
        let mut editing = state;
        editing.proxy_editor = Some(proxy_guard_core::ProxyEditor {
            host: "127.0.0.1".into(),
            port: "10808".into(),
            active_field: ProxyField::Port,
            error: None,
        });
        assert!(matches!(
            proxy_editor_intent(&editing, KeyEvent::from(KeyCode::Enter)),
            Some(UserIntent::SaveProxy)
        ));
        assert!(matches!(
            proxy_editor_intent(&editing, KeyEvent::from(KeyCode::Char('9'))),
            Some(UserIntent::UpdateProxyField { value, .. }) if value == "108089"
        ));
    }

    #[test]
    fn daemon_repair_confirmation_is_modal_and_single_shot() {
        let state = AppState::new(GuardConfig::default(), "config.toml".into());
        let mut prompting = state;
        prompting.daemon_repair_prompt = true;
        // While the prompt shows, only Y / N / Esc act; Enter and D do nothing.
        assert_eq!(handle_intent_key(&prompting, KeyCode::Enter), None);
        assert_eq!(handle_intent_key(&prompting, KeyCode::Char('d')), None);
        assert_eq!(
            handle_intent_key(&prompting, KeyCode::Char('y')),
            Some(UserIntent::ConfirmDaemonRepairLaunch)
        );
        assert_eq!(
            handle_intent_key(&prompting, KeyCode::Esc),
            Some(UserIntent::CancelDaemonRepairLaunch)
        );
        // Outside the prompt, D requests the repair confirmation.
        let mut normal = AppState::new(GuardConfig::default(), "config.toml".into());
        normal.daemon_repair_prompt = false;
        assert_eq!(
            handle_intent_key(&normal, KeyCode::Char('d')),
            Some(UserIntent::RequestDaemonRepairLaunch)
        );
        assert_eq!(
            handle_intent_key(&normal, KeyCode::Enter),
            Some(UserIntent::Launch)
        );
    }

    #[test]
    fn package_context_confirmation_is_modal() {
        let mut state = AppState::new(GuardConfig::default(), "config.toml".into());
        assert_eq!(
            handle_intent_key(&state, KeyCode::Char('p')),
            Some(UserIntent::RequestPackageContextLaunch)
        );
        state.package_context_prompt = true;
        assert_eq!(handle_intent_key(&state, KeyCode::Enter), None);
        assert_eq!(
            handle_intent_key(&state, KeyCode::Char('y')),
            Some(UserIntent::ConfirmPackageContextLaunch)
        );
        assert_eq!(
            handle_intent_key(&state, KeyCode::Esc),
            Some(UserIntent::CancelPackageContextLaunch)
        );
    }

    fn handle_intent_key(state: &AppState, code: KeyCode) -> Option<UserIntent> {
        key_to_intent(state, KeyEvent::from(code))
    }
}
