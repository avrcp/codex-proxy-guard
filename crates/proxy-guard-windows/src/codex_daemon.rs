//! Codex daemon compatibility adapter (public lifecycle CLI only).
//!
//! Codex 0.157+ runs a shared background daemon that keeps the environment it
//! inherited when it started, so a newly launched Desktop cannot fix a stale
//! proxy environment by itself. Before spawning Desktop, Guard asks the
//! official Codex CLI to stop a stale daemon (`codex app-server daemon stop`);
//! Desktop then starts a fresh daemon under the injected proxy environment.
//!
//! Guard never starts, restarts, updates, or monitors the daemon, never
//! connects to the app-server socket or any private IPC, and never terminates
//! Codex processes directly. The only allowed interaction is the public
//! lifecycle stop command executed through the locally resolved official CLI.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use proxy_guard_core::{DaemonPreparation, GuardConfig, redact_text};
use serde::Deserialize;
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

/// Codex daemon graceful shutdown is 60 seconds; leave headroom before the
/// CLI child is terminated. The daemon itself is never killed by Guard.
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(75);
const WHERE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DAEMON_STDOUT_BYTES: u64 = 64 * 1024;
const MAX_DAEMON_STDERR_BYTES: u64 = 64 * 1024;
const MAX_WHERE_STDOUT_BYTES: u64 = 64 * 1024;

/// The officially resolved Codex CLI used only for public lifecycle commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexCli {
    pub executable: PathBuf,
}

/// Resolves the official Codex CLI: explicit override, then the CODEX_HOME
/// app-server-daemon and standalone package layouts, then `where.exe`.
/// Missing CLI is not an error: it maps to `DaemonPreparation::LifecycleUnavailable`.
pub async fn resolve_codex_cli(
    config: &GuardConfig,
    cancellation: &CancellationToken,
) -> Option<CodexCli> {
    if let Some(cli) = cli_from_override(&config.codex.cli_executable_override) {
        return Some(cli);
    }
    if let Some(cli) = cli_from_codex_home(&codex_home()) {
        return Some(cli);
    }
    cli_from_path(cancellation).await
}

fn cli_from_override(override_path: &Path) -> Option<CodexCli> {
    (!override_path.as_os_str().is_empty() && override_path.is_file()).then(|| CodexCli {
        executable: override_path.to_path_buf(),
    })
}

pub fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home().unwrap_or_default().join(".codex"))
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn cli_from_codex_home(home: &Path) -> Option<CodexCli> {
    ["app-server-daemon", "standalone"]
        .into_iter()
        .map(|package| {
            home.join("packages")
                .join(package)
                .join("current")
                .join("bin")
                .join("codex.exe")
        })
        .find(|candidate| candidate.is_file())
        .map(|executable| CodexCli { executable })
}

#[cfg(windows)]
async fn cli_from_path(cancellation: &CancellationToken) -> Option<CodexCli> {
    use std::os::windows::process::CommandExt;

    let mut command = Command::new("where.exe");
    command
        .arg("codex.exe")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.as_std_mut().creation_flags(0x0800_0000);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_WHERE_STDOUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let waited = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return None,
        result = tokio::time::timeout(WHERE_TIMEOUT, child.wait()) => result.ok()?,
    };
    let status = waited.ok()?;
    let bytes = stdout_task.await.ok()?.ok()?;
    if !status.success() || bytes.len() > MAX_WHERE_STDOUT_BYTES as usize {
        return None;
    }
    String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && Path::new(line).is_file())
        .map(|executable| CodexCli {
            executable: PathBuf::from(executable),
        })
}

#[cfg(not(windows))]
async fn cli_from_path(_cancellation: &CancellationToken) -> Option<CodexCli> {
    None
}

/// Runs `codex app-server daemon stop` so a stale daemon cannot keep an old
/// proxy environment. The daemon is stopped by Codex itself through this
/// public command; Guard never terminates Codex processes.
pub async fn prepare_codex_daemon_for_launch(
    cli: Option<&CodexCli>,
    cancellation: &CancellationToken,
) -> Result<DaemonPreparation, String> {
    let Some(cli) = cli else {
        return Ok(DaemonPreparation::LifecycleUnavailable);
    };
    let mut command = Command::new(&cli.executable);
    command
        .args(["app-server", "daemon", "stop"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x0800_0000);
    }
    run_daemon_stop(command, cancellation).await
}

