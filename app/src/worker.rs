//! The hidden activation worker (`internal-activate-package`): one bounded
//! stdin request (≤16 KiB, 30 s), schema-v1 validation, COM activation with
//! AO_NONE and identity observation, one bounded stdout receipt.

use cpg_domain::json::json_dump;
use cpg_domain::text::public_text;
use cpg_domain::worker::{validate_activation_receipt, validate_activation_request};
use cpg_windows::console::{BoundedRead, read_stdin_bounded};
use std::io::Write;
use std::time::Duration;

const REQUEST_LIMIT: usize = 16 * 1024;

fn fail(code: &str, message: &str) -> ! {
    let text = format!("{code}: {}\n", public_text(message, 4096));
    let stderr = std::io::stderr();
    let mut error = stderr.lock();
    let _ = error.write_all(text.as_bytes());
    let _ = error.flush();
    std::process::exit(1);
}

/// Reads the request from stdin with a hard 30-second budget and the 16 KiB
/// ceiling. The bounded adapter polls pipes (an idle pipe would ignore any
/// timeout wrapped around a blocking read), refuses consoles and enforces
/// the byte limit; redirected or piped input only.
fn read_request() -> Vec<u8> {
    match read_stdin_bounded(REQUEST_LIMIT, Duration::from_secs(30)) {
        Ok(BoundedRead::Bytes(bytes)) => bytes,
        Ok(BoundedRead::Eof) => Vec::new(),
        Err(error) => fail(
            "APPX_ACTIVATION_PROTOCOL_INVALID",
            &format!("Worker request read failed: {error}"),
        ),
    }
}

pub fn activation_worker() -> i32 {
    let bytes = read_request();
    let Ok(request) = validate_activation_request(&bytes) else {
        fail(
            "APPX_ACTIVATION_PROTOCOL_INVALID",
            "Invalid activation request",
        );
    };
    if let Err(error) = cpg_windows::elevation::require_not_elevated() {
        fail(&error.code, &error.message);
    }
    let receipt = cpg_windows::activation::activate_and_observe(&request);
    // Validate our own receipt shape before emitting it (the parent re-checks
    // too; a malformed receipt must never leave the worker).
    if validate_activation_receipt(json_dump(&receipt).as_bytes()).is_err() {
        fail(
            "APPX_ACTIVATION_PROTOCOL_INVALID",
            "Activation receipt failed its own shape validation",
        );
    }
    let output = format!("{}\n", json_dump(&receipt));
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if out.write_all(output.as_bytes()).is_err() || out.flush().is_err() {
        return 1;
    }
    0
}
