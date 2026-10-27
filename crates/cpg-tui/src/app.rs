//! The Ratatui application: event loop, key translation, modal state
//! machine and rendering. All decisions about what is allowed come from the
//! engine's `SessionView`; this module only draws it and forwards intents.

use crate::terminal::{TerminalGuard, with_panic_guard};
use cpg_domain::text::{public_text, wide_to_utf8};
use cpg_domain::{Error, Result};
use cpg_engine::{
    ConsentProposal, EngineSession, LaunchOutcome, MutationOutcome, RepairProposal, SessionView,
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::path::Path;
use std::time::Duration;

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

/// Single-line editor field with a character-based cursor (DESIGN.md:
/// Home/End/Left/Right/Backspace/Delete editing; the port field additionally
/// accepts ASCII digits only, enforced by the callers).
#[derive(Clone, Debug, Default)]
struct EditorField {
    text: String,
    cursor: usize,
}

impl EditorField {
    fn set(&mut self, text: &str) {
        self.text = text.to_owned();
        self.cursor = self.text.chars().count();
    }

    fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte_index(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.cursor)
            .map(|(index, _)| index)
            .unwrap_or(self.text.len())
    }

    /// Inserts one character while the field stays under `limit` characters;
    /// returns whether the draft changed.
    fn insert(&mut self, character: char, limit: usize) -> bool {
        if self.char_len() >= limit {
            return false;
        }
        let index = self.byte_index();
        self.text.insert(index, character);
        self.cursor += 1;
        true
    }

    /// Removes the character before the cursor.
    fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let end = self.byte_index();
        let start = self.text[..end]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);
        self.text.drain(start..end);
        self.cursor -= 1;
        true
    }

    /// Removes the character at the cursor.
    fn delete(&mut self) -> bool {
        if self.cursor >= self.char_len() {
            return false;
        }
        let start = self.byte_index();
        let end = self.text[start..]
            .char_indices()
            .nth(1)
            .map(|(offset, _)| start + offset)
            .unwrap_or(self.text.len());
        self.text.drain(start..end);
        true
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_len());
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.char_len();
    }
}

/// The accepted length limits for the editor fields: characters for the
/// host, digits for the port.
const HOST_LIMIT: usize = 255;
const PORT_LIMIT: usize = 5;