async fn run_daemon_stop(
    mut command: Command,
    cancellation: &CancellationToken,
) -> Result<DaemonPreparation, String> {
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
    let operation = async move {
        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout
                .take(MAX_DAEMON_STDOUT_BYTES + 1)
                .read_to_end(&mut bytes)
                .await
                .map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr
                .take(MAX_DAEMON_STDERR_BYTES + 1)
                .read_to_end(&mut bytes)
                .await
                .map(|_| bytes)
        });
        let status = child.wait().await;
        let stdout = stdout_task
            .await
            .map_err(|error| format!("CODEX_DAEMON_STOP_FAILED: stdout reader failed: {error}"))?
            .map_err(|error| format!("CODEX_DAEMON_STOP_FAILED: stdout read failed: {error}"))?;
        let stderr = stderr_task
            .await
            .map_err(|error| format!("CODEX_DAEMON_STOP_FAILED: stderr reader failed: {error}"))?
            .map_err(|error| format!("CODEX_DAEMON_STOP_FAILED: stderr read failed: {error}"))?;
        Ok::<_, String>((status, stdout, stderr))
    };
    let (status, stdout, stderr) = tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
        }
        result = tokio::time::timeout(DAEMON_STOP_TIMEOUT, operation) => {
            result.map_err(|_| {
                "CODEX_DAEMON_STOP_TIMEOUT: Codex background server did not stop before the launch timeout. Run 'codex app-server daemon stop' manually, then retry Codex Proxy Guard."
                    .to_string()
            })??
        }
    };
    let status = status.map_err(|error| {
        format!("CODEX_DAEMON_STOP_FAILED: cannot wait for the Codex CLI: {error}")
    })?;
    let stdout_text = String::from_utf8_lossy(&stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(&stderr).into_owned();
    if !status.success() {
        if reports_unsupported_daemon_command(&stdout_text, &stderr_text) {
            return Ok(DaemonPreparation::LifecycleUnavailable);
        }
        return Err(format!(
            "CODEX_DAEMON_STOP_FAILED: codex app-server daemon stop exited with {status}: {}",
            output_snippet(&stdout_text, &stderr_text)
        ));
    }
    match parse_daemon_stop_status(&stdout_text) {
        DaemonStopStatus::Stopped => Ok(DaemonPreparation::Stopped),
        DaemonStopStatus::NotRunning => Ok(DaemonPreparation::NotNeeded),
        DaemonStopStatus::Running => Err(
            "CODEX_DAEMON_STOP_FAILED: the Codex background server reported that it is still running"
                .into(),
        ),
        DaemonStopStatus::Unknown => Err(format!(
            "CODEX_DAEMON_STOP_FAILED: unrecognized Codex daemon output: {}",
            output_snippet(&stdout_text, &stderr_text)
        )),
    }
}

/// Status values reported by `codex app-server daemon` lifecycle JSON output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonStopStatus {
    Running,
    Stopped,
    NotRunning,
    Unknown,
}

#[derive(Debug, Deserialize)]
struct DaemonStopReport {
    status: String,
}

/// Parses the machine-readable daemon lifecycle JSON. Unknown fields are
/// ignored; unknown or missing status values are reported as `Unknown`.
pub fn parse_daemon_stop_status(output: &str) -> DaemonStopStatus {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return DaemonStopStatus::Unknown;
    }
    let Ok(report) = serde_json::from_str::<DaemonStopReport>(trimmed) else {
        return DaemonStopStatus::Unknown;
    };
    match report
        .status
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-'], "")
        .as_str()
    {
        "stopped" => DaemonStopStatus::Stopped,
        "notrunning" => DaemonStopStatus::NotRunning,
        "running" => DaemonStopStatus::Running,
        _ => DaemonStopStatus::Unknown,
    }
}

