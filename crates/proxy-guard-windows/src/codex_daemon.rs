//! Codex daemon compatibility adapter, used only by the explicitly authorized
//! repair launch.
//!
//! Normal launches never resolve the Codex CLI and never run daemon commands.
//! When the user confirms a repair launch, Guard resolves the official CLI from
//! trusted sources (explicit override, the CODEX_HOME package layouts, or
//! controlled PATH directories — never the current directory) and invokes the
//! public `codex app-server daemon stop` lifecycle command once. That stop may
//! interrupt tasks shared with other CLI / IDE / remote clients, which is why
//! it requires single-use confirmation.
//!
//! Guard never starts, restarts, updates, or monitors the daemon, never
//! connects to the app-server socket or any private IPC, and never terminates
//! Codex processes directly — except its own short-lived helper child, which is
//! killed and reaped when its bounded operation times out or is cancelled.

use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use proxy_guard_core::{DaemonPreparation, GuardConfig, redact_text};
use serde::Deserialize;
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

/// Total budget for the explicit repair stop. The official daemon allows a
/// 0–300 s shutdown grace period plus lifecycle-lock waiting, so this is a
/// generous project-side ceiling, not a claim about Codex's own limits.
/// Cancellation stays available for the whole window.
pub const DAEMON_STOP_TOTAL_BUDGET: Duration = Duration::from_secs(720);
/// Budget for terminating and reaping Guard's own short-lived helper child
/// after a timeout or cancellation.
pub const HELPER_CLEANUP_BUDGET: Duration = Duration::from_secs(5);
const MAX_DAEMON_STDOUT_BYTES: usize = 64 * 1024;
const MAX_DAEMON_STDERR_BYTES: usize = 64 * 1024;
const CLI_EXECUTABLE_NAME: &str = "codex.exe";

/// Budget injection point so tests can exercise timeout and cleanup paths
/// without waiting for production durations.
#[derive(Clone, Copy, Debug)]
pub struct DaemonStopBudget {
    pub total: Duration,
    pub cleanup: Duration,
}

impl Default for DaemonStopBudget {
    fn default() -> Self {
        Self {
            total: DAEMON_STOP_TOTAL_BUDGET,
            cleanup: HELPER_CLEANUP_BUDGET,
        }
    }
}

/// The official Codex CLI resolved for one repair operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexCli {
    /// Canonicalized absolute path of the resolved executable. Resolution
    /// proves a trusted source selection, not an OpenAI signature check.
    pub executable: PathBuf,
    /// Absolute CODEX_HOME pinned for this operation when the user's explicit
    /// CODEX_HOME was relative. It is passed to the helper child and the
    /// Desktop child only; Guard's own environment is never modified.
    pub pinned_home: Option<PathBuf>,
}

/// Which Codex Home scope to resolve. Injectable so tests never depend on (or
/// mutate) the real environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexHomeInput {
    /// A non-empty `CODEX_HOME` was set. It must exist as a directory; failure
    /// is an error and never falls back to the default home.
    Explicit(PathBuf),
    /// `CODEX_HOME` was not set. Carries the resolved user home directory, if
    /// any; `None` means the user home could not be determined at all.
    Unset { user_home: Option<PathBuf> },
}

impl CodexHomeInput {
    pub fn from_env() -> Self {
        match env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
            Some(path) => Self::Explicit(PathBuf::from(path)),
            None => Self::Unset {
                user_home: user_home(),
            },
        }
    }
}

fn user_home() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Resolves the official Codex CLI for an explicit repair launch. `Ok(None)`
/// means no candidate was found in any trusted source; every `Err` is a
/// distinct, actionable failure and never silently falls back to another
/// source.
pub fn resolve_codex_cli(config: &GuardConfig) -> Result<Option<CodexCli>, String> {
    let path_var = env::var_os("PATH").unwrap_or_default();
    resolve_codex_cli_from(
        &CodexHomeInput::from_env(),
        &path_var,
        &config.codex.cli_executable_override,
    )
}

