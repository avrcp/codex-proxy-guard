//! Terminal capability probe and the Ratatui terminal guard. Initialization
//! goes through exactly one authoritative Ratatui entry point (which also
//! installs Ratatui's panic hook); restore mirrors it symmetrically for the
//! alternate screen, raw mode, keyboard enhancement and cursor visibility.

use cpg_domain::{Error, Result};
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
use crossterm::{execute, queue};
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
    restored: AtomicBool,
}

impl TerminalGuard {
    /// Enters raw mode and (depending on `alternate`) the alternate screen
    /// through Ratatui's own initialization:
    /// - `try_init` enables raw mode and enters the alternate screen;
    /// - `try_init_with_options` enables raw mode only, so
    ///   `tui.alternate_screen=never` is honored for real.
    ///
    /// Both return a `Result` and install Ratatui's restoring panic hook, so
    /// no manual raw/alternate-screen toggling runs alongside them.
    pub fn enter(alternate: bool) -> Result<TerminalGuard> {
        let terminal = if alternate {
            ratatui::try_init()
        } else {
            ratatui::try_init_with_options(ratatui::TerminalOptions::default())
        }
        .map_err(|_| {
            Error::new(
                "CONSOLE_OUTPUT_FAILED",
                "Cannot initialize the terminal interface.",
            )
        })?;
        {
            let stdout = std::io::stdout();
            let mut locked = stdout.lock();
            // Sent for real (and popped on restore); terminals without the
            // protocol ignore the sequence.
            let _ = execute!(
                locked,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
        }
        Ok(TerminalGuard {
            terminal,
            alternate,
            restored: AtomicBool::new(false),
        })
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
        let _ = execute!(locked, PopKeyboardEnhancementFlags);
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
