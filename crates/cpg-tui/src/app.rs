//! The Ratatui application: event loop, key translation, modal state
//! machine and rendering. All decisions about what is allowed come from the
//! engine's `SessionView`; this module only draws it and forwards intents.

use crate::terminal::{TerminalGuard, with_panic_guard};
use cpg_domain::text::{public_text, wide_to_utf8};
use cpg_domain::{Error, Result};
use cpg_engine::{
    ConsentProposal, EngineSession, LaunchOutcome, MutationOutcome, RepairProposal, SessionView,
};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::{execute, queue};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::path::Path;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Modal {
    None,
    ProxyEditor,
    Consent,
    Repair,
    LaunchResult,
    ErrorResult,
    Help,
    QuitWhileBusy,
}

#[derive(Debug)]
enum Action {
    None,
    Refresh,
    Launch,
    OpenEditor,
    OpenConsent,
    OpenRepair,
    SaveProxy { host: String, port: i64 },
    ConfirmConsent(ConsentProposal),
    ConfirmRepair(RepairProposal),
    CopyDiagnostic,
    Quit,
    Cancel,
    CancelAndQuit,
}

struct Model {
    session: EngineSession,
    view: SessionView,
    modal: Modal,
    notice: Option<String>,
    editor_field: u8, // 0 = host, 1 = port
    editor_host: String,
    editor_port: String,
    editor_host_cursor: usize, // char index into `editor_host`
    editor_port_cursor: usize, // char index into `editor_port`
    editor_error: Option<String>,
    saving_operation: Option<i64>,
    confirm_buffer: String,
    confirm_scroll: usize,
    outcome_scroll: usize,
    outcome_code: String,
    outcome_message: String,
    outcome_facts: String,
    outcome_warning: bool,
    exit_requested: bool,
    refreshed_launch: Option<i64>,
    spinner_phase: u8,
    busy_label: String,
    /// Last observed terminal size; gates confirmations that cannot show
    /// the full authorization scope.
    viewport: (u16, u16),
}

impl Model {
    fn new(session: EngineSession) -> Model {
        let view = session.view();
        Model {
            session,
            view,
            modal: Modal::None,
            notice: None,
            editor_field: 0,
            editor_host: String::new(),
            editor_port: String::new(),
            editor_host_cursor: 0,
            editor_port_cursor: 0,
            editor_error: None,
            saving_operation: None,
            confirm_buffer: String::new(),
            confirm_scroll: 0,
            outcome_scroll: 0,
            outcome_code: String::new(),
            outcome_message: String::new(),
            outcome_facts: String::new(),
            outcome_warning: false,
            exit_requested: false,
            refreshed_launch: None,
            spinner_phase: 0,
            busy_label: "Updating".to_owned(),
            viewport: (0, 0),
        }
    }

    fn synchronize(&mut self) {
        self.view = self.session.view();
        let label = if self.session.cancelling_after_timeout() {
            "Cancelling timed-out operation"
        } else if self.exit_requested {
            "Cleaning up before exit"
        } else if self.session.launching() {
            "Launching Desktop"
        } else {
            "Updating"
        };
        self.busy_label = label.to_owned();
        self.spinner_phase = self.spinner_phase.wrapping_add(1);
    }

    fn set_operation_outcome(&mut self, error: Option<&Error>) {
        self.notice = None;
        self.outcome_scroll = 0;
        self.outcome_facts.clear();
        self.outcome_warning = false;
        match error {
            Some(error) => {
                self.outcome_code = error.code.clone();
                self.outcome_message = public_text(&error.message, 4096);
                self.modal = Modal::ErrorResult;
            }
            None => {
                self.outcome_code = String::new();
                self.outcome_message = "Operation completed.".to_owned();
                self.modal = Modal::LaunchResult;
            }
        }
    }

    fn set_launch_outcome(&mut self, outcome: &LaunchOutcome) {
        self.set_operation_outcome(outcome.error.as_ref());
        self.outcome_facts = format!(
            "Activation: {}\nBackend config: {}\nDaemon: {}",
            outcome.activation_state, outcome.backend_proxy_config, outcome.daemon_preparation
        );
        if let Some(receipt) = &outcome.receipt {
            self.outcome_facts.push_str(&format!(
                "\nPID: {}\nInstance: {}\nProxy delivery: {}",
                receipt
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                receipt.instance,
                receipt.proxy_delivery
            ));
            self.outcome_warning = receipt.instance != "created";
            if receipt.instance == "reused" {
                self.outcome_message.push_str(
                    " An existing instance was returned. Adoption of this proxy is unconfirmed.",
                );
            } else if receipt.instance == "unknown" {
                self.outcome_message.push_str(
                    " Instance creation is unknown. Refresh status; do not automatically retry.",
                );
            } else if outcome.error.is_none() {
                self.outcome_message = "Desktop activation returned a new instance.".to_owned();
            }
        }
        if outcome.exceeded_budget {
            self.outcome_facts
                .push_str("\nSession budget exceeded; returned facts are preserved.");
        }
        self.outcome_facts
            .push_str("\nPrepared configuration is not a network verification.");
    }

