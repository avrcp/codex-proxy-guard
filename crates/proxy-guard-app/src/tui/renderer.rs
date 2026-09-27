use proxy_guard_core::{
    AppState, ConfigReadiness, DesktopAppDiscovery, DesktopProcessState, DesktopTargetKind,
    LaunchState, ProxyEditor, ProxyField,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
};

use super::theme;

pub fn draw(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();
    if area.width < 52 || area.height < 18 {
        draw_too_small(frame, area);
        return;
    }
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::muted())
        .padding(Padding::horizontal(2));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(inner);
    draw_header(frame, sections[0], state);
    frame.render_widget(
        Paragraph::new("─".repeat(sections[1].width as usize)).style(theme::muted()),
        sections[1],
    );
    if state.show_help {
        draw_help(frame, sections[2]);
    } else {
        draw_content(frame, sections[2], state);
    }
    draw_footer(frame, sections[3], state);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let status = if state.foreground.is_some() {
        Span::styled("WORKING", theme::warning())
    } else if state.proxy_editor.is_some() {
        Span::styled("CONFIGURE", theme::accent())
    } else if state.error_message.is_some() {
        Span::styled("BLOCKED", theme::error())
    } else if matches!(state.desktop_process, DesktopProcessState::Running { .. }) {
        Span::styled("RUNNING", theme::success())
    } else {
        Span::styled("READY", theme::accent())
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("CHATGPT DESKTOP PROXY GUARD", theme::title()),
                Span::raw("  "),
                status,
            ]),
            Line::styled(
                "Process-scoped launcher for Chat, Work, and Codex",
                theme::muted(),
            ),
        ]),
        area,
    );
}

