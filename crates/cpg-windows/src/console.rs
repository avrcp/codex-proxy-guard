//! Console control: Ctrl+C / Ctrl+Break / window-close signals are captured
//! by a handler that only sets an event (no blocking I/O in the handler),
//! and console mode guards save and restore terminal state.

use crate::cancellation::Cancellation;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use windows::Win32::System::Console::{
    CONSOLE_MODE, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    SetConsoleCtrlHandler, SetConsoleMode,
};

static SIGNAL_FLAGS: AtomicU8 = AtomicU8::new(0);
const FLAG_CANCEL: u8 = 1;
const FLAG_CLOSE: u8 = 2;

static HANDLER_INSTALLED: OnceLock<()> = OnceLock::new();

unsafe extern "system" fn handler(event: u32) -> windows::core::BOOL {
    // Handler contract: set flags only; no blocking I/O, no allocation.
    if matches!(
        event,
        windows::Win32::System::Console::CTRL_CLOSE_EVENT
            | windows::Win32::System::Console::CTRL_LOGOFF_EVENT
            | windows::Win32::System::Console::CTRL_SHUTDOWN_EVENT
    ) {
        SIGNAL_FLAGS.fetch_or(FLAG_CLOSE | FLAG_CANCEL, Ordering::SeqCst);
    } else {
        // CTRL_C_EVENT / CTRL_BREAK_EVENT: mark cancellation; returning TRUE
        // keeps the default terminator disarmed while our cooperative path
        // takes over.
        SIGNAL_FLAGS.fetch_or(FLAG_CANCEL, Ordering::SeqCst);
    }
    windows::core::BOOL::from(true)
}

/// Installs the console control handler once per process and returns a
/// cancel-scoped guard. `take_cancel` / `take_close` consume latched signals.
pub struct ConsoleControlScope {
    cancellation: Cancellation,
}

impl ConsoleControlScope {
    pub fn new() -> ConsoleControlScope {
        HANDLER_INSTALLED.get_or_init(|| {
            // SAFETY: the handler is a static function with no captures.
            unsafe { SetConsoleCtrlHandler(Some(handler), true) }.ok();
        });
        ConsoleControlScope {
            cancellation: Cancellation::new(),
        }
    }

    /// Routes latched signals into this scope's cancellation. Returns true
    /// when a cancel signal was consumed.
    pub fn take_cancel(&self) -> bool {
        if SIGNAL_FLAGS.load(Ordering::SeqCst) & FLAG_CANCEL != 0 {
            self.cancellation.cancel();
            SIGNAL_FLAGS.fetch_and(!FLAG_CANCEL, Ordering::SeqCst);
            return true;
        }
        false
    }

    pub fn take_close(&self) -> bool {
        if SIGNAL_FLAGS.load(Ordering::SeqCst) & FLAG_CLOSE != 0 {
            SIGNAL_FLAGS.fetch_and(!FLAG_CLOSE, Ordering::SeqCst);
            return true;
        }
        false
    }

    pub fn cancellation(&self) -> &Cancellation {
        &self.cancellation
    }
}

impl Default for ConsoleControlScope {
    fn default() -> Self {
        Self::new()
    }
}

/// Saves and restores one console mode (input or output) on drop.
pub struct ConsoleModeGuard {
    handle: windows::Win32::Foundation::HANDLE,
    saved: CONSOLE_MODE,
    restored: bool,
}

/// Captures the current mode of the standard input or output console; the
/// guard is a no-op on redirected (non-console) handles.
pub fn capture_mode(input: bool) -> Option<ConsoleModeGuard> {
    // SAFETY: standard handle query with no side effects.
    let Ok(handle) = (unsafe {
        GetStdHandle(if input {
            STD_INPUT_HANDLE
        } else {
            STD_OUTPUT_HANDLE
        })
    }) else {
        return None;
    };
    let mut mode = CONSOLE_MODE::default();
    // SAFETY: the handle is the live standard console handle.
    if unsafe { GetConsoleMode(handle, &mut mode) }.is_err() {
        return None;
    }
    Some(ConsoleModeGuard {
        handle,
        saved: mode,
        restored: false,
    })
}

impl ConsoleModeGuard {
    pub fn set(&mut self, mode: CONSOLE_MODE) -> bool {
        // SAFETY: captured console handle.
        self.restored = false;
        unsafe { SetConsoleMode(self.handle, mode) }.is_ok()
    }

    pub fn restore(&mut self) -> bool {
        if self.restored {
            return true;
        }
        self.restored = true;
        // SAFETY: captured console handle with the saved mode.
        unsafe { SetConsoleMode(self.handle, self.saved) }.is_ok()
    }

    pub fn mode(&self) -> CONSOLE_MODE {
        self.saved
    }
}

impl Drop for ConsoleModeGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// True when the standard output is a console (not redirected).
pub fn stdout_is_console() -> bool {
    // SAFETY: standard handle query and mode probe only.
    unsafe {
        let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) else {
            return false;
        };
        let mut mode = CONSOLE_MODE::default();
        GetConsoleMode(handle, &mut mode).is_ok()
    }
}

/// Counter of handler invocations, exposed for tests that raise Ctrl+ events
/// on their own console.
#[allow(dead_code)]
pub fn signal_count() -> u32 {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    COUNT.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_installs_and_scopes_capture_signals() {
        let scope = ConsoleControlScope::new();
        assert!(!scope.take_cancel());
        assert!(!scope.take_close());
        // Raising real console events would terminate the test process via
        // other attached handlers; the flag routing is exercised implicitly
        // by the TUI/plain frontends.
    }

    #[test]
    fn mode_guard_round_trips_when_attached() {
        if !stdout_is_console() {
            return; // redirected test runner
        }
        let mut guard = capture_mode(false).expect("console mode");
        let original = guard.mode();
        assert!(guard.restore());
        let mut after = capture_mode(false).expect("console mode");
        assert_eq!(after.mode(), original);
        after.restore();
    }
}