/// Test seam mirroring [`resolve_codex_cli`] with fully controlled inputs.
pub fn resolve_codex_cli_from(
    home: &CodexHomeInput,
    path_var: &OsStr,
    override_path: &Path,
) -> Result<Option<CodexCli>, String> {
    let (home, pinned_home) = resolve_home_scope(home)?;
    if !override_path.as_os_str().is_empty() {
        return cli_from_override(override_path, pinned_home).map(Some);
    }
    if let Some(home) = &home
        && let Some(executable) = cli_from_codex_home(home)
    {
        return Ok(Some(CodexCli {
            executable,
            pinned_home,
        }));
    }
    Ok(cli_from_path(path_var, pinned_home))
}

fn cli_from_override(
    override_path: &Path,
    pinned_home: Option<PathBuf>,
) -> Result<CodexCli, String> {
    let invalid = |message: String| {
        format!(
            "CODEX_CLI_OVERRIDE_INVALID: {message} Set codex.cli_executable_override to the \
             absolute path of the native codex.exe, or clear it to use automatic resolution."
        )
    };
    if !override_path.is_absolute() {
        return Err(invalid(
            "the configured Codex CLI path must be absolute.".into(),
        ));
    }
    if !override_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(invalid(
            "the configured Codex CLI must be the direct .exe entry point; shell shims such as \
             .cmd or .ps1 are not executed."
                .into(),
        ));
    }
    if !override_path.is_file() {
        return Err(invalid(format!(
            "the configured Codex CLI does not exist: {}",
            redact_text(&override_path.display().to_string())
        )));
    }
    let executable = fs::canonicalize(override_path).map_err(|error| {
        invalid(format!(
            "cannot canonicalize the configured Codex CLI {}: {error}",
            redact_text(&override_path.display().to_string())
        ))
    })?;
    Ok(CodexCli {
        executable,
        pinned_home,
    })
}

fn resolve_home_scope(home: &CodexHomeInput) -> Result<(Option<PathBuf>, Option<PathBuf>), String> {
    match home {
        CodexHomeInput::Explicit(path) => {
            // A relative explicit home is pinned against the working directory
            // once, then used consistently for the whole operation.
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                env::current_dir()
                    .map_err(|error| {
                        format!(
                            "CODEX_HOME_INVALID: cannot resolve the relative CODEX_HOME: {error}"
                        )
                    })?
                    .join(path)
            };
            if !absolute.is_dir() {
                return Err(format!(
                    "CODEX_HOME_INVALID: the explicit CODEX_HOME is not an existing directory: {}",
                    redact_text(&absolute.display().to_string())
                ));
            }
            let canonical = fs::canonicalize(&absolute).map_err(|error| {
                format!(
                    "CODEX_HOME_INVALID: cannot canonicalize CODEX_HOME {}: {error}",
                    redact_text(&absolute.display().to_string())
                )
            })?;
            let pinned_home = (!path.is_absolute()).then_some(canonical.clone());
            Ok((Some(canonical), pinned_home))
        }
        CodexHomeInput::Unset { user_home } => {
            let Some(user_home) = user_home else {
                return Err(
                    "CODEX_HOME_UNRESOLVED: cannot determine the user home directory to locate \
                     the Codex CLI; set CODEX_HOME or codex.cli_executable_override."
                        .into(),
                );
            };
            let default_home = user_home.join(".codex");
            if !default_home.is_dir() {
                // No default installation yet: not an error, just no package
                // candidates. Controlled PATH lookup still applies.
                return Ok((None, None));
            }
            let canonical = fs::canonicalize(&default_home).map_err(|error| {
                format!(
                    "CODEX_HOME_INVALID: cannot canonicalize the default Codex home {}: {error}",
                    redact_text(&default_home.display().to_string())
                )
            })?;
            Ok((Some(canonical), None))
        }
    }
}