fn draw_content(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut lines = Vec::new();
    if let Some(editor) = &state.proxy_editor {
        draw_proxy_editor(&mut lines, editor.clone(), state.foreground.is_some());
    } else if state.daemon_repair_prompt {
        draw_daemon_repair_prompt(&mut lines);
    } else if state.backend_proxy_prompt {
        draw_backend_proxy_prompt(&mut lines, state);
    } else if let Some(error) = &state.error_message {
        lines.push(Line::styled("Launch unavailable", theme::error()));
        lines.push(Line::raw(error.clone()));
        lines.push(Line::raw(""));
        let hint = if state.config_readiness == ConfigReadiness::RepairRequired {
            "Press C to rebuild the proxy configuration, or Enter/Esc to dismiss."
        } else {
            "Enter/Esc to dismiss; the message above names the matching cause."
        };
        lines.push(Line::styled(hint, theme::muted()));
    } else {
        draw_status(&mut lines, state);
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_status(lines: &mut Vec<Line<'static>>, state: &AppState) {
    let registered = matches!(
        &state.desktop_app,
        DesktopAppDiscovery::Found(info) if matches!(info.target_kind, DesktopTargetKind::RegisteredPackage(_))
    );
    lines.push(key_value(
        "Proxy",
        state.config.proxy_url(),
        theme::accent(),
    ));
    lines.push(key_value(
        "Backend",
        if registered {
            "Windows registered app activation".into()
        } else {
            "process environment injection".into()
        },
        Style::default(),
    ));
    lines.push(key_value("App", desktop_label(state), Style::default()));
    lines.push(key_value("Entry", entry_label(state), theme::muted()));
    lines.push(key_value(
        "Process",
        process_label(state),
        process_style(state),
    ));
    lines.push(key_value(
        "Home cfg",
        backend_config_label(state),
        backend_config_style(state),
    ));
    if let DesktopAppDiscovery::NotFound(message) = &state.desktop_app {
        lines.push(Line::styled(message.clone(), theme::error()));
        lines.push(Line::styled(
            "Install: https://chatgpt.com/download/ (Store ID 9PLM9XGG6VKS)",
            theme::muted(),
        ));
    }
    lines.push(Line::raw(""));
    if registered {
        lines.push(Line::from(vec![
            Span::styled("Delivers  ", theme::muted()),
            Span::raw("Chromium proxy arguments on activation"),
        ]));
        match backend_config_state(state) {
            BackendConfigState::Authorized => lines.push(Line::styled(
                "Plus the authorized HTTP(S)_PROXY/NO_PROXY block in the confirmed Codex Home",
                theme::muted(),
            )),
            BackendConfigState::NotAuthorized => lines.push(Line::styled(
                "Backend Codex traffic is NOT proxied yet — press B to authorize the home .env",
                theme::warning(),
            )),
        }
    } else {
        lines.push(Line::from(vec![
            Span::styled("Injects  ", theme::muted()),
            Span::raw("HTTP_PROXY · HTTPS_PROXY · NO_PROXY; removes ALL_PROXY"),
        ]));
        lines.push(Line::styled(
            "This sets process environment only; it does not enforce all traffic.",
            theme::muted(),
        ));
    }
    lines.push(Line::styled(
        "Press D for a shared-daemon repair launch (asks first).",
        theme::muted(),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::styled(primary_action(state), theme::accent()));
    lines.push(Line::styled(
        "Press C to change the proxy host or port.",
        theme::muted(),
    ));
}

enum BackendConfigState {
    Authorized,
    NotAuthorized,
}

fn backend_config_state(state: &AppState) -> BackendConfigState {
    if state.config.codex.manage_codex_proxy_env {
        BackendConfigState::Authorized
    } else {
        BackendConfigState::NotAuthorized
    }
}

fn backend_config_label(state: &AppState) -> String {
    if !matches!(
        &state.desktop_app,
        DesktopAppDiscovery::Found(info) if matches!(info.target_kind, DesktopTargetKind::RegisteredPackage(_))
    ) {
        return "—".into();
    }
    match backend_config_state(state) {
        BackendConfigState::Authorized => format!(
            "authorized ({})",
            proxy_guard_core::display_path(&state.config.codex.proxy_env_home)
        ),
        BackendConfigState::NotAuthorized => "not authorized (press B)".into(),
    }
}

fn backend_config_style(state: &AppState) -> Style {
    match backend_config_state(state) {
        BackendConfigState::Authorized => theme::success(),
        BackendConfigState::NotAuthorized => theme::warning(),
    }
}

fn draw_daemon_repair_prompt(lines: &mut Vec<Line<'static>>) {
    lines.push(Line::styled("Repair launch", theme::title()));
    lines.push(Line::raw(""));
    lines.push(Line::raw(
        "Stop the shared Codex background server, then launch?",
    ));
    lines.push(Line::raw(
        "This may interrupt tasks of other CLI / IDE / remote clients",
    ));
    lines.push(Line::raw("sharing the same Codex Home."));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Y  Confirm and launch      N / Esc  Cancel",
        theme::accent(),
    ));
}

fn draw_backend_proxy_prompt(lines: &mut Vec<Line<'static>>, state: &AppState) {
    let enabled = state.config.codex.manage_codex_proxy_env;
    let bound = &state.config.codex.proxy_env_home;
    let home = if bound.as_os_str().is_empty() {
        proxy_guard_core::GuardConfig::default_codex_home()
            .map(|home| proxy_guard_core::display_path(&home))
            .unwrap_or_else(|| "<cannot resolve>".into())
    } else {
        proxy_guard_core::display_path(bound)
    };
    lines.push(Line::styled(
        if enabled {
            "Revoke backend proxy consent"
        } else {
            "Authorize backend proxy consent"
        },
        theme::title(),
    ));
    lines.push(Line::raw(""));
    if enabled {
        lines.push(Line::raw(
            "Stop managing HTTP_PROXY / HTTPS_PROXY / NO_PROXY in this",
        ));
        lines.push(Line::raw("Codex Home .env?"));
        lines.push(Line::raw(""));
        lines.push(Line::styled(home, theme::accent()));
        lines.push(Line::raw(""));
        lines.push(Line::raw(
            "Guard removes only its own unmodified block; the rest of the",
        ));
        lines.push(Line::raw("file is left as-is."));
    } else {
        lines.push(Line::raw(
            "Allow Guard to manage HTTP_PROXY / HTTPS_PROXY /",
        ));
        lines.push(Line::raw("NO_PROXY inside this Codex Home .env?"));
        lines.push(Line::raw(""));
        lines.push(Line::styled(home, theme::accent()));
        lines.push(Line::raw(""));
        lines.push(Line::raw(
            "It affects later Codex processes using the same Home, not just",
        ));
        lines.push(Line::raw("this Desktop. No system proxy, auth, or model"));
        lines.push(Line::raw(
            "changes. The block is prepared by the next launch",
        ));
        lines.push(Line::raw("and removed when you revoke."));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Y  Confirm      N / Esc  Cancel",
        theme::accent(),
    ));
}

