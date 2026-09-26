use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::Path,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use proxy_guard_core::{
    DaemonPreparation, DesktopAppInfo, DesktopProcessState, GuardConfig, LaunchOptions,
    LaunchReceipt,
};
use sysinfo::System;
use tokio_util::sync::CancellationToken;

use crate::codex_daemon::{CodexCli, DaemonStopBudget, resolve_codex_cli, stop_codex_daemon};
use crate::environment::{apply_proxy_environment, proxy_environment};

struct StartupLock {
    _file: File,
}

impl StartupLock {
    fn acquire(busy_window: Duration) -> Result<Self, String> {
        let path = std::env::temp_dir().join("codex-proxy-guard-startup.lock");
        Self::acquire_at(&path, busy_window)
    }

    fn acquire_at(path: &Path, busy_window: Duration) -> Result<Self, String> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let mut lock = options
            .open(path)
            .map(|file| Self { _file: file })
            .map_err(|_| {
                "LAUNCH_BUSY: another Codex Proxy Guard instance is currently launching Desktop"
                    .to_string()
            })?;
        let mut text = String::new();
        let _ = lock._file.read_to_string(&mut text);
        let now = now_unix_ms();
        let window_ms = busy_window.as_millis() as u64;
        if text
            .trim()
            .parse::<u64>()
            .is_ok_and(|last| last <= now && now - last < window_ms)
        {
            return Err(
                "LAUNCH_BUSY: another Codex Proxy Guard instance just launched Desktop; refresh and retry"
                    .into(),
            );
        }
        Ok(lock)
    }

    fn mark_spawned(&mut self) {
        if self._file.rewind().is_ok() && self._file.set_len(0).is_ok() {
            let _ = write!(self._file, "{}", now_unix_ms());
            let _ = self._file.flush();
        }
    }
}

pub fn desktop_process_state(info: &DesktopAppInfo) -> DesktopProcessState {
    let mut system = System::new_all();
    system.refresh_all();
    let mut saw_unconfirmed_target = false;
    for (pid, process) in system.processes() {
        if process.start_time() == 0 {
            continue;
        }
        if process.cmd().iter().any(|part| {
            part.to_string_lossy()
                .to_ascii_lowercase()
                .contains("--type=")
        }) {
            continue;
        }
        match classify_process(process.exe(), process.name(), &info.executable) {
            CandidateVerdict::Target => {
                return DesktopProcessState::Running { pid: pid.as_u32() };
            }
            // A likely target whose executable path cannot be read must not be
            // reported as "stopped"; unrelated unreadable processes are ignored.
            CandidateVerdict::UnconfirmedTarget => saw_unconfirmed_target = true,
            CandidateVerdict::Unrelated => {}
        }
    }
    if saw_unconfirmed_target {
        DesktopProcessState::Unknown
    } else {
        DesktopProcessState::Stopped
    }
}

enum CandidateVerdict {
    Target,
    UnconfirmedTarget,
    Unrelated,
}

fn classify_process(
    exe: Option<&Path>,
    name: &OsStr,
    target_executable: &Path,
) -> CandidateVerdict {
    match exe {
        Some(exe) if paths_equal(exe, target_executable) => CandidateVerdict::Target,
        Some(_) => CandidateVerdict::Unrelated,
        None => {
            let matches_by_name = target_executable
                .file_name()
                .is_some_and(|target| name.eq_ignore_ascii_case(target));
            if matches_by_name {
                CandidateVerdict::UnconfirmedTarget
            } else {
                CandidateVerdict::Unrelated
            }
        }
    }
}

/// Injection points for tests: the CLI resolver can be faked and the stop
/// budget shortened without waiting for production durations.
pub struct LaunchHooks<'a> {
    pub resolve_cli: &'a (dyn Fn(&GuardConfig) -> Result<Option<CodexCli>, String> + Sync),
    pub stop_budget: DaemonStopBudget,
    /// How long a freshly recorded spawn keeps blocking other Guard launches.
    /// Production uses 5 s; tests shrink it so serialized launches do not trip
    /// the anti-race window.
    pub post_spawn_busy_window: Duration,
}