fn reports_unsupported_daemon_command(stdout: &str, stderr: &str) -> bool {
    let haystack = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    [
        "unrecognized subcommand",
        "unknown command",
        "daemon command unsupported",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn output_snippet(stdout: &str, stderr: &str) -> String {
    let combined = format!("{} {}", stdout.trim(), stderr.trim());
    let redacted = redact_text(combined.trim());
    let mut snippet: String = redacted.chars().take(200).collect();
    if snippet.is_empty() {
        snippet.push_str("(no output)");
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_daemon_statuses() {
        assert_eq!(
            parse_daemon_stop_status(r#"{"status":"running"}"#),
            DaemonStopStatus::Running
        );
        assert_eq!(
            parse_daemon_stop_status(r#"{"status":"stopped"}"#),
            DaemonStopStatus::Stopped
        );
        assert_eq!(
            parse_daemon_stop_status(r#"{"status":"notRunning"}"#),
            DaemonStopStatus::NotRunning
        );
        assert_eq!(
            parse_daemon_stop_status(r#"{"status":"not_running"}"#),
            DaemonStopStatus::NotRunning
        );
    }

    #[test]
    fn ignores_unknown_fields_and_reports_unknown_statuses() {
        assert_eq!(
            parse_daemon_stop_status(r#"{"status":"stopped","pid":4242,"extra":true}"#),
            DaemonStopStatus::Stopped
        );
        assert_eq!(
            parse_daemon_stop_status(r#"{"status":"hibernating"}"#),
            DaemonStopStatus::Unknown
        );
        assert_eq!(
            parse_daemon_stop_status(r#"{"pid":4242}"#),
            DaemonStopStatus::Unknown
        );
        assert_eq!(parse_daemon_stop_status(""), DaemonStopStatus::Unknown);
        assert_eq!(
            parse_daemon_stop_status("not json"),
            DaemonStopStatus::Unknown
        );
    }

    #[test]
    fn override_is_used_only_when_it_points_to_a_file() {
        assert!(cli_from_override(Path::new("")).is_none());
        assert!(cli_from_override(Path::new("Z:\\definitely\\missing\\codex.exe")).is_none());

        let root = temp_dir("cli-override");
        std::fs::write(root.join("codex.exe"), b"test").unwrap();
        assert_eq!(
            cli_from_override(&root.join("codex.exe"))
                .unwrap()
                .executable,
            root.join("codex.exe")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_home_prefers_the_daemon_package_over_standalone() {
        let home = temp_dir("cli-home-empty");
        assert!(cli_from_codex_home(&home).is_none());

        let standalone = home.join("packages").join("standalone");
        write_cli(&standalone);
        let resolved = cli_from_codex_home(&home).unwrap();
        assert_eq!(
            resolved.executable,
            standalone.join("current").join("bin").join("codex.exe")
        );

        let daemon = home.join("packages").join("app-server-daemon");
        write_cli(&daemon);
        let resolved = cli_from_codex_home(&home).unwrap();
        assert_eq!(
            resolved.executable,
            daemon.join("current").join("bin").join("codex.exe")
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn legacy_cli_errors_map_to_lifecycle_unavailable() {
        assert!(reports_unsupported_daemon_command(
            "",
            "error: unrecognized subcommand 'daemon'"
        ));
        assert!(reports_unsupported_daemon_command(
            "unknown command: app-server",
            ""
        ));
        assert!(reports_unsupported_daemon_command(
            "",
            "daemon command unsupported"
        ));
        assert!(!reports_unsupported_daemon_command(
            "",
            "daemon socket busy"
        ));
    }

    #[test]
    fn snippets_are_bounded_and_redacted() {
        let long = "x".repeat(500);
        assert_eq!(output_snippet(&long, "").len(), 200);
        assert_eq!(output_snippet("", "token=abc123"), "token=[REDACTED]");
        assert_eq!(output_snippet("  ", "  "), "(no output)");
    }

    fn temp_dir(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cpg {label} {unique}"));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_cli(package: &Path) {
        let bin = package.join("current").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("codex.exe"), b"test").unwrap();
    }
}