fn draw_proxy_editor(lines: &mut Vec<Line<'static>>, editor: ProxyEditor, saving: bool) {
    lines.push(Line::styled("Proxy configuration", theme::title()));
    lines.push(Line::styled(
        "Use the local HTTP/Mixed endpoint from your proxy app.",
        theme::muted(),
    ));
    lines.push(Line::raw(""));
    lines.push(editor_field(
        "Host",
        editor.host,
        editor.active_field == ProxyField::Host,
    ));
    lines.push(editor_field(
        "Port",
        editor.port,
        editor.active_field == ProxyField::Port,
    ));
    if let Some(error) = &editor.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(error.clone(), theme::error()));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        if saving {
            "Saving configuration…"
        } else {
            "Ctrl-U clear · Tab / Up/Down switch field · Enter save · Esc cancel"
        },
        if saving {
            theme::warning()
        } else {
            theme::accent()
        },
    ));
}

fn editor_field(label: &str, value: String, selected: bool) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let value = if selected { format!("{value}|") } else { value };
    Line::from(vec![
        Span::styled(
            format!("{marker} {label:<6}"),
            if selected {
                theme::accent()
            } else {
                theme::muted()
            },
        ),
        Span::styled(
            value,
            if selected {
                theme::title()
            } else {
                Style::default()
            },
        ),
    ])
}

fn draw_help(frame: &mut Frame<'_>, area: Rect) {
    let lines = vec![
        Line::styled("Keyboard", theme::title()),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Enter / L  ", theme::accent()),
            Span::raw(
                "Launch ChatGPT Desktop through the configured proxy (registered apps activate natively)",
            ),
        ]),
        Line::from(vec![
            Span::styled("D          ", theme::accent()),
            Span::raw(
                "Repair launch: stop the shared Codex background server first (asks for confirmation)",
            ),
        ]),
        Line::from(vec![
            Span::styled("B          ", theme::accent()),
            Span::raw("Authorize or revoke the Codex Home .env proxy block (asks first)"),
        ]),
        Line::from(vec![
            Span::styled("R          ", theme::accent()),
            Span::raw("Refresh ChatGPT Desktop discovery and running state"),
        ]),
        Line::from(vec![
            Span::styled("C          ", theme::accent()),
            Span::raw("Edit the proxy host and HTTP/Mixed port"),
        ]),
        Line::from(vec![
            Span::styled("?          ", theme::accent()),
            Span::raw("Close this help"),
        ]),
        Line::from(vec![
            Span::styled("Q / Ctrl-C ", theme::accent()),
            Span::raw("Quit Guard; ChatGPT Desktop keeps running"),
        ]),
        Line::raw(""),
        Line::styled(
            format!(
                "Build {} · commit {}{}",
                crate::build_info::VERSION,
                crate::build_info::COMMIT,
                if crate::build_info::dirty() {
                    " (dirty)"
                } else {
                    ""
                }
            ),
            theme::muted(),
        ),
        Line::styled(
            "Run `codex-proxy-guard build-info` for the exact executable and provenance.",
            theme::muted(),
        ),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let shortcuts = if state.daemon_repair_prompt || state.backend_proxy_prompt {
        "Y  Confirm     N / Esc  Cancel"
    } else if state.proxy_editor.is_some() {
        "Ctrl-U  Clear     Tab / Up/Down  Field     Enter  Save     Esc  Cancel"
    } else if state.show_help {
        "? / Esc  Back     Q  Quit"
    } else {
        "Enter  Launch     D  Repair     B  Home .env     R  Refresh     ?  Help     Q  Quit"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(&state.status_message, theme::muted()),
            Line::styled(shortcuts, theme::title()),
        ]),
        area,
    );
}