fn cli_from_codex_home(home: &Path) -> Option<PathBuf> {
    let daemon_package = home
        .join("packages")
        .join("app-server-daemon")
        .join("current");
    let standalone_package = home.join("packages").join("standalone").join("current");
    // No recursive scans, no AppData sweeps: exactly these known layouts.
    [
        daemon_package.join("bin").join(CLI_EXECUTABLE_NAME),
        standalone_package.join("bin").join(CLI_EXECUTABLE_NAME),
        // legacy flat standalone layout still discussed by the official installer
        standalone_package.join(CLI_EXECUTABLE_NAME),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .and_then(|candidate| fs::canonicalize(candidate).ok())
}

/// PATH lookup with a controlled source: only absolute directories, never the
/// current directory, no `where.exe` subprocess, no implicit search paths.
fn cli_from_path(path_var: &OsStr, pinned_home: Option<PathBuf>) -> Option<CodexCli> {
    cli_from_path_with(path_var, env::current_dir().ok().as_deref(), pinned_home)
}

fn cli_from_path_with(
    path_var: &OsStr,
    current_dir: Option<&Path>,
    pinned_home: Option<PathBuf>,
) -> Option<CodexCli> {
    // Compare canonicalized forms so a PATH entry spelling of the working
    // directory (including `\\?\` variants) is recognized and skipped.
    let canonical_cwd = current_dir.and_then(|cwd| fs::canonicalize(cwd).ok());
    env::split_paths(path_var)
        .filter(|entry| entry.is_absolute())
        .filter(|entry| {
            entry
                .components()
                .next_back()
                .is_none_or(|last| last.as_os_str() != ".")
        })
        .filter_map(|entry| fs::canonicalize(&entry).ok())
        .filter(|dir| canonical_cwd.as_ref().is_none_or(|cwd| **dir != **cwd))
        .find_map(|dir| {
            let candidate = dir.join(CLI_EXECUTABLE_NAME);
            candidate
                .is_file()
                .then(|| fs::canonicalize(candidate).ok())
                .flatten()
        })
        .map(|executable| CodexCli {
            executable,
            pinned_home,
        })
}

/// Runs the public `codex app-server daemon stop` lifecycle command once. The
/// daemon is stopped by Codex itself through this command; Guard never touches
/// daemon processes directly. Only the exact documented `stopped` /
/// `notRunning` lifecycle statuses count as success.
pub async fn stop_codex_daemon(
    cli: &CodexCli,
    budget: &DaemonStopBudget,
    cancellation: &CancellationToken,
) -> Result<DaemonPreparation, String> {
    if cancellation.is_cancelled() {
        return Err(LAUNCH_CANCELLED_UNCONFIRMED.into());
    }
    let mut command = Command::new(&cli.executable);
    command
        .args(["app-server", "daemon", "stop"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(home) = &cli.pinned_home {
        command.env("CODEX_HOME", home);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|error| {
        format!("CODEX_DAEMON_STOP_FAILED: cannot run the resolved Codex CLI: {error}")
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "CODEX_DAEMON_STOP_FAILED: stdout was not captured".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "CODEX_DAEMON_STOP_FAILED: stderr was not captured".to_string())?;

    // Drive the bounded readers and the wait concurrently; any over-limit read
    // fails the whole operation before any parsing or downgrade classification.
    let operation = async {
        let read_stdout = async {
            read_limited(stdout, MAX_DAEMON_STDOUT_BYTES)
                .await
                .map_err(|error| pipe_error("stdout", error))
        };
        let read_stderr = async {
            read_limited(stderr, MAX_DAEMON_STDERR_BYTES)
                .await
                .map_err(|error| pipe_error("stderr", error))
        };
        let waited = async {
            child.wait().await.map_err(|error| {
                format!("CODEX_DAEMON_STOP_FAILED: cannot wait for the Codex CLI: {error}")
            })
        };
        tokio::try_join!(waited, read_stdout, read_stderr)
    };
    let joined = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(LAUNCH_CANCELLED_UNCONFIRMED.to_string()),
        result = tokio::time::timeout(budget.total, operation) => {
            result.map_err(|_| DAEMON_STOP_TIMEOUT.to_string()).and_then(|joined| joined)
        }
    };
    let (status, stdout, stderr) = match joined {
        Ok(joined) => joined,
        Err(message) => {
            // Unified failure exit: terminate and reap this short-lived helper
            // child within a bounded budget before giving the lock back.
            let _ = tokio::time::timeout(budget.cleanup, child.kill()).await;
            return Err(message);
        }
    };

    if !status.success() {
        if reports_unsupported_daemon_command(&stdout, &stderr) {
            return Err(
                "CODEX_DAEMON_UNSUPPORTED: the resolved Codex CLI does not support the \
                 app-server daemon lifecycle command; start a normal launch instead"
                    .into(),
            );
        }
        return Err(format!(
            "CODEX_DAEMON_STOP_FAILED: codex app-server daemon stop exited with {status}: {}",
            output_snippet(&stdout, &stderr)
        ));
    }
    let stdout_text = String::from_utf8(stdout).map_err(|_| {
        "CODEX_DAEMON_STOP_FAILED: the Codex CLI stdout was not valid UTF-8".to_string()
    })?;
    match parse_daemon_stop_status(&stdout_text) {
        DaemonStopStatus::Stopped => Ok(DaemonPreparation::Stopped),
        DaemonStopStatus::NotRunning => Ok(DaemonPreparation::NotNeeded),
        DaemonStopStatus::Unknown => Err(format!(
            "CODEX_DAEMON_STOP_FAILED: unexpected Codex daemon lifecycle output: {}",
            output_snippet(stdout_text.as_bytes(), &stderr)
        )),
    }
}

const LAUNCH_CANCELLED_UNCONFIRMED: &str = "LAUNCH_CANCELLED: preparation cancelled; the shared Codex background server state is \
     unconfirmed";
const DAEMON_STOP_TIMEOUT: &str = "CODEX_DAEMON_STOP_TIMEOUT: the preparation step did not finish within its budget; Desktop \
     was not started. The shared service may have received the stop request; its final state is \
     unconfirmed";

fn pipe_error(stream: &str, error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::InvalidData {
        format!("CODEX_DAEMON_STOP_FAILED: the Codex CLI {stream} exceeded its size limit")
    } else {
        format!("CODEX_DAEMON_STOP_FAILED: reading the Codex CLI {stream} failed: {error}")
    }
}

/// Reads at most `limit` bytes; reading `limit + 1` bytes is an error so that
/// oversize output can never be truncated into a plausible protocol message.
async fn read_limited<R>(reader: R, limit: usize) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = reader.take(limit as u64 + 1);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "child output exceeded limit",
        ));
    }
    Ok(bytes)
}

