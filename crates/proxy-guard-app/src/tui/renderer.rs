use proxy_guard_core::{
    AppState, DesktopAppDiscovery, DesktopProcessState, LaunchState, ProxyEditor, ProxyField,
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
    } else if state.managed.proxy_lost {
        Span::styled("PROXY LOST", theme::error())
    } else if matches!(state.desktop_process, DesktopProcessState::Running { .. }) {
        Span::styled("RUNNING", theme::success())
    } else {
        Span::styled("READY", theme::accent())
    };
    let mode = if state.config.is_managed() {
        Span::styled("MANAGED", theme::accent())
    } else {
        Span::styled("EXTERNAL", theme::muted())
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("CHATGPT DESKTOP PROXY GUARD", theme::title()),
                Span::raw("  "),
                mode,
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
    } else if let Some(error) = &state.error_message {
        lines.push(Line::styled("Launch blocked", theme::error()));
        lines.push(Line::raw(error.clone()));
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Press C to configure and replace it, or Enter/Esc to dismiss.",
            theme::muted(),
        ));
    } else if state.config.is_managed() {
        draw_managed_content(&mut lines, state);
    } else {
        lines.push(key_value(
            "Proxy",
            state.config.proxy_url(),
            theme::accent(),
        ));
        lines.push(key_value("App", desktop_label(state), Style::default()));
        lines.push(key_value(
            "Source",
            desktop_source_label(state),
            theme::muted(),
        ));
        lines.push(key_value(
            "Process",
            process_label(state),
            process_style(state),
        ));
        if let DesktopAppDiscovery::NotFound(message) = &state.desktop_app {
            lines.push(Line::styled(message.clone(), theme::error()));
            lines.push(Line::styled(
                "Install: https://chatgpt.com/download/ (Store ID 9PLM9XGG6VKS)",
                theme::muted(),
            ));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("Injects  ", theme::muted()),
            Span::raw("HTTP_PROXY · HTTPS_PROXY · NO_PROXY; removes ALL_PROXY"),
        ]));
        lines.push(Line::styled(
            "This sets process environment only; it does not enforce all traffic.",
            theme::muted(),
        ));
        lines.push(Line::raw(""));
        lines.push(Line::styled(primary_action(state), theme::accent()));
        lines.push(Line::styled(
            "Press C to change the proxy host or port.",
            theme::muted(),
        ));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_managed_content(lines: &mut Vec<Line<'static>>, state: &AppState) {
    let managed = &state.managed;
    lines.push(key_value("Mode", "Managed".into(), theme::accent()));
    lines.push(key_value(
        "Subscription",
        managed
            .subscription_name
            .clone()
            .unwrap_or_else(|| "—".into()),
        Style::default(),
    ));
    lines.push(key_value("App", desktop_label(state), Style::default()));
    lines.push(key_value(
        "Process",
        process_label(state),
        process_style(state),
    ));
    if let DesktopAppDiscovery::NotFound(message) = &state.desktop_app {
        lines.push(Line::styled(message.clone(), theme::error()));
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled("SELECTED NODE", theme::title()));
    match &managed.selected {
        Some(selection) => {
            lines.push(key_value("Node", selection.name.clone(), theme::accent()));
            lines.push(key_value(
                "Region",
                format!("{} VERIFIED", selection.region),
                theme::success(),
            ));
            lines.push(key_value(
                "Score",
                selection.score.to_string(),
                Style::default(),
            ));
            lines.push(key_value(
                "Success",
                format!("{}%", selection.success_percent),
                Style::default(),
            ));
            lines.push(key_value(
                "Median",
                format!("{} ms", selection.median_ms),
                Style::default(),
            ));
            lines.push(key_value(
                "P95/5",
                format!("{} ms", selection.p95_ms),
                Style::default(),
            ));
            lines.push(key_value(
                "Proxy",
                managed
                    .proxy_endpoint
                    .clone()
                    .unwrap_or_else(|| "not running".into()),
                theme::accent(),
            ));
        }
        None => {
            lines.push(Line::styled(
                "No healthy node selected — run a benchmark (B)",
                theme::warning(),
            ));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled("REGIONS", theme::title()));
    lines.push(region_line(
        "JP",
        managed.regions.jp_active,
        managed.regions.jp_healthy,
    ));
    lines.push(region_line(
        "SG",
        managed.regions.sg_active,
        managed.regions.sg_healthy,
    ));
    lines.push(region_line(
        "US",
        managed.regions.us_active,
        managed.regions.us_healthy,
    ));

    if managed.proxy_lost {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Managed proxy stopped unexpectedly. Desktop remains open.",
            theme::error(),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(primary_action(state), theme::accent()));
}

fn region_line(region: &str, active: usize, healthy: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{region:<4}"), theme::muted()),
        Span::styled(
            format!("{active:<3} active"),
            if active > 0 {
                Style::default()
            } else {
                theme::muted()
            },
        ),
        Span::raw("  "),
        Span::styled(
            format!("{healthy:<3} healthy"),
            if healthy > 0 {
                theme::success()
            } else {
                theme::muted()
            },
        ),
    ])
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
            Span::raw("Launch ChatGPT Desktop through the configured proxy"),
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
            Span::styled("S          ", theme::accent()),
            Span::raw("Sync the configured subscription (Managed Mode)"),
        ]),
        Line::from(vec![
            Span::styled("B          ", theme::accent()),
            Span::raw("Benchmark JP/SG/US nodes (Managed Mode)"),
        ]),
        Line::from(vec![
            Span::styled("?          ", theme::accent()),
            Span::raw("Close this help"),
        ]),
        Line::from(vec![
            Span::styled("Q / Ctrl-C ", theme::accent()),
            Span::raw("Quit Guard; ChatGPT Desktop keeps running"),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let shortcuts = if state.proxy_editor.is_some() {
        "Ctrl-U  Clear     Tab / Up/Down  Field     Enter  Save     Esc  Cancel"
    } else if state.show_help {
        "? / Esc  Back     Q  Quit"
    } else if state.config.is_managed() {
        "Enter  Launch     B  Benchmark     S  Sync     R  Refresh     ?  Help     Q  Quit"
    } else {
        "Enter  Launch     R  Refresh     ?  Help     Q  Quit"
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

fn desktop_source_label(state: &AppState) -> String {
    match &state.desktop_app {
        DesktopAppDiscovery::Found(info) => format!(
            "{} · {}",
            info.discovery_source.display_name(),
            info.product.selection_reason()
        ),
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
        "ChatGPT Desktop launched through the configured proxy"
    } else {
        "Press Enter to launch ChatGPT Desktop through this proxy"
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proxy_guard_core::{DesktopAppInfo, DesktopDiscoverySource, DesktopProduct, GuardConfig};
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
    fn discovered_app_explains_product_version_architecture_and_source() {
        let backend = TestBackend::new(110, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(GuardConfig::default(), "config.toml".into());
        state.desktop_app = DesktopAppDiscovery::Found(DesktopAppInfo {
            product: DesktopProduct::ChatGpt,
            package_name: "OpenAI.Codex".into(),
            package_version: "26.727.6591.0".into(),
            architecture: "X64".into(),
            discovery_source: DesktopDiscoverySource::AppxManifest,
            install_location: PathBuf::from("app"),
            executable: PathBuf::from("app/ChatGPT.exe"),
        });
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("ChatGPT Desktop · 26.727.6591.0 · X64"));
        assert!(text.contains("APPX manifest · current ChatGPT desktop app"));
    }
}
