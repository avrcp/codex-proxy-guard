//! Test-only fixture binary. It imitates the two child roles Guard creates so
//! integration tests can drive the launch pipeline without a real Desktop or
//! Codex CLI:
//!
//! - `fake-codex-cli app-server daemon stop` (the lifecycle helper): behavior
//!   is controlled through `FAKE_CLI_*` environment variables.
//! - any other invocation (the Desktop stand-in): touches the file named by
//!   `FAKE_DESKTOP_TOUCH` and exits immediately.
//!
//! Never packaged into the portable product; `scripts/build-portable.ps1`
//! copies only the `codex-proxy-guard` binary.

use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    process, thread,
    time::{Duration, Instant},
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let is_stop_helper =
        args.len() == 3 && args[0] == "app-server" && args[1] == "daemon" && args[2] == "stop";
    if is_stop_helper {
        run_stop_helper();
    } else {
        touch("FAKE_DESKTOP_TOUCH");
        process::exit(0);
    }
}

fn run_stop_helper() {
    touch("FAKE_CLI_TOUCH");
    if let Some(wait_file) = env::var_os("FAKE_CLI_WAIT_FILE") {
        wait_until_exists(PathBuf::from(wait_file));
    }
    if let Ok(text) = env::var("FAKE_CLI_STDOUT") {
        print!("{text}");
    }
    if let Ok(spec) = env::var("FAKE_CLI_STDOUT_BYTES") {
        emit_padding(&spec, true);
    }
    if let Ok(count) = env::var("FAKE_CLI_STDOUT_SPACES") {
        emit_spaces(&count, true);
    }
    if let Ok(hex) = env::var("FAKE_CLI_STDOUT_HEX") {
        emit_hex(&hex);
    }
    let _ = std::io::stdout().flush();
    if let Ok(text) = env::var("FAKE_CLI_STDERR") {
        eprint!("{text}");
    }
    if let Ok(spec) = env::var("FAKE_CLI_STDERR_BYTES") {
        emit_padding(&spec, false);
    }
    if let Ok(count) = env::var("FAKE_CLI_STDERR_SPACES") {
        emit_spaces(&count, false);
    }
    let _ = std::io::stderr().flush();
    if let Ok(millis) = env::var("FAKE_CLI_SLEEP_MS")
        && let Ok(millis) = millis.parse::<u64>()
    {
        thread::sleep(Duration::from_millis(millis));
    }
    let code = env::var("FAKE_CLI_EXIT")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    process::exit(code);
}

fn touch(variable: &str) {
    if let Some(path) = env::var_os(variable) {
        let _ = fs::write(PathBuf::from(path), b"1");
    }
}

fn wait_until_exists(path: PathBuf) {
    // File-based barrier for tests; gives up after 60 s so a lost test never
    // hangs forever (Guard kills the helper on cancel/timeout anyway).
    let deadline = Instant::now() + Duration::from_secs(60);
    while !path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

/// `FAKE_CLI_STDOUT_BYTES="<char> <count>"` appends `count` copies of `char`.
fn emit_padding(spec: &str, stdout: bool) {
    let mut parts = spec.split_ascii_whitespace();
    let filler = parts.next().unwrap_or(" ").chars().next().unwrap_or(' ');
    let count: usize = parts
        .next()
        .and_then(|count| count.parse().ok())
        .unwrap_or(0);
    let chunk: String = std::iter::repeat_n(filler, 1024).collect();
    let mut remaining = count;
    while remaining > 0 {
        let take = remaining.min(chunk.len());
        let slice = &chunk[..take];
        if stdout {
            print!("{slice}");
        } else {
            eprint!("{slice}");
        }
        remaining -= take;
    }
}

/// Emits `count` spaces so tests can pad JSON to an exact size while keeping
/// the output valid (trailing whitespace after one JSON object).
fn emit_spaces(count: &str, stdout: bool) {
    let count: usize = count.parse().unwrap_or(0);
    let chunk = " ".repeat(1024);
    let mut remaining = count;
    while remaining > 0 {
        let take = remaining.min(chunk.len());
        let slice = &chunk[..take];
        if stdout {
            print!("{slice}");
        } else {
            eprint!("{slice}");
        }
        remaining -= take;
    }
}

fn emit_hex(hex: &str) {
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .filter_map(|index| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok())
        .collect();
    if !bytes.is_empty() {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(&bytes);
        let _ = stdout.flush();
    }
}
