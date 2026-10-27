//! Frontends: the Ratatui interactive console and the plain line-mode
//! fallback. The frontends never recompute engine business rules; they
//! render the typed `SessionView` and translate keys into session calls.

mod app;
mod plain;
mod terminal;

// The launch facts are stated once, in these words, by every frontend.
pub use app::{DELIVERY_LIMITATION, DIVERGED_ENDPOINT, NO_RECEIPT, dash};

use cpg_domain::Result;
use std::path::Path;

/// Interactive entry: `plain` selects line mode; VT unavailability or
/// terminal-guard failure falls back to plain mode like the C++ product.
pub fn run_console(config_path: &Path, plain: bool) -> Result<i32> {
    if plain {
        return plain::run(config_path);
    }
    match terminal::supports_terminal() {
        true => match app::run(config_path) {
            Ok(code) => Ok(code),
            Err(error)
                if error.code == "CONSOLE_OUTPUT_FAILED"
                    || error.code == "CONSOLE_INPUT_FAILED" =>
            {
                // Terminal guard failures degrade to the plain console.
                plain::run(config_path)
            }
            Err(error) => Err(error),
        },
        false => plain::run(config_path),
    }
}