impl Default for LaunchHooks<'_> {
    fn default() -> Self {
        Self {
            resolve_cli: &resolve_codex_cli,
            stop_budget: DaemonStopBudget::default(),
            post_spawn_busy_window: Duration::from_secs(5),
        }
    }
}

/// Launches Desktop with the proxy environment injected.
///
/// Normal launches are side-effect free with respect to Codex: the CLI is not
/// resolved and no daemon command runs. The explicit repair launch
/// (`LaunchOptions::refresh_codex_daemon`) additionally stops the shared
/// daemon through the official lifecycle command after the user's one-shot
/// confirmation, then re-verifies cancellation, the executable, and the
/// Desktop running state before spawning.
///
/// Cancellation contract: once cancellation has been observed, no new spawn is
/// started; after a spawn has happened, Guard never kills the Desktop. The
/// startup lock is held from the running-check through the daemon step to the
/// spawn itself. Guard never terminates Desktop, the daemon, or any process it
/// did not create.
pub async fn launch_codex(
    info: &DesktopAppInfo,
    config: &GuardConfig,
    options: LaunchOptions,
    cancellation: &CancellationToken,
) -> Result<LaunchReceipt, String> {
    launch_codex_with(info, config, options, cancellation, &LaunchHooks::default()).await
}

pub async fn launch_codex_with(
    info: &DesktopAppInfo,
    config: &GuardConfig,
    options: LaunchOptions,
    cancellation: &CancellationToken,
    hooks: &LaunchHooks<'_>,
) -> Result<LaunchReceipt, String> {
    config.validate().map_err(|error| error.to_string())?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    crate::elevation::ensure_non_elevated()?;
    let mut lock = StartupLock::acquire(hooks.post_spawn_busy_window)?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }

    // The repair path always refuses an already-running Desktop; a normal
    // launch may allow it only when explicitly configured.
    let refuse_if_running = options.refresh_codex_daemon || config.codex.refuse_if_running;
    if refuse_if_running {
        match desktop_process_state(info) {
            DesktopProcessState::Running { .. } => {
                return Err(
                    "CODEX_ALREADY_RUNNING: Desktop is already running; fully exit it before launching through Guard"
                        .into(),
                );
            }
            DesktopProcessState::Unknown => {
                return Err(
                    "CODEX_RUNNING_UNKNOWN: cannot confirm whether Desktop is running; it was not launched"
                        .into(),
                );
            }
            DesktopProcessState::Stopped => {}
        }
    }
    if !info.executable.is_file() {
        return Err(format!(
            "CODEX_EXECUTABLE_MISSING: {} no longer exists; refresh and retry",
            info.executable.display()
        ));
    }

    let mut pinned_home = None;
    let daemon_preparation = if options.refresh_codex_daemon {
        if cancellation.is_cancelled() {
            return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
        }
        let cli = (hooks.resolve_cli)(config)?;
        let Some(cli) = cli else {
            return Err(
                "CODEX_CLI_UNAVAILABLE: no official Codex CLI was found for the repair launch; \
                 start a normal launch instead, or set codex.cli_executable_override / CODEX_HOME"
                    .into(),
            );
        };
        if cancellation.is_cancelled() {
            return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
        }
        let preparation = stop_codex_daemon(&cli, &hooks.stop_budget, cancellation).await?;
        pinned_home = cli.pinned_home;
        // The stop may take a while: re-verify everything that was checked
        // before the wait window, still under the startup lock.
        if cancellation.is_cancelled() {
            return Err(match preparation {
                DaemonPreparation::Stopped => {
                    "LAUNCH_CANCELLED: preparation cancelled after the shared Codex background \
                     server was stopped; Desktop was not started"
                        .into()
                }
                _ => "LAUNCH_CANCELLED: Guard is shutting down".into(),
            });
        }
        if !info.executable.is_file() {
            return Err(format!(
                "CODEX_EXECUTABLE_MISSING: {} disappeared while preparing the launch; refresh and retry",
                info.executable.display()
            ));
        }
        if matches!(
            desktop_process_state(info),
            DesktopProcessState::Running { .. } | DesktopProcessState::Unknown
        ) {
            return Err(
                "CODEX_ALREADY_RUNNING: Desktop appeared to be running while preparing the \
                 launch; no second instance was started"
                    .into(),
            );
        }
        preparation
    } else {
        DaemonPreparation::Skipped
    };

    // Final cancellation checkpoint. No `await` is allowed between here and
    // the synchronous Desktop spawn below.
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    let environment = proxy_environment(config);
    let mut command = Command::new(&info.executable);
    apply_proxy_environment(&mut command, &environment);
    if let Some(home) = &pinned_home {
        // Keep the Desktop and the stop helper on the same explicit Home scope.
        command.env("CODEX_HOME", home);
    }
    let child = command.spawn().map_err(|error| match daemon_preparation {
        DaemonPreparation::Stopped => format!(
            "CODEX_LAUNCH_FAILED: Desktop could not be started: {error}; note: the shared \
                 Codex background server was stopped before this failure"
        ),
        _ => format!("CODEX_LAUNCH_FAILED: Desktop could not be started: {error}"),
    })?;
    lock.mark_spawned();

    Ok(LaunchReceipt {
        pid: child.id(),
        proxy_endpoint: environment.proxy_url,
        daemon_preparation,
        desktop: info.into(),
    })
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Compares two paths as the same Windows location. Both plain and
/// extended-length (`\\?\C:\...`, `\\?\UNC\server\share\...`) spellings of the
/// same location compare equal; comparison is ASCII case-insensitive like
/// Windows itself.
fn paths_equal(left: &Path, right: &Path) -> bool {
    normalize_windows_path(left).eq_ignore_ascii_case(&normalize_windows_path(right))
}

