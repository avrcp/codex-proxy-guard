use std::io::{self, IsTerminal, Stdout};

use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use proxy_guard_core::AlternateScreen;
use ratatui::{Terminal, backend::CrosstermBackend};

pub type GuardTerminal = Terminal<CrosstermBackend<Stdout>>;

pub struct TerminalManager {
    terminal: GuardTerminal,
    alternate_screen: bool,
    raw_mode: bool,
}

impl TerminalManager {
    pub fn enter(preference: AlternateScreen) -> io::Result<Self> {
        let mut stdout = io::stdout();
        let alternate_screen = match preference {
            AlternateScreen::Always => true,
            AlternateScreen::Never => false,
            AlternateScreen::Auto => stdout.is_terminal(),
        };
        enable_raw_mode()?;
        if alternate_screen {
            if let Err(error) = execute!(stdout, EnterAlternateScreen) {
                let _ = disable_raw_mode();
                return Err(error);
            }
        }
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self {
            terminal,
            alternate_screen,
            raw_mode: true,
        })
    }

    pub fn terminal_mut(&mut self) -> &mut GuardTerminal {
        &mut self.terminal
    }

    fn restore(&mut self) -> io::Result<()> {
        self.terminal.show_cursor()?;
        if self.alternate_screen {
            execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
            self.alternate_screen = false;
        }
        if self.raw_mode {
            disable_raw_mode()?;
            self.raw_mode = false;
        }
        Ok(())
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