/// Lifecycle statuses documented by `codex app-server daemon` (camelCase).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonStopStatus {
    Stopped,
    NotRunning,
    Unknown,
}

#[derive(Debug, Deserialize)]
struct DaemonStopReport {
    status: String,
}

/// Parses the machine-readable lifecycle JSON: exactly one JSON object, unknown
/// fields tolerated, and only the exact `stopped` / `notRunning` values
/// accepted. Everything else — extra objects, invalid JSON, unknown or
/// missing statuses — is `Unknown` and treated as a failed stop.
pub fn parse_daemon_stop_status(output: &str) -> DaemonStopStatus {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return DaemonStopStatus::Unknown;
    }
    let Ok(report) = serde_json::from_str::<DaemonStopReport>(trimmed) else {
        return DaemonStopStatus::Unknown;
    };
    match report.status.as_str() {
        "stopped" => DaemonStopStatus::Stopped,
        "notRunning" => DaemonStopStatus::NotRunning,
        _ => DaemonStopStatus::Unknown,
    }
}

fn reports_unsupported_daemon_command(stdout: &[u8], stderr: &[u8]) -> bool {
    let haystack = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
    .to_ascii_lowercase();
    ["unrecognized subcommand", "unknown command"]
        .iter()
        .any(|needle| haystack.contains(needle))
}