fn normalize_windows_path(path: &Path) -> String {
    let text = path.as_os_str().to_string_lossy().replace('/', "\\");
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalent_windows_paths_compare_equal() {
        for (left, right) in [
            (
                r"C:\Program Files\OpenAI\ChatGPT.exe",
                r"\\?\C:\Program Files\OpenAI\ChatGPT.exe",
            ),
            (
                r"C:/program files/openai/chatgpt.exe",
                r"c:\PROGRAM FILES\OpenAI\ChatGPT.exe",
            ),
            (
                r"\\server\share\ChatGPT.exe",
                r"\\?\UNC\server\share\ChatGPT.exe",
            ),
        ] {
            assert!(
                paths_equal(Path::new(left), Path::new(right)),
                "{left} vs {right}"
            );
        }
    }

    #[test]
    fn different_locations_do_not_match_by_file_name() {
        assert!(!paths_equal(
            Path::new(r"C:\Apps\ChatGPT\ChatGPT.exe"),
            Path::new(r"D:\Other\ChatGPT.exe"),
        ));
        assert!(!paths_equal(
            Path::new(r"\\?\C:\Apps\ChatGPT.exe"),
            Path::new(r"\\?\UNC\host\C$\Apps\ChatGPT.exe"),
        ));
    }

    #[test]
    fn classify_processes_by_path_then_name() {
        let target = Path::new(r"\\?\C:\Apps\OpenAI\ChatGPT.exe");
        assert!(matches!(
            classify_process(
                Some(Path::new(r"C:\Apps\OpenAI\ChatGPT.exe")),
                OsStr::new("other.exe"),
                target
            ),
            CandidateVerdict::Target
        ));
        assert!(matches!(
            classify_process(
                Some(Path::new(r"C:\Elsewhere\codex.exe")),
                OsStr::new("codex.exe"),
                target
            ),
            CandidateVerdict::Unrelated
        ));
        assert!(matches!(
            classify_process(None, OsStr::new("ChatGPT.EXE"), target),
            CandidateVerdict::UnconfirmedTarget
        ));
        assert!(matches!(
            classify_process(None, OsStr::new("unrelated.exe"), target),
            CandidateVerdict::Unrelated
        ));
    }

    #[test]
    fn startup_lock_is_exclusive_on_windows() {
        let path = std::env::temp_dir().join(format!(
            "codex-proxy-guard-lock-test-{}.lock",
            std::process::id()
        ));
        let window = Duration::from_secs(5);
        let first = StartupLock::acquire_at(&path, window).unwrap();
        #[cfg(windows)]
        assert!(StartupLock::acquire_at(&path, window).is_err());
        drop(first);
        assert!(StartupLock::acquire_at(&path, window).is_ok());
        let _ = std::fs::remove_file(path);
    }
}