    fn apply_mutation_outcome(&mut self, outcome: &MutationOutcome) {
        match self.saving_operation {
            Some(operation) if operation == outcome.operation_id => {
                self.saving_operation = None;
                if outcome.committed {
                    self.modal = Modal::None;
                    self.notice = Some(
                        if outcome.refresh_error.is_some() {
                            "Saved. Status refresh failed; R retries reading only."
                        } else {
                            "Proxy saved."
                        }
                        .to_owned(),
                    );
                } else {
                    self.modal = Modal::ProxyEditor;
                    let message = outcome
                        .mutation_error
                        .as_ref()
                        .map(|error| public_text(&error.message, 1024))
                        .unwrap_or_else(|| "Save did not complete. Your draft is retained.".into());
                    self.editor_error = Some(message);
                    match outcome
                        .mutation_error
                        .as_ref()
                        .and_then(|error| error.field.as_deref())
                    {
                        Some("proxy.host") => self.editor_field = 0,
                        Some("proxy.port") => self.editor_field = 1,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            let action = if self.session.busy() {
                Action::Cancel
            } else {
                Action::None
            };
            return action;
        }
        if self.modal == Modal::QuitWhileBusy {
            match key.code {
                KeyCode::Char('y' | 'Y') => {
                    self.modal = Modal::None;
                    return Action::CancelAndQuit;
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n' | 'N') => {
                    self.modal = Modal::None
                }
                _ => {}
            }
            return Action::None;
        }
        if self.session.busy() {
            if let KeyCode::Char('q' | 'Q') = key.code {
                self.modal = Modal::QuitWhileBusy;
            }
            return Action::None;
        }
        match self.modal {
            Modal::None => self.main_key(key),
            Modal::QuitWhileBusy => Action::None,
            Modal::ProxyEditor => self.editor_key(key),
            Modal::Consent | Modal::Repair => self.confirm_key(key),
            Modal::ErrorResult | Modal::LaunchResult => match key.code {
                KeyCode::Up => {
                    self.outcome_scroll = self.outcome_scroll.saturating_sub(1);
                    Action::None
                }
                KeyCode::Down => {
                    self.outcome_scroll += 1;
                    Action::None
                }
                KeyCode::Char('c' | 'C') => Action::CopyDiagnostic,
                KeyCode::Char('r' | 'R') | KeyCode::F(5) => Action::Refresh,
                KeyCode::Char('e' | 'E' | 'b' | 'B') => self.main_key(key),
                KeyCode::Esc | KeyCode::Enter => {
                    self.modal = Modal::None;
                    Action::None
                }
                _ => Action::None,
            },
            Modal::Help => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::F(1) | KeyCode::Char('?') => {
                    self.modal = Modal::None;
                    Action::None
                }
                _ => Action::None,
            },
        }
    }

    fn main_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Enter | KeyCode::Char('l' | 'L') => {
                if self.view.actions.can_launch {
                    Action::Launch
                } else {
                    let reason = if self.view.actions.blocked_reason.is_empty() {
                        "Launch unavailable. R refreshes status."
                    } else {
                        &self.view.actions.blocked_reason
                    };
                    self.notice = Some(reason.to_owned());
                    Action::None
                }
            }
            KeyCode::F(5) => {
                if self.view.actions.can_refresh {
                    Action::Refresh
                } else {
                    Action::None
                }
            }
            KeyCode::Char('r' | 'R') => {
                if self.view.actions.can_refresh {
                    Action::Refresh
                } else {
                    Action::None
                }
            }
            KeyCode::Char('e' | 'E') => {
                if self.view.actions.can_edit_proxy {
                    Action::OpenEditor
                } else {
                    self.notice = Some("Proxy editing unavailable - press R to refresh.".into());
                    Action::None
                }
            }
            KeyCode::Char('b' | 'B') => {
                if (self.view.actions.can_authorize_backend_proxy
                    || self.view.actions.can_revoke_backend_proxy)
                    && self.view.consent.is_some()
                {
                    Action::OpenConsent
                } else {
                    self.notice =
                        Some("Backend authorization unavailable - press R to refresh.".into());
                    Action::None
                }
            }
            KeyCode::Char('d' | 'D') => {
                if self.view.actions.can_repair && self.view.repair.is_some() {
                    Action::OpenRepair
                } else {
                    self.notice = Some("Repair launch unavailable - press R to refresh.".into());
                    Action::None
                }
            }
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.modal = Modal::Help;
                Action::None
            }
            KeyCode::Char('q' | 'Q') => Action::Quit,
            _ => Action::None,
        }
    }

    /// Inserts one accepted character at the active field's cursor.
    fn editor_insert(&mut self, character: char) {
        let port_field = self.editor_field == 1;
        let limit = if port_field { 5 } else { 255 };
        if port_field && !character.is_ascii_digit() {
            return;
        }
        if self.active_editor_text().chars().count() >= limit {
            return;
        }
        let (field, cursor) = self.active_editor_parts_mut();
        let byte = char_index_to_byte(field, *cursor);
        field.insert(byte, character);
        *cursor += 1;
        self.editor_error = None;
    }

    /// Accepts a single-line paste into the proxy editor only; the consent
    /// and repair confirmations never accept pasted input, and a paste that
    /// carries line breaks or control characters is refused whole.
    fn editor_paste(&mut self, text: &str) {
        if self.modal != Modal::ProxyEditor || self.session.busy() {
            return;
        }
        if text.chars().any(|character| character.is_control()) {
            return;
        }
        for character in text.chars() {
            self.editor_insert(character);
        }
    }

    fn active_editor_text(&self) -> &str {
        if self.editor_field == 0 {
            &self.editor_host
        } else {
            &self.editor_port
        }
    }

    fn active_editor_parts_mut(&mut self) -> (&mut String, &mut usize) {
        if self.editor_field == 0 {
            (&mut self.editor_host, &mut self.editor_host_cursor)
        } else {
            (&mut self.editor_port, &mut self.editor_port_cursor)
        }
    }

    fn editor_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.modal = Modal::None;
                Action::None
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.editor_field = 1 - self.editor_field;
                let (field, cursor) = self.active_editor_parts_mut();
                *cursor = field.chars().count();
                Action::None
            }
            KeyCode::Enter => {
                let port = match cpg_domain::parse_port_strict(&self.editor_port) {
                    Ok(port) => port as i64,
                    Err(_) => {
                        self.editor_field = 1;
                        self.editor_port_cursor = self.editor_port.chars().count();
                        self.editor_error =
                            Some("Port must be a number between 1 and 65535.".into());
                        return Action::None;
                    }
                };
                Action::SaveProxy {
                    host: self.editor_host.clone(),
                    port,
                }
            }
            KeyCode::Left => {
                let (_, cursor) = self.active_editor_parts_mut();
                *cursor = cursor.saturating_sub(1);
                Action::None
            }
            KeyCode::Right => {
                let (field, cursor) = self.active_editor_parts_mut();
                if *cursor < field.chars().count() {
                    *cursor += 1;
                }
                Action::None
            }
            KeyCode::Home => {
                let (_, cursor) = self.active_editor_parts_mut();
                *cursor = 0;
                Action::None
            }
            KeyCode::End => {
                let (field, cursor) = self.active_editor_parts_mut();
                *cursor = field.chars().count();
                Action::None
            }
            KeyCode::Backspace => {
                let (field, cursor) = self.active_editor_parts_mut();
                if *cursor > 0 {
                    let byte = char_index_to_byte(field, *cursor - 1);
                    field.remove(byte);
                    *cursor -= 1;
                    self.editor_error = None;
                }
                Action::None
            }
            KeyCode::Delete => {
                let (field, cursor) = self.active_editor_parts_mut();
                if *cursor < field.chars().count() {
                    let byte = char_index_to_byte(field, *cursor);
                    field.remove(byte);
                    self.editor_error = None;
                }
                Action::None
            }
            KeyCode::Char(character) => {
                self.editor_insert(character);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn confirm_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.modal = Modal::None;
                self.confirm_buffer.clear();
                Action::None
            }
            KeyCode::Up => {
                self.confirm_scroll = self.confirm_scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.confirm_scroll += 1;
                Action::None
            }
            KeyCode::Backspace => {
                self.confirm_buffer.pop();
                Action::None
            }
            KeyCode::Enter => {
                let mut action = Action::None;
                if self.confirm_buffer == "YES" {
                    let consent = self.modal == Modal::Consent;
                    let proposal = self.session.view().consent;
                    if !confirmation_fully_visible(self.viewport, consent, proposal.as_ref()) {
                        // The scope (or its side effects) is clipped: a typed
                        // YES authorizes nothing until it is fully readable.
                        // The buffer is kept so a resize plus Enter can
                        // confirm without retyping.
                        return Action::None;
                    } else if self.modal == Modal::Repair {
                        if let Some(repair) = self.session.view().repair.clone() {
                            action = Action::ConfirmRepair(repair);
                            self.modal = Modal::None;
                        }
                    } else if let Some(consent) = proposal {
                        action = Action::ConfirmConsent(consent);
                        self.modal = Modal::None;
                    }
                }
                self.confirm_buffer.clear();
                action
            }
            KeyCode::Char(character) => {
                if !character.is_control() && self.confirm_buffer.len() < 8 {
                    self.confirm_buffer.push(character.to_ascii_uppercase());
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn diagnostic_text(&self) -> String {
        let mut text = format!(
            "Codex Proxy Guard diagnostic\nversion: {}\ncommit: {}",
            crate::terminal::diagnostic_version(),
            crate::terminal::diagnostic_commit()
        );
        if !self.outcome_message.is_empty() {
            text.push_str(&format!(
                "\noperation.error.code: {}\noperation.message: {}",
                self.outcome_code, self.outcome_message
            ));
        }
        if let Some(error) = &self.view.error {
            text.push_str(&format!(
                "\nstatus.error.code: {}\nstatus.error.message: {}",
                error.code, error.message
            ));
        }
        text.push_str(&format!(
            "\ndesktop: {}\nentry: {}\ncoverage: {}\n{}",
            self.view.desktop.display_name,
            self.view.desktop.application_id.as_deref().unwrap_or("-"),
            self.view.coverage.state,
            self.outcome_facts
        ));
        public_text(&text, 4096)
    }
}

/// Byte offset of a character index (cursor math is per character, never
/// per byte, so multi-byte host input cannot split a code point).
fn char_index_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

fn state_style(state: &str) -> Color {
    match state {
        "ready" | "current" | "running" | "stopped" | "found" | "prepared" => Color::Green,
        "unknown" => Color::DarkGray,
        "pending" | "stale" | "not_authorized" | "repair_required" => Color::Yellow,
        _ => Color::Red,
    }
}

fn render(frame: &mut Frame, model: &Model) {
    let area = frame.area();
    let [header, body, action_bar, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(2),
    ])
    .areas(area);

    // Header
    let title = Line::from(vec![
        Span::styled(" CODEX PROXY GUARD ", Style::default().bold()),
        Span::raw("  "),
        Span::styled(
            format!("{} · ", crate::terminal::diagnostic_version()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            {
                if model.session.busy() {
                    format!(
                        "BUSY {}",
                        "|/-\\"
                            .chars()
                            .nth(model.spinner_phase as usize % 4)
                            .unwrap_or('|')
                    )
                } else if model.view.process.state == "running" {
                    "RUNNING".to_owned()
                } else if model.view.actions.can_launch {
                    "READY".to_owned()
                } else {
                    "BLOCKED".to_owned()
                }
            },
            Style::default().fg(if model.view.actions.can_launch {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
    ]);
    let subtitle = Line::from(Span::styled(
        " Local proxy · Windows desktop launcher",
        Style::default().fg(Color::DarkGray),
    ));
    let header_paragraph = Paragraph::new(vec![title, subtitle]).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(header_paragraph, header);

    // Body: two columns on wide terminals, single column when narrow.
    let wide = body.width >= 72;
    let columns = if wide {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .spacing(1)
                .areas(body);
        vec![left, right]
    } else {
        vec![body]
    };

    let mut rows: Vec<(&str, Line)> = Vec::new();
    rows.push((
        "TARGET",
        Line::from(vec![
            Span::styled(model.view.desktop.display_name, Style::default().bold()),
            Span::styled(
                format!(
                    " · {} · {}",
                    model.view.desktop.package_version, model.view.desktop.architecture
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ));
    rows.push((
        "LAUNCH",
        Line::from(vec![
            Span::raw(model.view.launch_method),
            Span::styled(
                format!(
                    " · {}",
                    model
                        .view
                        .desktop
                        .manifest_executable
                        .as_deref()
                        .unwrap_or("-")
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ));
    rows.push((
        "PROCESS",
        Line::from(Span::styled(
            format!(
                "{}{}",
                model.view.process.state,
                model
                    .view
                    .process
                    .pid
                    .map(|pid| format!(" ({pid})"))
                    .unwrap_or_default()
            ),
            Style::default().fg(state_style(model.view.process.state)),
        )),
    ));
    let left_lines: Vec<Line> = rows
        .iter()
        .map(|(label, line)| {
            let mut merged = Line::from(Span::styled(
                format!("{label:<9}"),
                Style::default().fg(Color::DarkGray),
            ));
            merged.spans.extend(line.spans.iter().cloned());
            merged
        })
        .collect();
    let left_paragraph = Paragraph::new(left_lines);
    frame.render_widget(left_paragraph, columns[0]);

    if let Some(right) = columns.get(1) {
        let mut right_rows: Vec<Line> = vec![Line::from(vec![
            Span::styled("PROXY    ", Style::default().fg(Color::DarkGray)),
            Span::raw(model.view.proxy.url.clone()),
        ])];
        right_rows.push(Line::from(vec![
            Span::styled("COVERAGE ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                model.view.coverage.state,
                Style::default().fg(state_style(model.view.coverage.state)),
            ),
            Span::styled(
                model
                    .view
                    .coverage
                    .authorized_home
                    .as_ref()
                    .map(|home| format!(" · {}", home.display()))
                    .unwrap_or_default(),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        right_rows.push(Line::from(vec![
            Span::styled("ELEVATION", Style::default().fg(Color::DarkGray)),
            Span::raw(" "),
            Span::styled(
                model.view.elevation,
                Style::default().fg(if model.view.elevation == "not_elevated" {
                    Color::Green
                } else {
                    Color::Red
                }),
            ),
        ]));
        frame.render_widget(Paragraph::new(right_rows), *right);
    }

    // Action bar
    let primary = if model.session.busy() {
        Line::from(Span::styled(
            format!("Ctrl+C Cancel · Q Cancel and quit · {}", model.busy_label),
            Style::default().fg(Color::Yellow),
        ))
    } else if model.view.actions.can_launch {
        Line::from(Span::styled(
            format!("[ Enter ] Launch {}", model.view.desktop.display_name),
            Style::default().bold(),
        ))
    } else {
        Line::from(Span::styled(
            model.view.actions.blocked_reason.clone(),
            Style::default().fg(Color::Yellow),
        ))
    };
    frame.render_widget(primary, action_bar);
    frame.render_widget(
        Line::from(Span::styled(
            " E Proxy · B Backend · D Repair · R Refresh · ? Help · Q Quit",
            Style::default().fg(Color::DarkGray),
        )),
        footer,
    );

    render_modal(frame, model, area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

/// Rows a single text line occupies when word-wrapped at `width` (greedy
/// wrap with hard breaks for overlong words, matching the Paragraph wrap
/// used for these sentences closely enough for a conservative gate).
fn wrapped_rows(text: &str, width: usize) -> usize {
    if width == 0 {
        return usize::MAX;
    }
    let mut rows = 1usize;
    let mut current = 0usize; // columns used on the current row
    let mut first = true; // word would start the row (no separator needed)
    for word in text.split(' ').filter(|word| !word.is_empty()) {
        let columns = word.width();
        if !first && current + 1 + columns <= width {
            current += 1 + columns;
            continue;
        }
        if !first {
            rows += 1;
        }
        if columns <= width {
            current = columns;
        } else {
            // Hard-break a word wider than a full row.
            rows += columns / width - usize::from(columns % width == 0);
            current = if columns % width == 0 {
                width
            } else {
                columns % width
            };
        }
        first = false;
    }
    rows
}

/// Logical lines of the confirmation text. `true` marks the exact-Home
/// line, which must be visible unwrapped: authorizing a scope the user
/// cannot read end-to-end is never allowed (audit F08).
fn confirmation_lines(consent: bool, proposal: Option<&ConsentProposal>) -> Vec<(String, bool)> {
    if let (true, Some(proposal)) = (consent, proposal) {
        vec![
            (
                if proposal.enabled {
                    "Authorize HTTP_PROXY/HTTPS_PROXY/NO_PROXY in this exact Home:"
                } else {
                    "Remove Guard's unchanged proxy block from this exact Home:"
                }
                .to_owned(),
                false,
            ),
            (proposal.home.display().to_string(), true),
            (String::new(), false),
            (
                "Only Guard's marked block is managed. It affects later Codex processes sharing this Home."
                    .to_owned(),
                false,
            ),
        ]
    } else {
        vec![
            (
                "Stop the shared Codex background server once. This may interrupt other clients' tasks on the same Home."
                    .to_owned(),
                false,
            ),
            (
                "The backend proxy block is prepared before the stop.".to_owned(),
                false,
            ),
        ]
    }
}

/// Popup width: the fixed 72-column dialog, widened (capped at 100) so a
/// long Home path can sit on one unwrapped row on wide terminals.
fn confirmation_popup_width(consent: bool, proposal: Option<&ConsentProposal>) -> u16 {
    let mut width = 72u16;
    if let (true, Some(proposal)) = (consent, proposal) {
        let needed = proposal.home.display().to_string().width() as u16 + 8;
        width = width.max(needed).min(100);
    }
    width
}

/// True when the whole confirmation text - including the exact Home - is
/// visible at once at the current terminal size, together with the YES row.
/// `YES` entered while this is false authorizes nothing.
fn confirmation_fully_visible(
    viewport: (u16, u16),
    consent: bool,
    proposal: Option<&ConsentProposal>,
) -> bool {
    let area = Rect {
        x: 0,
        y: 0,
        width: viewport.0,
        height: viewport.1,
    };
    let rect = centered(area, confirmation_popup_width(consent, proposal), 17);
    let inner = rect.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 2,
    });
    if inner.width < 8 || inner.height < 5 {
        return false;
    }
    // Rows above the confirmation row (rendered 3 rows above the bottom).
    let content_rows = inner.height.saturating_sub(3) as usize;
    let width = inner.width as usize;
    let mut rows = 0usize;
    for (line, exact_home) in confirmation_lines(consent, proposal) {
        if line.is_empty() {
            rows += 1;
        } else if exact_home {
            if line.width() > width {
                return false;
            }
            rows += 1;
        } else {
            rows += wrapped_rows(&line, width);
        }
    }
    rows > 0 && rows <= content_rows
}

fn render_modal(frame: &mut Frame, model: &Model, area: Rect) {
    let popup = |frame: &mut Frame, title: &str, width, height| -> Rect {
        let rect = centered(area, width, height);
        let block = Block::default()
            .title(Span::styled(
                format!(" {title} "),
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        rect
    };

    match model.modal {
        Modal::None => {}
        Modal::ProxyEditor => {
            let rect = popup(frame, "Proxy Configuration", 64, 12);
            let inner = rect.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 2,
            });
            let caret_field = |text: &str, cursor: usize, active: bool| -> String {
                if !active {
                    return text.to_owned();
                }
                let chars: Vec<char> = text.chars().collect();
                let head: String = chars.iter().take(cursor).collect();
                let tail: String = chars.iter().skip(cursor).collect();
                format!("{head}▌{tail}")
            };
            let mut lines = vec![
                Line::from(format!(
                    "Host [{}]",
                    caret_field(
                        &model.editor_host,
                        model.editor_host_cursor,
                        model.editor_field == 0
                    )
                )),
                Line::from(""),
                Line::from(format!(
                    "Port [{}]",
                    caret_field(
                        &model.editor_port,
                        model.editor_port_cursor,
                        model.editor_field == 1
                    )
                )),
            ];
            if let Some(error) = &model.editor_error {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    error.clone(),
                    Style::default().fg(Color::Red),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                if model.saving_operation.is_some() {
                    "Saving... Ctrl+C cancels; draft retained."
                } else {
                    "Tab switch · Enter save · Esc cancel"
                },
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Modal::Consent | Modal::Repair => {
            let consent = model.modal == Modal::Consent;
            let proposal = model.session.view().consent;
            let rect = popup(
                frame,
                if consent {
                    "Backend Proxy Authorization"
                } else {
                    "Repair Launch"
                },
                confirmation_popup_width(consent, proposal.as_ref()),
                17,
            );
            let inner = rect.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 2,
            });
            let message = confirmation_lines(consent, proposal.as_ref())
                .iter()
                .map(|(line, _)| line.as_str())
                .collect::<Vec<&str>>()
                .join("\n");
            let lines: Vec<Line> = message.lines().map(Line::from).collect();
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((model.confirm_scroll as u16, 0)),
                inner,
            );
            let gate_open = confirmation_fully_visible(model.viewport, consent, proposal.as_ref());
            let confirm = if gate_open {
                Line::from(vec![
                    Span::styled(
                        "Type YES to confirm this single operation: ",
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(
                        format!("{}_", model.confirm_buffer),
                        Style::default().bold(),
                    ),
                ])
            } else {
                Line::from(Span::styled(
                    "Window too small for the full scope - enlarge it to confirm.",
                    Style::default().fg(Color::Red),
                ))
            };
            frame.render_widget(
                Paragraph::new(confirm).alignment(Alignment::Left),
                Rect {
                    x: inner.x,
                    y: inner.y + inner.height.saturating_sub(3),
                    width: inner.width,
                    height: 1,
                },
            );
        }
        Modal::QuitWhileBusy => {
            let rect = popup(frame, "Operation Active", 64, 6);
            let inner = rect.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 2,
            });
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        "Cancel the current operation and exit after cleanup?",
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Y confirms · Enter/Esc keeps running (default)",
                        Style::default().fg(Color::DarkGray),
                    )),
                ]),
                inner,
            );
        }
        Modal::Help => {
            let rect = popup(frame, "Help", 72, 15);
            let inner = rect.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 2,
            });
            frame.render_widget(
                Paragraph::new(
                    "Enter / L   Launch Desktop\n\
                     E   Edit proxy host and port\n\
                     B   Authorize/revoke backend proxy block\n\
                     D   Repair launch (one shared daemon stop)\n\
                     R / F5   Refresh status\n\
                     Ctrl+C   Cancel current operation\n\
                     Q   Quit (never terminates Desktop)\n\n\
                     Closing Guard never closes Desktop."
                        .to_owned(),
                )
                .wrap(Wrap { trim: false }),
                inner,
            );
        }
        Modal::LaunchResult | Modal::ErrorResult => {
            let rect = popup(
                frame,
                if model.modal == Modal::ErrorResult {
                    "Operation Failed"
                } else {
                    "Operation Result"
                },
                72,
                17,
            );
            let inner = rect.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 2,
            });
            let title = if !model.outcome_code.is_empty() {
                model.outcome_code.clone()
            } else if model.outcome_warning {
                "Outcome needs attention".to_owned()
            } else {
                "Operation completed".to_owned()
            };
            let body = format!("{}\n{}", model.outcome_message, model.outcome_facts);
            let title_color = match model.modal {
                Modal::ErrorResult => Color::Red,
                _ if model.outcome_warning => Color::Yellow,
                _ => Color::Green,
            };
            let mut lines: Vec<Line> = vec![Line::from(Span::styled(
                title,
                Style::default()
                    .fg(title_color)
                    .add_modifier(Modifier::BOLD),
            ))];
            lines.extend(body.lines().map(Line::from));
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((model.outcome_scroll as u16, 0)),
                inner,
            );
            frame.render_widget(
                Line::from(Span::styled(
                    "Up/Down details · R refresh · C copy · Esc close",
                    Style::default().fg(Color::DarkGray),
                )),
                Rect {
                    x: inner.x,
                    y: inner.y + inner.height.saturating_sub(1),
                    width: inner.width,
                    height: 1,
                },
            );
        }
    }
}

/// Runs the interactive console. The terminal guard restores state on every
/// exit path, including panics and Ctrl+C-driven early exit.
pub fn run(config_path: &Path) -> Result<i32> {
    let config = cpg_engine::load_or_create_config(config_path)?;
    let alternate = !matches!(config.alternate_screen.as_str(), "never");
    let mut guard = TerminalGuard::enter(alternate)?;
    let outcome = with_panic_guard(|| run_loop(&mut guard, config_path));
    guard.restore();
    outcome
}

fn run_loop(guard: &mut TerminalGuard, config_path: &Path) -> Result<i32> {
    // Console control handler for this process: raw mode keeps Ctrl+C
    // arriving as ordinary key events (handled by `Model::key`), so this
    // scope covers Ctrl+Break, window Close and logoff/shutdown. Close
    // routes to cancel-and-exit; the OS enforces its own final termination
    // deadline after CTRL_CLOSE_EVENT, so settlement is bounded by the
    // engine's reap, not by an unbounded wait.
    let controls = cpg_windows::console::ConsoleControlScope::new();
    let stdout = std::io::stdout();
    {
        let mut locked = stdout.lock();
        let _ = execute!(locked, EnableBracketedPaste);
    }
    let result = (|| -> Result<i32> {
        let session = EngineSession::new(config_path.to_path_buf())?;
        let mut model = Model::new(session);
        model.session.request_refresh();
        model.synchronize();
        // Event-driven rendering: draw once, then only when input, engine
        // events, resize or a state transition changed something. A busy
        // spinner animates at a bounded frame rate instead of the old fixed
        // ~50 fps redraw, and an idle session performs no draws at all.
        const SPINNER_FRAME_INTERVAL: Duration = Duration::from_millis(125);
        let mut dirty = true;
        let mut last_spinner_frame = std::time::Instant::now();
        let mut last_busy = model.session.busy();
        let mut last_cancelling = model.session.cancelling_after_timeout();
        let mut last_viewport = model.viewport;
        loop {
            if controls.take_close() {
                model.exit_requested = true;
                if model.session.busy() {
                    model.session.cancel();
                }
            } else if controls.take_cancel() && model.session.busy() {
                // Idle Ctrl+C is consumed as a no-op (key events keep their
                // existing behavior), busy Ctrl+Break cancels the operation.
                model.session.cancel();
            }
            if let Ok(size) = guard.terminal_mut().size() {
                model.viewport = (size.width, size.height);
            }
            if model.viewport != last_viewport {
                last_viewport = model.viewport;
                dirty = true;
            }
            let busy = model.session.busy();
            let cancelling = model.session.cancelling_after_timeout();
            if busy != last_busy || cancelling != last_cancelling {
                dirty = true;
            }
            if dirty || (busy && last_spinner_frame.elapsed() >= SPINNER_FRAME_INTERVAL) {
                let frame_result = guard.terminal_mut().draw(|frame| render(frame, &model));
                if frame_result.is_err() {
                    return Err(Error::new(
                        "CONSOLE_OUTPUT_FAILED",
                        "Cannot render the console interface.",
                    ));
                }
                dirty = false;
                last_spinner_frame = std::time::Instant::now();
            }
            last_busy = busy;
            last_cancelling = cancelling;
            // Poll the session and input with short slices so busy states and
            // timeouts progress.
            if crossterm::event::poll(Duration::from_millis(20)).unwrap_or(false) {
                match crossterm::event::read().ok() {
                    Some(Event::Key(key)) => {
                        let action = model.key(key);
                        apply_action(&mut model, action)?;
                        dirty = true;
                    }
                    Some(Event::Paste(text)) => {
                        model.editor_paste(&text);
                        dirty = true;
                    }
                    // Any other event (resize, focus) still forces a redraw.
                    Some(_) => dirty = true,
                    None => {}
                }
            }
            // Drive the engine state machine.
            let event = model.session.poll();
            if event != cpg_engine::session::Event::None {
                model.synchronize();
                dirty = true;
            }
            handle_event(&mut model, event)?;
            model.synchronize();
            if model.exit_requested && !model.session.busy() {
                return Ok(0);
            }
        }
    })();
    let stdout = std::io::stdout();
    {
        let mut locked = stdout.lock();
        let _ = queue!(locked, DisableBracketedPaste);
    }
    result
}

fn handle_event(model: &mut Model, event: cpg_engine::session::Event) -> Result<()> {
    match event {
        cpg_engine::session::Event::None => {}
        cpg_engine::session::Event::SnapshotArrived => {
            if let Some(error) = model.view.error.clone() {
                if model.modal == Modal::None && model.session.launch_outcome().is_none() {
                    model.set_operation_outcome(Some(&error));
                } else if model.session.launch_outcome().is_some() {
                    model.notice = Some("Status refresh failed; launch result is retained.".into());
                }
            }
        }
        cpg_engine::session::Event::MutationFinished => {
            if let Some(outcome) = model.session.mutation_outcome() {
                if model.saving_operation.is_some() {
                    model.apply_mutation_outcome(&outcome);
                } else if let Some(error) = outcome.mutation_error {
                    model.set_operation_outcome(Some(&error));
                } else {
                    model.notice = Some(
                        if outcome.refresh_error.is_some() {
                            "Saved. Status refresh failed; R retries reading only."
                        } else {
                            "Authorization updated."
                        }
                        .to_owned(),
                    );
                }
            }
        }
        cpg_engine::session::Event::MutationFailed => {
            if let Some(error) = model.session.mutation_error() {
                if model.session.launch_outcome().is_none() {
                    model.set_operation_outcome(Some(&error));
                } else {
                    model.notice = Some("Status refresh failed; launch result is retained.".into());
                }
            }
        }
        cpg_engine::session::Event::OperationFinished => {
            if let Some(outcome) = model.session.launch_outcome() {
                model.set_launch_outcome(&outcome);
                let operation = outcome.operation_id;
                if !model.exit_requested && model.refreshed_launch != Some(operation) {
                    model.refreshed_launch = Some(operation);
                    model.session.request_refresh();
                }
            }
        }
    }
    Ok(())
}

fn apply_action(model: &mut Model, action: Action) -> Result<()> {
    // A refused proposal can burn its token before throwing; publish that
    // authoritative state immediately while retaining drafts and results.
    let publish_refusal = |model: &mut Model, error: Error| {
        model.view = model.session.view();
        model.set_operation_outcome(Some(&error));
    };
    match action {
        Action::None => {}
        Action::Quit => model.exit_requested = true,
        Action::CancelAndQuit => {
            model.exit_requested = true;
            if model.session.busy() {
                model.session.cancel();
            }
        }
        Action::Cancel => {
            if model.session.busy() {
                model.session.cancel();
            }
        }
        Action::Refresh => {
            model.session.request_refresh();
        }
        Action::Launch => {
            if let Err(error) = model.session.request_launch(false, "") {
                publish_refusal(model, error);
            }
        }
        Action::OpenEditor => {
            let view = model.session.view();
            model.editor_field = 0;
            model.editor_host = view.proxy.host.clone();
            model.editor_port = view.proxy.port.to_string();
            model.editor_host_cursor = model.editor_host.chars().count();
            model.editor_port_cursor = model.editor_port.chars().count();
            model.editor_error = None;
            model.saving_operation = None;
            model.modal = Modal::ProxyEditor;
        }
        Action::OpenConsent => {
            if let Some(proposal) = model.session.view().consent.clone() {
                drop(proposal);
                model.confirm_buffer.clear();
                model.confirm_scroll = 0;
                model.modal = Modal::Consent;
            }
        }
        Action::OpenRepair => {
            model.confirm_buffer.clear();
            model.confirm_scroll = 0;
            model.modal = Modal::Repair;
        }
        Action::SaveProxy { host, port } => match model.session.request_set_proxy(&host, port) {
            Ok(()) => {
                model.saving_operation = Some(model.session.operation_id());
                model.editor_error = None;
            }
            Err(error) => {
                let operation = model.session.operation_id();
                model.saving_operation = Some(operation);
                model.apply_mutation_outcome(&MutationOutcome {
                    operation_id: operation,
                    mutation_error: Some(error),
                    ..MutationOutcome::default()
                });
            }
        },
        Action::ConfirmConsent(proposal) => {
            if let Err(error) = model.session.request_set_backend_consent(&proposal) {
                publish_refusal(model, error);
            }
        }
        Action::ConfirmRepair(proposal) => {
            if let Err(error) = model.session.request_launch(true, &proposal.token) {
                publish_refusal(model, error);
            }
        }
        Action::CopyDiagnostic => {
            let diagnostic = model.diagnostic_text();
            let copied = cpg_windows::clipboard::copy_text(&diagnostic);
            model.notice = Some(if copied {
                "Diagnostic copied.".to_owned()
            } else {
                "CLIPBOARD_FAILED: Cannot copy the diagnostic; the operation result is retained."
                    .to_owned()
            });
        }
    }
    model.synchronize();
    Ok(())
}

#[allow(dead_code)]
fn sink(_value: Result<Vec<u16>>) {
    let _ = wide_to_utf8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpg_domain::discovery::Desktop;
    use cpg_engine::SessionServices;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn proposal_with_home(home: &str) -> ConsentProposal {
        ConsentProposal {
            token: "token".to_owned(),
            enabled: true,
            home: PathBuf::from(home),
            revision: 1,
        }
    }

    #[test]
    fn wrap_row_counts_match_greedy_word_wrap() {
        assert_eq!(wrapped_rows("aa bb cc", 4), 3);
        assert_eq!(wrapped_rows("aa bb", 8), 1);
        assert_eq!(wrapped_rows("abcdefgh", 4), 2, "hard-break overlong words");
        assert_eq!(wrapped_rows("abcdefghij", 4), 3);
    }

    #[test]
    fn confirmation_gate_requires_fullly_visible_scope() {
        let normal = proposal_with_home(r"C:\Users\luke\.codex");
        assert!(confirmation_fully_visible((100, 30), true, Some(&normal)));
        assert!(confirmation_fully_visible((100, 30), false, None));
        assert!(
            !confirmation_fully_visible((24, 6), true, Some(&normal)),
            "tiny window cannot confirm"
        );
        assert!(
            !confirmation_fully_visible((30, 8), true, Some(&normal)),
            "clipped popup cannot confirm"
        );
        // A long Home path widens the popup (capped at 100) so wide
        // terminals can still confirm, narrow ones cannot.
        let long_home =
            proposal_with_home(&(r"C:\Users\".to_owned() + &"x".repeat(64) + r"\.codex"));
        assert_eq!(long_home.home.display().to_string().len(), 80);
        assert!(confirmation_fully_visible(
            (100, 30),
            true,
            Some(&long_home)
        ));
        assert!(!confirmation_fully_visible(
            (70, 24),
            true,
            Some(&long_home)
        ));
        // CJK Home paths measure by display width, not character count.
        let cjk_home = proposal_with_home(&format!(r"C:\{}", "用".repeat(36)));
        assert!(
            !confirmation_fully_visible((72, 24), true, Some(&cjk_home)),
            "36 CJK characters are 72 columns wide, not 39"
        );
        assert!(confirmation_fully_visible((100, 30), true, Some(&cjk_home)));
    }

    fn model_with_consent(case: &str) -> Model {
        let directory =
            std::env::temp_dir().join(format!("cpg-tui-confirm-{}-{case}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("config.toml");
        cpg_engine::config_store::initialize_config(
            &path,
            &cpg_domain::config::Config::default(),
            false,
        )
        .expect("config");
        let desktop = Desktop {
            registered: true,
            ..Desktop::default()
        };
        let services = SessionServices {
            process: |_| cpg_windows::process_state::ProcessState {
                state: "stopped",
                pid: 0,
            },
            proposal_home: cpg_engine::config_store::consent_home,
            discover: Arc::new(move |_config, _cancel| Ok(desktop.clone())),
            launch: Arc::new(|_config, _path, _options, _cancel, _facts| {
                Ok(cpg_domain::json::Json::Null)
            }),
        };
        let mut session = EngineSession::with_services(
            path,
            services,
            cpg_engine::session::SessionBudgets {
                mutation: Duration::from_millis(500),
                launch: Duration::from_millis(500),
            },
            || "not_elevated",
        )
        .expect("session");
        session.request_refresh();
        while session.busy() {
            session.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(session.view().consent.is_some(), "consent proposal");
        Model::new(session)
    }

    #[test]
    fn typed_yes_authorizes_only_when_scope_is_visible() {
        let mut model = model_with_consent("typed-yes");
        model.modal = Modal::Consent;
        model.confirm_buffer = "YES".to_owned();
        model.viewport = (24, 6);
        let action = model.key(KeyEvent::from(KeyCode::Enter));
        assert!(
            matches!(action, Action::None),
            "clipped scope must not authorize on YES"
        );
        // The same typed YES authorizes once the full scope is visible.
        model.viewport = (100, 30);
        let action = model.key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(action, Action::ConfirmConsent(_)));
    }

    #[test]
    fn editor_supports_cursor_editing() {
        let mut model = model_with_consent("editor");
        model.modal = Modal::ProxyEditor;
        model.editor_field = 0;
        model.editor_host.clear();
        model.editor_host_cursor = 0;
        for character in "localhost".chars() {
            model.key(KeyEvent::from(KeyCode::Char(character)));
        }
        assert_eq!(model.editor_host, "localhost");
        // Left x2 positions the cursor over 't'; typing inserts there.
        model.key(KeyEvent::from(KeyCode::Left));
        model.key(KeyEvent::from(KeyCode::Left));
        model.key(KeyEvent::from(KeyCode::Char('X')));
        assert_eq!(model.editor_host, "localhoXst");
        // Home/Delete removes the first character; End+Backspace the last.
        model.key(KeyEvent::from(KeyCode::Home));
        model.key(KeyEvent::from(KeyCode::Delete));
        assert_eq!(model.editor_host, "ocalhoXst");
        model.key(KeyEvent::from(KeyCode::End));
        model.key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(model.editor_host, "ocalhoXs");
    }

    #[test]
    fn editor_paste_is_single_line_and_gate_aware() {
        let mut model = model_with_consent("paste");
        model.modal = Modal::ProxyEditor;
        model.editor_field = 1;
        model.editor_port.clear();
        model.editor_port_cursor = 0;
        model.editor_paste(
            "1080
8080",
        );
        assert!(
            model.editor_port.is_empty(),
            "multi-line paste is refused whole"
        );
        model.editor_paste("10808");
        assert_eq!(model.editor_port, "10808");
        model.editor_paste("abc");
        assert_eq!(model.editor_port, "10808", "non-digits never paste");
        // The confirmation dialog never accepts pasted text.
        model.modal = Modal::Consent;
        model.confirm_buffer.clear();
        model.editor_paste("YES");
        assert!(model.confirm_buffer.is_empty());
    }

    #[test]
    fn shrink_after_typing_blocks_authorization() {
        let mut model = model_with_consent("shrink");
        model.modal = Modal::Consent;
        model.viewport = (100, 30);
        for character in "YES".chars() {
            model.key(KeyEvent::from(KeyCode::Char(character)));
        }
        // The user typed the confirmation, then shrank the window.
        model.viewport = (30, 8);
        let action = model.key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(action, Action::None));
    }
}