fn draw_too_small(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("CHATGPT DESKTOP PROXY GUARD", theme::title()),
            Line::raw("Terminal too small"),
            Line::styled(
                "Resize to at least 52 × 18. Press Q to quit.",
                theme::muted(),
            ),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        area,
    );
}

fn key_value(label: &str, value: String, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<9}"), theme::muted()),
        Span::styled(value, value_style),
    ])
}

fn desktop_label(state: &AppState) -> String {
    match &state.desktop_app {
        DesktopAppDiscovery::Unknown => "Not inspected".into(),
        DesktopAppDiscovery::Searching => "Searching…".into(),
        DesktopAppDiscovery::Found(info) => format!(
            "{} · {} · {}",
            info.product.display_name(),
            info.package_version,
            info.architecture
        ),
        DesktopAppDiscovery::NotFound(_) => "Not found".into(),
    }
}

/// rc-stage discovery diagnostics (see the remediation manual §21): show the
/// exact registered application entry so a wrong or missing selection is
/// visible without a launch. Only relative manifest text is displayed —
/// never a full WindowsApps path.
fn entry_label(state: &AppState) -> String {
    match &state.desktop_app {
        DesktopAppDiscovery::Found(info) => match &info.target_kind {
            DesktopTargetKind::RegisteredPackage(application) => {
                let runtime = match application.runtime_kind {
                    proxy_guard_core::PackageRuntimeKind::FullTrustDesktop => "FullTrust",
                    proxy_guard_core::PackageRuntimeKind::AppContainer => "AppContainer",
                    proxy_guard_core::PackageRuntimeKind::Unknown => "Unknown",
                };
                format!(
                    "{} · {} · {}",
                    application.application_id, application.manifest_executable, runtime
                )
            }
            DesktopTargetKind::UnpackagedExecutable => "—".into(),
        },
        DesktopAppDiscovery::Unknown
        | DesktopAppDiscovery::Searching
        | DesktopAppDiscovery::NotFound(_) => "—".into(),
    }
}

fn process_label(state: &AppState) -> String {
    match state.desktop_process {
        DesktopProcessState::Unknown => "Unknown".into(),
        DesktopProcessState::Stopped => "Not running".into(),
        DesktopProcessState::Running { pid } => format!("Running (PID {pid})"),
    }
}

fn process_style(state: &AppState) -> Style {
    match state.desktop_process {
        DesktopProcessState::Running { .. } => theme::success(),
        DesktopProcessState::Stopped => theme::accent(),
        DesktopProcessState::Unknown => theme::muted(),
    }
}