/// Redacts first, strips control characters, then truncates; never shows raw
/// CLI output, socket paths, or full command lines in Guard messages.
fn output_snippet(stdout: &[u8], stderr: &[u8]) -> String {
    let combined = format!(
        "{} {}",
        String::from_utf8_lossy(stdout).trim(),
        String::from_utf8_lossy(stderr).trim()
    );
    let redacted = redact_text(combined.trim());
    let mut snippet: String = redacted
        .chars()
        .map(|character| {
            if character.is_control() && character != '\n' {
                ' '
            } else {
                character
            }
        })
        .take(200)
        .collect();
    if snippet.is_empty() {
        snippet.push_str("(no output)");
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_the_documented_lifecycle_statuses() {
        assert_eq!(
            parse_daemon_stop_status(r#"{"status":"stopped"}"#),
            DaemonStopStatus::Stopped
        );
        assert_eq!(
            parse_daemon_stop_status(r#"{"status":"notRunning"}"#),
            DaemonStopStatus::NotRunning
        );
    }

    #[test]
    fn tolerates_unknown_fields_but_nothing_else() {
        assert_eq!(
            parse_daemon_stop_status(r#"{"status":"stopped","pid":4242,"extra":true}"#),
            DaemonStopStatus::Stopped
        );
        // Unknown or running-style statuses are never treated as a successful stop.
        for output in [
            r#"{"status":"running"}"#,
            r#"{"status":"alreadyRunning"}"#,
            r#"{"status":"restarted"}"#,
            r#"{"status":"started"}"#,
            r#"{"status":"Stopped"}"#,
            r#"{"status":"not_running"}"#,
            r#"{"pid":4242}"#,
        ] {
            assert_eq!(parse_daemon_stop_status(output), DaemonStopStatus::Unknown);
        }
        // Exactly one JSON object: trailing objects, invalid JSON, and empty
        // output are all unknown.
        for output in [
            r#"{"status":"stopped"}{"status":"running"}"#,
            "not json",
            "",
            "   ",
        ] {
            assert_eq!(parse_daemon_stop_status(output), DaemonStopStatus::Unknown);
        }
    }

    #[test]
    fn override_must_be_an_absolute_native_exe() {
        let root = temp_dir("cli-override");
        let exe = root.join("codex.exe");
        fs::write(&exe, b"test").unwrap();

        assert!(cli_from_override(Path::new("codex.exe"), None).is_err());
        assert!(cli_from_override(Path::new(r"D:\Tools\codex.cmd"), None).is_err());
        assert!(cli_from_override(Path::new(r"Z:\missing\codex.exe"), None).is_err());

        let resolved = cli_from_override(&exe, None).unwrap();
        assert_eq!(resolved.executable, fs::canonicalize(&exe).unwrap());
        assert_eq!(resolved.pinned_home, None);
        let pinned = PathBuf::from(r"C:\somewhere");
        let resolved = cli_from_override(&exe, Some(pinned.clone())).unwrap();
        assert_eq!(resolved.pinned_home, Some(pinned));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_home_packages_prefer_daemon_layout_and_support_flat_standalone() {
        let home = temp_dir("cli-home");
        assert!(cli_from_codex_home(&home).is_none());

        let standalone = home.join("packages").join("standalone").join("current");
        write_executable(&standalone.join("codex.exe"));
        assert_eq!(
            cli_from_codex_home(&home).unwrap(),
            fs::canonicalize(standalone.join("codex.exe")).unwrap()
        );

        let daemon = home
            .join("packages")
            .join("app-server-daemon")
            .join("current");
        write_executable(&daemon.join("bin").join("codex.exe"));
        assert_eq!(
            cli_from_codex_home(&home).unwrap(),
            fs::canonicalize(daemon.join("bin").join("codex.exe")).unwrap()
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn explicit_home_scope_is_validated_without_fallback() {
        let existing = temp_dir("home-explicit");
        let missing = existing.join("missing");
        let file_home = existing.join("file.txt");
        fs::write(&file_home, b"x").unwrap();

        assert!(resolve_home_scope(&CodexHomeInput::Explicit(missing)).is_err());
        assert!(resolve_home_scope(&CodexHomeInput::Explicit(file_home)).is_err());
        let (home, pinned) =
            resolve_home_scope(&CodexHomeInput::Explicit(existing.clone())).unwrap();
        assert_eq!(home.unwrap(), fs::canonicalize(&existing).unwrap());
        assert_eq!(pinned, None, "absolute explicit homes are not re-pinned");

        let (home, pinned) = resolve_home_scope(&CodexHomeInput::Unset {
            user_home: Some(existing.clone()),
        })
        .unwrap();
        assert!(
            home.is_none(),
            "a user home without .codex yields no candidates, not an error"
        );
        assert_eq!(pinned, None);

        let error = resolve_home_scope(&CodexHomeInput::Unset { user_home: None }).unwrap_err();
        assert!(error.contains("CODEX_HOME_UNRESOLVED"));
        fs::remove_dir_all(existing).unwrap();
    }

    #[test]
    fn resolver_uses_only_the_explicit_home_scope() {
        let explicit = temp_dir("home-a");
        let default_like = temp_dir("home-b");
        write_executable(
            &explicit
                .join("packages")
                .join("standalone")
                .join("current")
                .join("bin")
                .join("codex.exe"),
        );
        write_executable(
            &default_like
                .join("packages")
                .join("standalone")
                .join("current")
                .join("bin")
                .join("codex.exe"),
        );

        let resolved = resolve_codex_cli_from(
            &CodexHomeInput::Explicit(explicit.clone()),
            OsStr::new(""),
            Path::new(""),
        )
        .unwrap()
        .unwrap();
        assert!(
            resolved
                .executable
                .starts_with(fs::canonicalize(&explicit).unwrap())
        );

        // An explicit home without packages and an empty PATH yield no
        // candidate — never a fallback to another home.
        let empty_home = temp_dir("home-empty");
        let resolved = resolve_codex_cli_from(
            &CodexHomeInput::Explicit(empty_home.clone()),
            OsStr::new(""),
            Path::new(""),
        )
        .unwrap();
        assert_eq!(resolved, None);
        for home in [explicit, default_like, empty_home] {
            fs::remove_dir_all(home).unwrap();
        }
    }

    #[test]
    fn legacy_cli_errors_are_reported_as_unsupported() {
        assert!(reports_unsupported_daemon_command(
            b"",
            b"error: unrecognized subcommand 'daemon'"
        ));
        assert!(reports_unsupported_daemon_command(
            b"unknown command: app-server",
            b""
        ));
        assert!(!reports_unsupported_daemon_command(
            b"",
            b"daemon socket busy"
        ));
    }

    #[test]
    fn path_lookup_ignores_relative_entries_and_the_current_directory() {
        let cwd_decoy = temp_dir("path-decoy");
        write_executable(&cwd_decoy.join(CLI_EXECUTABLE_NAME));
        let trusted = temp_dir("path-trusted");
        write_executable(&trusted.join(CLI_EXECUTABLE_NAME));

        // Empty, `.`, relative entries, and the current directory itself are
        // never selected; the trusted absolute directory wins.
        let path_var = env::join_paths([
            PathBuf::new(),
            PathBuf::from("."),
            PathBuf::from("relative/dir"),
            cwd_decoy.clone(),
            trusted.clone(),
        ])
        .unwrap();
        let resolved = cli_from_path_with(&path_var, Some(&cwd_decoy), None).unwrap();
        assert_eq!(
            resolved.executable,
            fs::canonicalize(trusted.join(CLI_EXECUTABLE_NAME)).unwrap()
        );
        assert_eq!(resolved.pinned_home, None);

        // With only unusable entries the lookup yields nothing — a decoy in
        // the working directory is never chosen.
        let path_var = env::join_paths([
            PathBuf::new(),
            PathBuf::from("."),
            PathBuf::from("relative/dir"),
            cwd_decoy.clone(),
        ])
        .unwrap();
        assert!(cli_from_path_with(&path_var, Some(&cwd_decoy), None).is_none());
        fs::remove_dir_all(cwd_decoy).unwrap();
        fs::remove_dir_all(trusted).unwrap();
    }

    #[test]
    fn path_lookup_handles_paths_with_spaces_and_non_ascii() {
        let dir = temp_dir("path 带 空格 和 中文");
        write_executable(&dir.join(CLI_EXECUTABLE_NAME));
        let path_var = env::join_paths([dir.clone()]).unwrap();
        let resolved = cli_from_path_with(&path_var, None, None).unwrap();
        assert!(resolved.executable.ends_with(CLI_EXECUTABLE_NAME));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snippets_are_redacted_bounded_and_control_free() {
        let long = vec![b'x'; 500];
        assert_eq!(output_snippet(&long, b"").len(), 200);
        assert_eq!(output_snippet(b"", b"token=abc123"), "token=[REDACTED]");
        assert_eq!(output_snippet(b"  ", b"  "), "(no output)");
        assert!(!output_snippet(b"a\x1b[31mb", b"").contains('\x1b'));
    }

    fn temp_dir(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("cpg {label} {unique} {}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_executable(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"test").unwrap();
    }
}
