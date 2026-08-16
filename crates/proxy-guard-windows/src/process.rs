use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use proxy_guard_core::{DesktopAppInfo, DesktopProcessState, GuardConfig, LaunchReceipt};
use sysinfo::System;

use crate::environment::{apply_proxy_environment, proxy_environment_with_endpoint};

struct StartupLock {
    _file: File,
}

impl StartupLock {
    fn acquire() -> Result<Self, String> {
        let path = std::env::temp_dir().join("codex-proxy-guard-startup.lock");
        Self::acquire_at(&path)
    }

    fn acquire_at(path: &Path) -> Result<Self, String> {
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
        if text
            .trim()
            .parse::<u64>()
            .is_ok_and(|last| last <= now && now - last < 5_000)
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
    system
        .processes()
        .iter()
        .find(|(_, process)| {
            process.start_time() != 0
                && process
                    .exe()
                    .is_some_and(|executable| paths_equal(executable, &info.executable))
                && !process.cmd().iter().any(|part| {
                    part.to_string_lossy()
                        .to_ascii_lowercase()
                        .contains("--type=")
                })
        })
        .map_or(DesktopProcessState::Stopped, |(pid, _)| {
            DesktopProcessState::Running { pid: pid.as_u32() }
        })
}

pub fn launch_codex(info: &DesktopAppInfo, config: &GuardConfig) -> Result<LaunchReceipt, String> {
    let proxy_url = config.proxy_url();
    launch_codex_with_proxy(info, config, &proxy_url)
}

pub fn launch_codex_with_proxy(
    info: &DesktopAppInfo,
    config: &GuardConfig,
    proxy_url: &str,
) -> Result<LaunchReceipt, String> {
    config.validate().map_err(|error| error.to_string())?;
    let mut lock = StartupLock::acquire()?;

    if config.codex.refuse_if_running
        && matches!(
            desktop_process_state(info),
            DesktopProcessState::Running { .. }
        )
    {
        return Err(
            "CODEX_ALREADY_RUNNING: Desktop is already running; fully exit it before launching through Guard"
                .into(),
        );
    }
    if !info.executable.is_file() {
        return Err(format!(
            "CODEX_EXECUTABLE_MISSING: {} no longer exists; refresh and retry",
            info.executable.display()
        ));
    }

    let environment = proxy_environment_with_endpoint(config, proxy_url.to_owned());
    let mut command = Command::new(&info.executable);
    apply_proxy_environment(&mut command, &environment);
    let child = command
        .spawn()
        .map_err(|error| format!("CODEX_LAUNCH_FAILED: Desktop could not be started: {error}"))?;
    lock.mark_spawned();

    Ok(LaunchReceipt {
        pid: child.id(),
        proxy_endpoint: environment.proxy_url,
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

fn paths_equal(left: &Path, right: &Path) -> bool {
    normalized_path(left).eq_ignore_ascii_case(&normalized_path(right))
}

fn normalized_path(path: &Path) -> String {
    path.as_os_str().to_string_lossy().replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_compare_case_insensitively() {
        assert!(paths_equal(
            Path::new(r"C:\Program Files\OpenAI\ChatGPT.exe"),
            Path::new(r"c:/program files/openai/chatgpt.exe"),
        ));
    }

    #[test]
    fn startup_lock_is_exclusive_on_windows() {
        let path = std::env::temp_dir().join(format!(
            "codex-proxy-guard-lock-test-{}.lock",
            std::process::id()
        ));
        let first = StartupLock::acquire_at(&path).unwrap();
        #[cfg(windows)]
        assert!(StartupLock::acquire_at(&path).is_err());
        drop(first);
        assert!(StartupLock::acquire_at(&path).is_ok());
        let _ = std::fs::remove_file(path);
    }
}