fn primary_action(state: &AppState) -> &'static str {
    if state.foreground.is_some() {
        "Please wait…"
    } else if matches!(state.desktop_process, DesktopProcessState::Running { .. }) {
        "ChatGPT Desktop is already running — exit it fully before relaunching"
    } else if matches!(state.launch, LaunchState::Running(_)) {
        "ChatGPT Desktop was launched through the configured proxy"
    } else {
        "Press Enter to launch ChatGPT Desktop through this proxy"
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proxy_guard_core::{
        DesktopAppInfo, DesktopDiscoverySource, DesktopProduct, DesktopTargetKind, GuardConfig,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn rendered(width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = AppState::new(GuardConfig::default(), "config.toml".into());
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn full_layout_has_one_clear_primary_action() {
        let text = rendered(90, 20);
        assert!(text.contains("Press Enter to launch"));
        assert!(text.contains("HTTP_PROXY"));
        assert!(!text.contains("Usage"));
        assert!(!text.contains("Readiness"));
    }

    #[test]
    fn proxy_editor_has_clear_fields_and_save_instructions() {
        let backend = TestBackend::new(90, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(GuardConfig::default(), "config.toml".into());
        state.proxy_editor = Some(ProxyEditor {
            host: "127.0.0.1".into(),
            port: "7890".into(),
            active_field: ProxyField::Port,
            error: None,
        });
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Proxy configuration"));
        assert!(text.contains("7890|"));
        assert!(text.contains("Enter save"));
    }

    #[test]
    fn too_small_layout_is_actionable() {
        assert!(rendered(40, 10).contains("Terminal too small"));
    }

    #[test]
    fn daemon_repair_prompt_shows_interruption_warning_and_choices() {
        let backend = TestBackend::new(90, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(GuardConfig::default(), "config.toml".into());
        state.daemon_repair_prompt = true;
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Stop the shared Codex background server"));
        assert!(text.contains("may interrupt tasks"));
        assert!(text.contains("Y  Confirm and launch"));
        assert!(!text.contains("Press Enter to launch"));
        assert!(
            !text.contains("package-context"),
            "the experimental candidate wording must be gone"
        );
    }

    #[test]
    fn backend_proxy_prompt_names_the_home_and_scope() {
        let backend = TestBackend::new(100, 26);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(GuardConfig::default(), "config.toml".into());
        state.config.codex.proxy_env_home = PathBuf::from(r"C:\Users\fixture\.codex");
        state.backend_proxy_prompt = true;
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Authorize backend proxy consent"));
        assert!(text.contains("same Home, not just"));
        assert!(text.contains("Y  Confirm"));
    }

    #[test]
    fn registered_desktop_shows_native_activation_and_missing_backend_layer() {
        let backend = TestBackend::new(110, 26);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(GuardConfig::default(), "config.toml".into());
        state.desktop_app = DesktopAppDiscovery::Found(Box::new(DesktopAppInfo {
            product: DesktopProduct::ChatGpt,
            package_name: "OpenAI.Codex".into(),
            package_version: "26.924.2738.0".into(),
            architecture: "X64".into(),
            discovery_source: DesktopDiscoverySource::AppxManifest,
            target_kind: DesktopTargetKind::RegisteredPackage(
                proxy_guard_core::PackageApplication {
                    package_full_name: "OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0".into(),
                    package_family_name: "OpenAI.Codex_2p2nqsd0c76g0".into(),
                    application_id: "App".into(),
                    app_user_model_id: "OpenAI.Codex_2p2nqsd0c76g0!App".into(),
                    manifest_executable: "app/ChatGPT.exe".into(),
                    runtime_kind: proxy_guard_core::PackageRuntimeKind::FullTrustDesktop,
                },
            ),
            install_location: PathBuf::from("app"),
            executable: PathBuf::from("app/ChatGPT.exe"),
        }));
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Windows registered app activation"));
        assert!(text.contains("Chromium proxy arguments on activation"));
        assert!(text.contains("NOT proxied yet"));
        assert!(text.contains("press B"));
        // rc-stage discovery diagnostics: the selected entry is visible.
        assert!(text.contains("App · app/ChatGPT.exe · FullTrust"));
        // No experimental candidate hint remains.
        assert!(!text.contains("package-context"));
        assert!(!text.contains("press P"));
    }

    #[test]
    fn help_page_lists_current_keys_and_build_provenance() {
        let backend = TestBackend::new(110, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(GuardConfig::default(), "config.toml".into());
        state.show_help = true;
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("B  "));
        assert!(text.contains(".env proxy block"));
        assert!(text.contains("Build "));
        assert!(text.contains("commit "));
        assert!(!text.contains("package-context candidate"));
    }
}
