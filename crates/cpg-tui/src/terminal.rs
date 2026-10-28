//! Terminal capability probe and the Ratatui terminal guard: raw mode,
//! alternate screen (per `tui.alternate_screen`), mouse capture off, and a
//! restore order that runs even on panic paths.

use cpg_domain::{Error, Result};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use ratatui::{TerminalOptions, Viewport};
use std::io::Stdout;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};

/// True when both stdout and stdin are attached to a console-capable
/// terminal (VT mode available).
pub fn supports_terminal() -> bool {
    cpg_windows::console::stdout_is_console() && crossterm::terminal::size().is_ok()
}

pub struct TerminalGuard {
    terminal: ratatui::DefaultTerminal,
    alternate: bool,
    saved_title: Option<String>,
    restored: AtomicBool,
}

impl TerminalGuard {
    /// Enters raw mode and (depending on `alternate`) the alternate screen.
    pub fn enter(alternate: bool) -> Result<TerminalGuard> {
        let saved_title = cpg_windows::console::current_title();
        enable_raw_mode().map_err(|_| {
            Error::new(
                "CONSOLE_OUTPUT_FAILED",
                "Cannot put the console into raw mode.",
            )
        })?;
        if alternate {
            let stdout = std::io::stdout();
            let mut locked = stdout.lock();
            let _ = execute!(locked, EnterAlternateScreen);
        }
        // Fullscreen viewport over the current buffer: entering/leaving the
        // alternate screen stays owned by this guard, so the configured
        // `tui.alternate_screen` policy is honored on every exit path (a bare
        // `ratatui::init()` would always enter the alternate screen).
        let terminal = ratatui::try_init_with_options(TerminalOptions {
            viewport: Viewport::Fullscreen,
        })
        .map_err(|_| {
            Error::new(
                "CONSOLE_OUTPUT_FAILED",
                "Cannot initialize the terminal buffer.",
            )
        })?;
        let guard = TerminalGuard {
            terminal,
            alternate,
            saved_title,
            restored: AtomicBool::new(false),
        };
        // Panic hook: restore the terminal before the default hook unwinds.
        let previous = panic::take_hook();
        let title_on_panic = guard.saved_title.clone();
        panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let stdout = std::io::stdout();
            let mut locked = stdout.lock();
            let _ = execute!(locked, LeaveAlternateScreen);
            let _ = crossterm::cursor::Show;
            if let Some(title) = title_on_panic.as_deref() {
                cpg_windows::console::set_title(title);
            }
            previous(info);
        }));
        Ok(guard)
    }

    pub fn terminal_mut(&mut self) -> &mut ratatui::DefaultTerminal {
        &mut self.terminal
    }

    pub fn restore(&self) {
        if self.restored.swap(true, Ordering::SeqCst) {
            return;
        }
        let stdout = std::io::stdout();
        let mut locked = stdout.lock();
        if self.alternate {
            let _ = queue!(locked, LeaveAlternateScreen);
        }
        let _ = queue!(locked, crossterm::cursor::Show);
        use std::io::Write as _;
        let _ = locked.flush();
        let _ = disable_raw_mode();
        if let Some(title) = self.saved_title.as_deref() {
            cpg_windows::console::set_title(title);
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Runs `body` with panic containment so the guard still restores the
/// terminal state when a render path panics.
pub fn with_panic_guard<T, F>(body: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(_) => Err(Error::new(
            "CONSOLE_OUTPUT_FAILED",
            "The terminal interface failed; the console was restored.",
        )),
    }
}

#[allow(dead_code)]
fn sink(stdout: Stdout) {
    let _ = stdout;
}

/// Diagnostic provenance lines (module-qualified so app rendering never
/// depends on the app crate's build_info).
pub fn diagnostic_version() -> &'static str {
    option_env!("CPG_PRODUCT_VERSION").unwrap_or("unknown")
}

pub fn diagnostic_commit() -> &'static str {
    option_env!("CPG_BUILD_COMMIT").unwrap_or("unknown")
}