struct Model {
    session: EngineSession,
    view: SessionView,
    modal: Modal,
    notice: Option<String>,
    editor_field: u8, // 0 = host, 1 = port
    editor_host: EditorField,
    editor_port: EditorField,
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
            editor_host: EditorField::default(),
            editor_port: EditorField::default(),
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
            let configured = self.view.proxy.url.clone();
            let submitted = receipt.proxy_endpoint.clone().unwrap_or_default();
            self.outcome_facts.push_str(&format!(
                "\nPID: {}\nInstance: {}\nMethod: {}\nProxy delivery: {}\nConfigured: {}\nSubmitted: {}",
                receipt
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                receipt.instance,
                receipt.launch_method,
                receipt.proxy_delivery,
                configured,
                if submitted.is_empty() {
                    "-"
                } else {
                    &submitted
                }
            ));
            if !submitted.is_empty() && submitted != configured {
                // The receipt is a past fact and the view is a present fact:
                // saying so is the only honest way to show both.
                self.outcome_facts
                    .push_str("\nSubmitted endpoint differs from the current configuration.");
                self.outcome_warning = true;
            }
            self.outcome_warning |= receipt.instance != "created";
            if receipt.instance == "reused" {
                self.outcome_message.push_str(
                    " An existing instance was returned. Adoption of this proxy is unconfirmed.",
                );
            } else if receipt.instance == "unknown" {
                self.outcome_message.push_str(
                    " Instance creation is unknown. Refresh status; do not automatically retry.",
                );
            } else if outcome.error.is_none() {
                self.outcome_message = if receipt.launch_method == "native_process" {
                    format!(
                        "Desktop process created with the proxy in its environment ({}).",
                        if submitted.is_empty() {
                            "unknown"
                        } else {
                            &submitted
                        }
                    )
                } else {
                    format!(
                        "Desktop activation returned a new instance with the proxy arguments{}.",
                        if submitted.is_empty() {
                            String::new()
                        } else {
                            format!(" ({submitted})")
                        }
                    )
                };
            }
        }
        if outcome.exceeded_budget {
            self.outcome_facts
                .push_str("\nSession budget exceeded; returned facts are preserved.");
        }
        self.outcome_facts.push_str(
            "\nGuard verifies these delivery facts only: Desktop adoption and network use are not verified.",
        );
    }

    /// Keeps the editor open with the draft and shows the refusal inline,
    /// focusing the offending field when the engine names one.
    fn retain_editor_error(&mut self, error: &Error) {
        self.modal = Modal::ProxyEditor;
        self.editor_error = Some(public_text(&error.message, 1024));
        match error.field.as_deref() {
            Some("proxy.host") => self.editor_field = 0,
            Some("proxy.port") => self.editor_field = 1,
            _ => {}
        }
    }

    /// Bracketed-paste insertion. Windows crossterm delivers pastes as key
    /// events, so Guard never requests bracketed paste mode; this path stays
    /// for terminals that synthesize `Event::Paste` anyway.
    fn paste(&mut self, text: &str) -> Action {
        if self.modal != Modal::ProxyEditor || self.saving_operation.is_some() {
            return Action::None;
        }
        let port_field = self.editor_field == 1;
        let limit = if port_field { PORT_LIMIT } else { HOST_LIMIT };
        let mut changed = false;
        for character in text.chars() {
            if character.is_control() {
                continue;
            }
            if port_field && !character.is_ascii_digit() {
                continue;
            }
            changed |= self.active_editor_field_mut().insert(character, limit);
        }
        if changed {
            self.editor_error = None;
        }
        Action::None
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
                    match outcome.mutation_error.as_ref() {
                        Some(error) => self.retain_editor_error(error),
                        None => {
                            self.modal = Modal::ProxyEditor;
                            self.editor_error =
                                Some("Save did not complete. Your draft is retained.".into());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn key(&mut self, key: KeyEvent) -> Action {
        // Windows reports both press and release for one physical key. Ignore
        // releases before any state change, including across modal transitions;
        // keep press/repeat events for normal typing and held-key editing.
        if key.kind.is_release() {
            return Action::None;
        }
        // Any key supersedes the last inline notice; handlers below may set a
        // fresh one for the refused action.
        self.notice = None;
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
        // Control/Alt character events must never reach the modal handlers as
        // plain letters: Ctrl+L would launch the Desktop, Ctrl+Q would quit
        // and Ctrl+V would insert a literal 'v' into the editor. Shift (for
        // uppercase) stays allowed.
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Char(_))
        {
            return Action::None;
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

    fn editor_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.modal = Modal::None;
                Action::None
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.editor_field = 1 - self.editor_field;
                Action::None
            }
            KeyCode::Enter => match cpg_domain::parse_port_strict(&self.editor_port.text) {
                Ok(port) => Action::SaveProxy {
                    host: self.editor_host.text.clone(),
                    port: port as i64,
                },
                Err(error) => {
                    self.editor_field = 1;
                    self.editor_error = Some(public_text(&error.message, 1024));
                    Action::None
                }
            },
            KeyCode::Left => {
                self.active_editor_field_mut().left();
                Action::None
            }
            KeyCode::Right => {
                self.active_editor_field_mut().right();
                Action::None
            }
            KeyCode::Home => {
                self.active_editor_field_mut().home();
                Action::None
            }
            KeyCode::End => {
                self.active_editor_field_mut().end();
                Action::None
            }
            KeyCode::Backspace => {
                self.edit_field(EditorField::backspace);
                Action::None
            }
            KeyCode::Delete => {
                self.edit_field(EditorField::delete);
                Action::None
            }
            KeyCode::Char(character) => {
                let port_field = self.editor_field == 1;
                let limit = if port_field { PORT_LIMIT } else { HOST_LIMIT };
                if port_field && !character.is_ascii_digit() {
                    return Action::None;
                }
                self.edit_field(|field| field.insert(character, limit));
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Applies one field edit and clears the editor error only when the edit
    /// actually changed the draft.
    fn edit_field(&mut self, edit: impl FnOnce(&mut EditorField) -> bool) {
        if edit(self.active_editor_field_mut()) {
            self.editor_error = None;
        }
    }

    fn active_editor_field_mut(&mut self) -> &mut EditorField {
        if self.editor_field == 0 {
            &mut self.editor_host
        } else {
            &mut self.editor_port
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
                    if self.modal == Modal::Repair {
                        if let Some(repair) = self.session.view().repair.clone() {
                            action = Action::ConfirmRepair(repair);
                            self.modal = Modal::None;
                        }
                    } else if let Some(consent) = self.session.view().consent.clone() {
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
            "\nconfig: {}\nproxy: {} ({})\nplan: {} {}\nlaunch: {}\ndesktop: {}\nentry: {}\ncoverage: {}\n{}",
            self.session.config_path().display(),
            self.view.proxy.url,
            self.view.proxy.state,
            self.view.plan.state,
            self.view.plan.endpoint,
            self.view.launch_method,
            self.view.desktop.display_name,
            self.view.desktop.application_id.as_deref().unwrap_or("-"),
            self.view.coverage.state,
            self.outcome_facts
        ));
        public_text(&text, 4096)
    }
}

/// The engine's own statement about how the displayed endpoint would be
/// delivered, so a configuration that cannot produce activation arguments is
/// visible before a launch is attempted.
fn plan_row(view: &SessionView) -> Line<'static> {
    let (detail, color) = match view.plan.state {
        "available" => (
            format!("via activation arguments · {}", view.plan.endpoint),
            Color::Green,
        ),
        "environment" => (
            format!("via process environment · {}", view.plan.endpoint),
            Color::Green,
        ),
        "unavailable" => (
            format!(
                "not buildable · {}",
                view.plan
                    .error
                    .as_ref()
                    .map(|error| error.code.clone())
                    .unwrap_or_else(|| "unknown".to_owned())
            ),
            Color::Red,
        ),
        _ => ("not confirmed · press R".to_owned(), Color::DarkGray),
    };
    Line::from(vec![
        Span::styled("PLAN     ", Style::default().fg(Color::DarkGray)),
        Span::styled(detail, Style::default().fg(color)),
    ])
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
        let proxy_row = Line::from(vec![
            Span::styled("PROXY    ", Style::default().fg(Color::DarkGray)),
            Span::raw(model.view.proxy.url.clone()),
            Span::styled(
                match model.view.proxy.state {
                    "read" => String::new(),
                    "committed" => " · saved, not re-read".to_owned(),
                    "retained" => " · last good read".to_owned(),
                    other => format!(" · {other}"),
                },
                Style::default().fg(Color::Yellow),
            ),
        ]);
        let mut right_rows: Vec<Line> = vec![proxy_row, plan_row(&model.view)];
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
    if let Some(notice) = &model.notice {
        let notice_row = Rect {
            x: action_bar.x,
            y: action_bar.y + 1,
            width: action_bar.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {notice}"),
                Style::default().fg(Color::Yellow),
            )),
            notice_row,
        );
    }
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

/// One editor row: `label [text]` with a reversed cursor cell on the active
/// field so the character-based cursor position is always visible.
fn field_line(label: &str, field: &EditorField, active: bool) -> Line<'static> {
    let marker = if active { " ◄" } else { "" };
    let mut spans = vec![Span::raw(format!("{label} ["))];
    if active {
        let characters: Vec<char> = field.text.chars().collect();
        let cursor = field.cursor.min(characters.len());
        let before: String = characters[..cursor].iter().collect();
        let at: String = characters
            .get(cursor)
            .map(|character| character.to_string())
            .unwrap_or_else(|| " ".to_owned());
        let after: String = characters[cursor..].iter().collect();
        spans.push(Span::raw(before));
        spans.push(Span::styled(
            at,
            Style::default().add_modifier(Modifier::REVERSED),
        ));
        spans.push(Span::raw(after));
    } else {
        spans.push(Span::raw(field.text.clone()));
    }
    spans.push(Span::raw(format!("]{marker}")));
    Line::from(spans)
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
            let mut lines = vec![
                field_line("Host", &model.editor_host, model.editor_field == 0),
                Line::from(""),
                field_line("Port", &model.editor_port, model.editor_field == 1),
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
                    "Tab switch · ← → Home End · Enter save · Esc cancel"
                },
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Modal::Consent | Modal::Repair => {
            let consent = model.modal == Modal::Consent;
            let rect = popup(
                frame,
                if consent {
                    "Backend Proxy Authorization"
                } else {
                    "Repair Launch"
                },
                72,
                17,
            );
            let inner = rect.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 2,
            });
            let message = if let Some(proposal) =
                model.session.view().consent.as_ref().filter(|_| consent)
            {
                format!(
                    "{}\n{}\n\nOnly Guard's marked block is managed. It affects later Codex processes sharing this Home.",
                    if proposal.enabled {
                        "Authorize HTTP_PROXY/HTTPS_PROXY/NO_PROXY in this exact Home:"
                    } else {
                        "Remove Guard's unchanged proxy block from this exact Home:"
                    },
                    proposal.home.display()
                )
            } else {
                "Stop the shared Codex background server once. This may interrupt other clients' tasks on the same Home.\nThe backend proxy block is prepared before the stop.".to_owned()
            };
            let lines: Vec<Line> = message.lines().map(Line::from).collect();
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((model.confirm_scroll as u16, 0)),
                inner,
            );
            let confirm = Line::from(vec![
                Span::styled(
                    "Type YES to confirm this single operation: ",
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("{}_", model.confirm_buffer),
                    Style::default().bold(),
                ),
            ]);
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
                     \x20   Editor: Tab switch, ← → Home End, Enter save\n\
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
    (|| -> Result<i32> {
        let session = EngineSession::new(config_path.to_path_buf())?;
        let mut model = Model::new(session);
        model.session.request_refresh();
        model.synchronize();
        loop {
            let frame_result = guard.terminal_mut().draw(|frame| render(frame, &model));
            if frame_result.is_err() {
                return Err(Error::new(
                    "CONSOLE_OUTPUT_FAILED",
                    "Cannot render the console interface.",
                ));
            }
            // Poll the session and input with short slices so busy states and
            // timeouts progress. Bracketed paste mode is never requested:
            // Windows crossterm cannot synthesize `Event::Paste`, so enabling
            // it would only inject raw `ESC[200~…ESC[201~` keystrokes into
            // the modal handlers.
            if crossterm::event::poll(Duration::from_millis(20)).unwrap_or(false) {
                let action = crossterm::event::read().ok().and_then(|event| match event {
                    Event::Key(key) => Some(model.key(key)),
                    Event::Paste(text) => Some(model.paste(&text)),
                    _ => None,
                });
                if let Some(action) = action {
                    apply_action(&mut model, action)?;
                }
            }
            // Drive the engine state machine.
            let event = model.session.poll();
            if event != cpg_engine::session::Event::None {
                model.synchronize();
            }
            handle_event(&mut model, event)?;
            model.synchronize();
            if model.exit_requested && !model.session.busy() {
                return Ok(0);
            }
        }
    })()
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
            model.editor_host.set(&view.proxy.host);
            model.editor_port.set(&view.proxy.port.to_string());
            model.editor_error = None;
            model.saving_operation = None;
            model.modal = Modal::ProxyEditor;
        }
        Action::OpenConsent => {
            if model.session.view().consent.is_some() {
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
            // A synchronous refusal (busy, validation) never dispatched a
            // task: keep the draft and show the cause inline instead of
            // fabricating a mutation outcome under a stale operation id.
            Err(error) => model.retain_editor_error(&error),
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
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    /// A model with the editor open; the session is never driven, so no
    /// filesystem or discovery work happens.
    fn editor_model() -> Model {
        let session = EngineSession::new(r"C:\cpg-tui-tests\config.toml".into()).expect("session");
        let mut model = Model::new(session);
        model.modal = Modal::ProxyEditor;
        model.editor_field = 0;
        model.editor_host.set("127.0.0.1");
        model.editor_port.set("10808");
        model.editor_error = None;
        model.saving_operation = None;
        model
    }

    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn with_modifier(code: KeyCode, modifier: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifier)
    }

    fn tap(model: &mut Model, code: KeyCode) {
        for kind in [KeyEventKind::Press, KeyEventKind::Release] {
            assert!(matches!(
                model.key(KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind)),
                Action::None
            ));
        }
    }

    #[test]
    fn opening_editor_does_not_insert_the_shortcut_on_release() {
        let mut model = editor_model();
        model.modal = Modal::None;
        let action = model.key(plain(KeyCode::Char('e')));
        assert!(matches!(action, Action::OpenEditor));
        apply_action(&mut model, action).expect("open editor");
        let host = model.editor_host.text.clone();
        let cursor = model.editor_host.cursor;

        assert!(matches!(
            model.key(KeyEvent::new_with_kind(
                KeyCode::Char('e'),
                KeyModifiers::NONE,
                KeyEventKind::Release
            )),
            Action::None
        ));
        assert_eq!(model.editor_host.text, host);
        assert_eq!(model.editor_host.cursor, cursor);
    }

    #[test]
    fn editor_key_taps_edit_once_and_tab_reaches_the_port() {
        let mut model = editor_model();
        model.editor_host.set("abcd");
        tap(&mut model, KeyCode::Char('X'));
        assert_eq!(model.editor_host.text, "abcdX");
        tap(&mut model, KeyCode::Backspace);
        assert_eq!(model.editor_host.text, "abcd");
        tap(&mut model, KeyCode::Left);
        assert_eq!(model.editor_host.cursor, 3);
        tap(&mut model, KeyCode::Delete);
        assert_eq!(model.editor_host.text, "abc");

        tap(&mut model, KeyCode::Tab);
        assert_eq!(model.editor_field, 1);
        model.editor_port.set("");
        tap(&mut model, KeyCode::Char('8'));
        tap(&mut model, KeyCode::Char('0'));
        assert_eq!(model.editor_port.text, "80");
        tap(&mut model, KeyCode::BackTab);
        assert_eq!(model.editor_field, 0);
        assert_eq!(model.editor_host.text, "abc");
    }

    #[test]
    fn releases_never_submit_dismiss_or_clear_feedback() {
        let mut model = editor_model();
        for modal in [
            Modal::None,
            Modal::ProxyEditor,
            Modal::Consent,
            Modal::Repair,
            Modal::LaunchResult,
            Modal::ErrorResult,
            Modal::Help,
            Modal::QuitWhileBusy,
        ] {
            model.modal = modal;
            model.notice = Some("Proxy saved.".into());
            model.confirm_buffer = "YES".into();
            for code in [
                KeyCode::Enter,
                KeyCode::Esc,
                KeyCode::Char('q'),
                KeyCode::Char('l'),
                KeyCode::Char('y'),
            ] {
                assert!(matches!(
                    model.key(KeyEvent::new_with_kind(
                        code,
                        KeyModifiers::NONE,
                        KeyEventKind::Release
                    )),
                    Action::None
                ));
                assert_eq!(model.modal, modal);
                assert_eq!(model.notice.as_deref(), Some("Proxy saved."));
                assert_eq!(model.confirm_buffer, "YES");
            }
        }
    }

    #[test]
    fn editor_retains_key_repeat_for_typing_and_cursor_editing() {
        let mut model = editor_model();
        model.editor_host.set("");
        model.key(plain(KeyCode::Char('a')));
        for code in [KeyCode::Char('a'), KeyCode::Char('a'), KeyCode::Left] {
            assert!(matches!(
                model.key(KeyEvent::new_with_kind(
                    code,
                    KeyModifiers::NONE,
                    KeyEventKind::Repeat
                )),
                Action::None
            ));
        }
        assert_eq!(model.editor_host.text, "aaa");
        assert_eq!(model.editor_host.cursor, 2);
        model.key(KeyEvent::new_with_kind(
            KeyCode::Backspace,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ));
        assert_eq!(model.editor_host.text, "aa");
        assert_eq!(model.editor_host.cursor, 1);
    }

    #[test]
    fn control_and_alt_shortcuts_never_act_as_plain_letters() {
        let mut model = editor_model();
        model.modal = Modal::None;
        assert!(matches!(model.key(plain(KeyCode::Char('q'))), Action::Quit));

        let mut model = editor_model();
        model.modal = Modal::None;
        for key in [
            with_modifier(KeyCode::Char('q'), KeyModifiers::CONTROL),
            with_modifier(KeyCode::Char('l'), KeyModifiers::CONTROL),
            with_modifier(KeyCode::Char('e'), KeyModifiers::CONTROL),
            with_modifier(KeyCode::Char('d'), KeyModifiers::CONTROL),
            with_modifier(KeyCode::Char('b'), KeyModifiers::CONTROL),
            with_modifier(KeyCode::Char('e'), KeyModifiers::ALT),
        ] {
            assert!(matches!(model.key(key), Action::None));
        }
        assert!(!model.exit_requested);
        assert_eq!(model.modal, Modal::None);
    }

    #[test]
    fn editor_ignores_control_characters_and_filters_pastes() {
        let mut model = editor_model();
        model.key(with_modifier(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(model.editor_host.text, "127.0.0.1");
        model.paste("ab\r\ncd");
        assert_eq!(model.editor_host.text, "127.0.0.1abcd");
        model.key(plain(KeyCode::Tab));
        model.editor_port.set("");
        model.paste("12x34");
        assert_eq!(model.editor_port.text, "1234");

        // Pastes outside the editor modal change nothing.
        let mut model = editor_model();
        model.modal = Modal::None;
        model.paste("x");
        assert_eq!(model.editor_host.text, "127.0.0.1");
    }

    #[test]
    fn editor_cursor_editing_matches_design() {
        let mut model = editor_model();
        model.editor_host.set("abcd");
        model.key(plain(KeyCode::Home));
        assert_eq!(model.editor_host.cursor, 0);
        model.key(plain(KeyCode::Char('X')));
        assert_eq!(model.editor_host.text, "Xabcd");
        model.key(plain(KeyCode::Delete));
        assert_eq!(model.editor_host.text, "Xbcd");
        model.key(plain(KeyCode::End));
        model.key(plain(KeyCode::Backspace));
        assert_eq!(model.editor_host.text, "Xbc");
        model.key(plain(KeyCode::Left));
        model.key(plain(KeyCode::Char('!')));
        assert_eq!(model.editor_host.text, "Xb!c");
        model.key(plain(KeyCode::Home));
        model.key(plain(KeyCode::Backspace));
        assert_eq!(model.editor_host.text, "Xb!c");
        model.key(plain(KeyCode::End));
        model.key(plain(KeyCode::Right));
        assert_eq!(model.editor_host.cursor, model.editor_host.char_len());
    }

    #[test]
    fn port_field_accepts_digits_only_and_caps_length() {
        let mut model = editor_model();
        model.editor_field = 1;
        model.editor_port.set("");
        for character in ['1', 'a', '2', '3', '4', '5', '6'] {
            model.key(plain(KeyCode::Char(character)));
        }
        assert_eq!(model.editor_port.text, "12345");
    }

    #[test]
    fn enter_submits_only_with_a_strict_port_and_keeps_the_draft() {
        let mut model = editor_model();
        model.editor_port.set("99999");
        assert!(matches!(model.key(plain(KeyCode::Enter)), Action::None));
        assert!(model.editor_error.is_some());
        assert_eq!(model.editor_field, 1, "port error focuses the port field");
        assert_eq!(model.editor_port.text, "99999", "draft retained");
        model.editor_port.set("10808");
        match model.key(plain(KeyCode::Enter)) {
            Action::SaveProxy { host, port } => {
                assert_eq!(host, "127.0.0.1");
                assert_eq!(port, 10808);
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn tab_switches_between_fields() {
        let mut model = editor_model();
        tap(&mut model, KeyCode::Tab);
        assert_eq!(model.editor_field, 1);
        tap(&mut model, KeyCode::BackTab);
        assert_eq!(model.editor_field, 0);
    }

    #[test]
    fn editing_after_an_error_clears_it() {
        let mut model = editor_model();
        model.editor_error = Some("prior failure".into());
        model.editor_host.set("127.0.0.");
        model.key(plain(KeyCode::Char('1')));
        assert!(model.editor_error.is_none());
    }

    #[test]
    fn editor_field_cursor_is_character_based() {
        let mut field = EditorField::default();
        field.set("héllo");
        assert_eq!(field.char_len(), 5);
        field.left();
        field.left();
        assert_eq!(field.cursor, 3);
        assert!(field.insert('X', HOST_LIMIT));
        assert_eq!(field.text, "hélXlo");
        assert!(field.backspace());
        assert_eq!(field.text, "héllo");
        field.home();
        assert!(field.delete());
        assert_eq!(field.text, "éllo");
        assert!(!field.backspace());
        assert_eq!(field.text, "éllo");
    }

    #[test]
    fn notices_render_in_the_action_bar() {
        let mut model = editor_model();
        model.modal = Modal::None;
        model.notice = Some("Proxy saved.".to_owned());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).expect("terminal");
        terminal.draw(|frame| render(frame, &model)).expect("draw");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect();
        assert!(
            rendered.contains("Proxy saved."),
            "notice visible: {rendered}"
        );
    }

    #[test]
    fn notices_clear_on_the_next_key_press() {
        let mut model = editor_model();
        model.modal = Modal::None;
        model.notice = Some("Proxy saved.".to_owned());
        model.key(plain(KeyCode::Char('q')));
        assert!(model.notice.is_none());
    }

    fn launched_outcome(method: &str, delivery: &str, endpoint: &str) -> LaunchOutcome {
        LaunchOutcome {
            operation_id: 7,
            receipt: Some(cpg_engine::LaunchReceipt {
                pid: Some(4242),
                launch_method: method.to_owned(),
                activation_state: "activated".to_owned(),
                instance: "created".to_owned(),
                proxy_delivery: delivery.to_owned(),
                backend_proxy_config: "not_authorized".to_owned(),
                daemon_preparation: "not_requested".to_owned(),
                proxy_endpoint: Some(endpoint.to_owned()),
                target_elevation: Some(false),
            }),
            ..LaunchOutcome::default()
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn the_result_modal_separates_the_submitted_endpoint_from_the_current_one() {
        let mut model = editor_model();
        model.view.proxy.url = "http://127.0.0.1:10809".to_owned();
        model.set_launch_outcome(&launched_outcome(
            "appmodel_activation",
            "activation_arguments",
            "http://127.0.0.1:10809",
        ));
        assert!(
            model
                .outcome_facts
                .contains("Configured: http://127.0.0.1:10809\nSubmitted: http://127.0.0.1:10809"),
            "{}",
            model.outcome_facts
        );
        assert!(model.outcome_facts.contains("Method: appmodel_activation"));
        assert!(
            model.outcome_facts.contains("PID: 4242\nInstance: created"),
            "{}",
            model.outcome_facts
        );
        assert!(
            !model.outcome_warning,
            "the receipt matches the current view"
        );
        assert_eq!(
            model.outcome_message,
            "Desktop activation returned a new instance with the proxy arguments (http://127.0.0.1:10809)."
        );

        // A later edit makes the retained receipt a past fact: it stays
        // visible, but the divergence is stated instead of hidden.
        model.view.proxy.url = "http://127.0.0.1:10810".to_owned();
        model.set_launch_outcome(&launched_outcome(
            "appmodel_activation",
            "activation_arguments",
            "http://127.0.0.1:10809",
        ));
        assert!(model.outcome_warning);
        assert!(
            model
                .outcome_facts
                .contains("Submitted endpoint differs from the current configuration."),
            "{}",
            model.outcome_facts
        );
    }

    #[test]
    fn a_reused_or_unknown_instance_is_never_reported_as_adoption() {
        for (instance, expected) in [
            (
                "reused",
                "An existing instance was returned. Adoption of this proxy is unconfirmed.",
            ),
            (
                "unknown",
                "Instance creation is unknown. Refresh status; do not automatically retry.",
            ),
        ] {
            let mut model = editor_model();
            let mut outcome = launched_outcome(
                "appmodel_activation",
                "activation_arguments",
                "http://127.0.0.1:10809",
            );
            outcome.receipt.as_mut().expect("receipt").instance = instance.to_owned();
            model.set_launch_outcome(&outcome);
            assert!(model.outcome_warning, "{instance} reads as a warning");
            assert!(
                model.outcome_message.contains(expected),
                "{}",
                model.outcome_message
            );
            assert!(
                !model
                    .outcome_message
                    .contains("returned a new instance with the proxy arguments"),
                "{}",
                model.outcome_message
            );
        }
    }

    #[test]
    fn the_native_receipt_is_not_described_as_an_activation() {
        let mut model = editor_model();
        model.view.proxy.url = "http://127.0.0.1:10809".to_owned();
        model.set_launch_outcome(&launched_outcome(
            "native_process",
            "process_environment",
            "http://127.0.0.1:10809",
        ));
        assert_eq!(
            model.outcome_message,
            "Desktop process created with the proxy in its environment (http://127.0.0.1:10809)."
        );
        assert!(
            model
                .outcome_facts
                .contains("Guard verifies these delivery facts only"),
            "{}",
            model.outcome_facts
        );
    }

    #[test]
    fn the_plan_row_states_the_channel_or_the_refusal() {
        let mut view = SessionView::default();
        assert_eq!(
            line_text(&plan_row(&view)),
            "PLAN     not confirmed · press R"
        );
        view.plan = cpg_engine::session::LaunchPlanView {
            state: "available",
            endpoint: "http://127.0.0.1:10809".to_owned(),
            error: None,
        };
        assert_eq!(
            line_text(&plan_row(&view)),
            "PLAN     via activation arguments · http://127.0.0.1:10809"
        );
        view.plan = cpg_engine::session::LaunchPlanView {
            state: "environment",
            endpoint: "http://127.0.0.1:10809".to_owned(),
            error: None,
        };
        assert_eq!(
            line_text(&plan_row(&view)),
            "PLAN     via process environment · http://127.0.0.1:10809"
        );
        view.plan = cpg_engine::session::LaunchPlanView {
            state: "unavailable",
            endpoint: String::new(),
            error: Some(Error::new("PROXY_BYPASS_UNSUPPORTED", "no bypass spelling")),
        };
        assert_eq!(
            line_text(&plan_row(&view)),
            "PLAN     not buildable · PROXY_BYPASS_UNSUPPORTED"
        );
    }

    #[test]
    fn a_wide_terminal_renders_the_plan_row() {
        let mut model = editor_model();
        model.modal = Modal::None;
        model.view.proxy.url = "http://127.0.0.1:10809".to_owned();
        model.view.plan = cpg_engine::session::LaunchPlanView {
            state: "available",
            endpoint: "http://127.0.0.1:10809".to_owned(),
            error: None,
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).expect("terminal");
        terminal.draw(|frame| render(frame, &model)).expect("draw");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect();
        assert!(
            rendered.contains("PLAN") && rendered.contains("via activation arguments"),
            "plan row visible: {rendered}"
        );
    }

    #[test]
    fn the_copied_diagnostic_names_the_file_the_endpoint_and_the_plan() {
        let mut model = editor_model();
        model.view.proxy = cpg_engine::session::ProxyView {
            url: "http://127.0.0.1:10809".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 10809,
            state: "retained",
        };
        model.view.plan = cpg_engine::session::LaunchPlanView {
            state: "unknown",
            endpoint: String::new(),
            error: None,
        };
        let text = model.diagnostic_text();
        assert!(
            text.contains(r"config: C:\cpg-tui-tests\config.toml"),
            "{text}"
        );
        assert!(
            text.contains("proxy: http://127.0.0.1:10809 (retained)"),
            "{text}"
        );
        assert!(text.contains("plan: unknown "), "{text}");
    }
}
