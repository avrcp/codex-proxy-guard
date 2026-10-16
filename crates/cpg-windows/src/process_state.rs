//! Desktop process observation via a Toolhelp snapshot: name-matched
//! candidates, canonical image comparison, and the Electron-helper rule
//! (a candidate whose parent is also a candidate is not the root).

use crate::handles::OwnedHandle;
use crate::paths::normalized_windows_path;
use cpg_domain::discovery::Desktop;
use std::path::Path;
use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Storage::FileSystem::SYNCHRONIZE as SYNCHRONIZE_ACCESS;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, WaitForSingleObject,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessState {
    pub state: &'static str,
    pub pid: u32,
}

fn image_path(process: windows::Win32::Foundation::HANDLE) -> Option<String> {
    let mut buffer = [0u16; 32768];
    let mut size = buffer.len() as u32;
    // SAFETY: the handle is owned by the caller; the buffer is sized with its
    // length passed in and out.
    if unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    }
    .is_err()
    {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}

fn open_process(pid: u32) -> Option<OwnedHandle> {
    // SAFETY: PID from the snapshot; the returned handle is owned.
    match unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_ACCESS_RIGHTS(SYNCHRONIZE_ACCESS.0),
            false,
            pid,
        )
    } {
        Ok(handle) => OwnedHandle::new(handle),
        Err(_) => None,
    }
}

/// Port of `desktopProcessState`.
pub fn desktop_process_state(desktop: &Desktop) -> ProcessState {
    let target_name = desktop
        .executable
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    if target_name.is_empty() {
        return ProcessState {
            state: "unknown",
            pid: 0,
        };
    }
    // SAFETY: snapshot creation; the handle is owned and closed on drop.
    let snapshot = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(handle) => OwnedHandle::new(handle).expect("snapshot handle"),
        Err(_) => {
            return ProcessState {
                state: "unknown",
                pid: 0,
            };
        }
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    struct Candidate {
        pid: u32,
        parent: u32,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut unknown = false;
    // SAFETY: the entry is initialized with its size; the snapshot handle is
    // owned for the whole enumeration.
    if unsafe { Process32FirstW(snapshot.raw(), &mut entry) }.is_err() {
        return ProcessState {
            state: "unknown",
            pid: 0,
        };
    }
    loop {
        let name = String::from_utf16_lossy(
            &entry.szExeFile[..entry
                .szExeFile
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.szExeFile.len())],
        );
        if name.eq_ignore_ascii_case(&target_name) {
            match open_process(entry.th32ProcessID) {
                None => unknown = true,
                Some(process) => {
                    // SAFETY: owned handle; zero timeout polls.
                    let wait = unsafe { WaitForSingleObject(process.raw(), 0) };
                    if wait == WAIT_OBJECT_0 {
                        // already exited
                    } else if wait == WAIT_TIMEOUT {
                        match image_path(process.raw()) {
                            None => unknown = true,
                            Some(path) => {
                                if normalized_windows_path(Path::new(&path))
                                    == normalized_windows_path(&desktop.executable)
                                {
                                    candidates.push(Candidate {
                                        pid: entry.th32ProcessID,
                                        parent: entry.th32ParentProcessID,
                                    });
                                }
                            }
                        }
                    } else {
                        return ProcessState {
                            state: "unknown",
                            pid: 0,
                        };
                    }
                }
            }
        }
        // SAFETY: continuation of the owned snapshot walk.
        if unsafe { Process32NextW(snapshot.raw(), &mut entry) }.is_err() {
            break;
        }
    }
    for candidate in &candidates {
        // Electron helper processes descend from a matching Desktop root; do
        // not report their PID.
        let child = candidates.iter().any(|other| other.pid == candidate.parent);
        if !child {
            return ProcessState {
                state: "running",
                pid: candidate.pid,
            };
        }
    }
    ProcessState {
        state: if unknown || !candidates.is_empty() {
            "unknown"
        } else {
            "stopped"
        },
        pid: 0,
    }
}

#[allow(dead_code)]
fn unused() {
    let _ = WAIT_TIMEOUT;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_running_under_its_own_image() {
        let desktop = Desktop {
            executable: std::env::current_exe().expect("current exe"),
            ..Desktop::default()
        };
        let state = desktop_process_state(&desktop);
        assert_eq!(state.state, "running");
        assert_eq!(state.pid, std::process::id());
    }

    #[test]
    fn absent_executable_is_stopped_or_unknown_never_running() {
        let desktop = Desktop {
            executable: std::env::temp_dir().join("cpg-definitely-absent-9527.exe"),
            ..Desktop::default()
        };
        let state = desktop_process_state(&desktop);
        assert_ne!(state.state, "running");
    }
}
