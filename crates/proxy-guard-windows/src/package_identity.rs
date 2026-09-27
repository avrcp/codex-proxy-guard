//! OS-assigned identity and token observations for an exact process handle.
//! A missing identity is always distinct from an unavailable query, and a
//! Win32 status code is never reported as if it were an HRESULT.

use std::{
    ffi::OsString,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
};

use windows_sys::Win32::{
    Foundation::{
        APPMODEL_ERROR_NO_APPLICATION, APPMODEL_ERROR_NO_PACKAGE, CloseHandle,
        ERROR_INSUFFICIENT_BUFFER, FILETIME, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    Storage::Packaging::Appx::{GetApplicationUserModelId, GetPackageFullName},
    System::SystemInformation::GetSystemTimeAsFileTime,
    System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
        WaitForSingleObject,
    },
};

const MAX_PACKAGE_NAME_UNITS: u32 = 1024;
const MAX_AUMID_UNITS: u32 = 1024;
/// FILETIME ticks between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessPackageIdentity {
    Packaged(String),
    Unpackaged,
}

/// Application identity of a process. `None` means the OS answered "this
/// process has no application entry" (`APPMODEL_ERROR_NO_APPLICATION`), which
/// is a valid observation and not a query failure.
pub type ProcessAumid = Option<String>;

pub fn query_process_package_identity(
    handle: &impl AsRawHandle,
) -> Result<ProcessPackageIdentity, String> {
    query_handle_package(handle.as_raw_handle() as HANDLE)
}

pub fn open_process_for_identity(pid: u32) -> Result<OwnedHandle, String> {
    if pid == 0 {
        return Err("APPX_IDENTITY_QUERY_FAILED: invalid process ID".into());
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(format!(
            "APPX_IDENTITY_QUERY_FAILED: OpenProcess({pid}) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// Opens a target for both identity queries and bounded alive observation.
/// The handle is held for the whole observation so a reused PID cannot be
/// confused with the activated target.
pub fn open_process_for_observation(pid: u32) -> Result<OwnedHandle, String> {
    if pid == 0 {
        return Err("APPX_TARGET_VERIFY_FAILED: invalid target PID".into());
    }
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(format!(
            "APPX_TARGET_VERIFY_FAILED: cannot open target PID {pid} after activation: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

pub fn query_pid_package_identity(pid: u32) -> Result<ProcessPackageIdentity, String> {
    let handle = open_process_for_identity(pid)?;
    query_process_package_identity(&handle)
}

fn query_handle_package(handle: HANDLE) -> Result<ProcessPackageIdentity, String> {
    let mut length = 0_u32;
    let status = unsafe { GetPackageFullName(handle, &mut length, std::ptr::null_mut()) };
    if status == APPMODEL_ERROR_NO_PACKAGE {
        return Ok(ProcessPackageIdentity::Unpackaged);
    }
    if status != ERROR_INSUFFICIENT_BUFFER || !(2..=MAX_PACKAGE_NAME_UNITS).contains(&length) {
        return Err(format!(
            "APPX_IDENTITY_QUERY_FAILED: GetPackageFullName size query returned Win32 status {status}, length {length}"
        ));
    }

    let mut buffer = vec![0_u16; length as usize];
    let status = unsafe { GetPackageFullName(handle, &mut length, buffer.as_mut_ptr()) };
    if status != 0 {
        return Err(format!(
            "APPX_IDENTITY_QUERY_FAILED: GetPackageFullName returned Win32 status {status}"
        ));
    }
    if !(2..=MAX_PACKAGE_NAME_UNITS).contains(&length) || buffer[(length - 1) as usize] != 0 {
        return Err("APPX_IDENTITY_QUERY_FAILED: malformed package name length".into());
    }
    let name = String::from_utf16(&buffer[..(length - 1) as usize])
        .map_err(|_| "APPX_IDENTITY_QUERY_FAILED: package name is not UTF-16".to_string())?;
    if name.is_empty() {
        return Err("APPX_IDENTITY_QUERY_FAILED: empty package name".into());
    }
    Ok(ProcessPackageIdentity::Packaged(name))
}

/// Queries the application user model ID of a process handle. Returns
/// `Ok(None)` when the OS reports that this process was not started through
/// an application-model activation (`APPMODEL_ERROR_NO_APPLICATION`), which
/// includes ordinary unpackaged processes (`APPMODEL_ERROR_NO_PACKAGE`).
pub fn query_process_aumid(handle: &impl AsRawHandle) -> Result<ProcessAumid, String> {
    let handle = handle.as_raw_handle() as HANDLE;
    let mut length = 0_u32;
    let status = unsafe { GetApplicationUserModelId(handle, &mut length, std::ptr::null_mut()) };
    if status == APPMODEL_ERROR_NO_APPLICATION || status == APPMODEL_ERROR_NO_PACKAGE {
        return Ok(None);
    }
    if status != ERROR_INSUFFICIENT_BUFFER || !(2..=MAX_AUMID_UNITS).contains(&length) {
        return Err(format!(
            "APPX_AUMID_QUERY_FAILED: GetApplicationUserModelId size query returned Win32 status {status}, length {length}"
        ));
    }
    let mut buffer = vec![0_u16; length as usize];
    let status = unsafe { GetApplicationUserModelId(handle, &mut length, buffer.as_mut_ptr()) };
    if status != 0 {
        return Err(format!(
            "APPX_AUMID_QUERY_FAILED: GetApplicationUserModelId returned Win32 status {status}"
        ));
    }
    if !(2..=MAX_AUMID_UNITS).contains(&length) || buffer[(length - 1) as usize] != 0 {
        return Err("APPX_AUMID_QUERY_FAILED: malformed AUMID length".into());
    }
    let aumid = String::from_utf16(&buffer[..(length - 1) as usize])
        .map_err(|_| "APPX_AUMID_QUERY_FAILED: AUMID is not UTF-16".to_string())?;
    if aumid.is_empty() {
        return Err("APPX_AUMID_QUERY_FAILED: empty AUMID".into());
    }
    Ok(Some(aumid))
}

/// Token elevation of an arbitrary process handle. A failed query is an error;
/// callers decide whether to record `None` or block on it.
pub fn query_process_elevation(handle: &impl AsRawHandle) -> Result<bool, String> {
    struct TokenHandle(HANDLE);
    impl Drop for TokenHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    let mut raw_token: HANDLE = std::ptr::null_mut();
    if unsafe {
        OpenProcessToken(
            handle.as_raw_handle() as HANDLE,
            TOKEN_QUERY,
            &mut raw_token,
        )
    } == 0
    {
        return Err(format!(
            "TARGET_ELEVATION_QUERY_FAILED: OpenProcessToken failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let token = TokenHandle(raw_token);
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned_length = 0_u32;
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            &mut elevation as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_length,
        )
    } == 0
        || (returned_length as usize) < std::mem::size_of::<TOKEN_ELEVATION>()
    {
        return Err(format!(
            "TARGET_ELEVATION_QUERY_FAILED: GetTokenInformation failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(elevation.TokenIsElevated != 0)
}

/// Process creation time in Unix milliseconds. Used to tell a freshly created
/// instance from one the activation request merely returned.
pub fn query_process_creation_time_unix_ms(handle: &impl AsRawHandle) -> Result<u64, String> {
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut kernel = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut user = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    if unsafe {
        GetProcessTimes(
            handle.as_raw_handle() as HANDLE,
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(format!(
            "APPX_TARGET_OBSERVATION_FAILED: GetProcessTimes failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let ticks = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
    if ticks == 0 {
        return Err(
            "APPX_TARGET_OBSERVATION_FAILED: GetProcessTimes returned a zero creation time".into(),
        );
    }
    ticks
        .checked_sub(FILETIME_UNIX_EPOCH_TICKS)
        .map(|ticks| ticks / 10_000)
        .ok_or_else(|| {
            "APPX_TARGET_OBSERVATION_FAILED: creation time predates the Unix epoch".into()
        })
}

/// Current system time in Unix milliseconds from the same timebase as
/// [`query_process_creation_time_unix_ms`], so the two can be compared.
pub fn query_system_time_unix_ms() -> u64 {
    let mut time = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    unsafe { GetSystemTimeAsFileTime(&mut time) };
    let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    ticks
        .checked_sub(FILETIME_UNIX_EPOCH_TICKS)
        .map(|ticks| ticks / 10_000)
        .unwrap_or(0)
}

/// Full image path of a process handle, for verifying the activated target
/// really is the registered Desktop executable.
pub fn query_process_image_path(handle: &impl AsRawHandle) -> Result<PathBuf, String> {
    let handle = handle.as_raw_handle() as HANDLE;
    let mut size = 32_768_u32;
    let mut buffer = vec![0_u16; size as usize];
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    if ok == 0 || size == 0 || size as usize > buffer.len() {
        return Err(format!(
            "APPX_TARGET_OBSERVATION_FAILED: QueryFullProcessImageNameW failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..size as usize])))
}

/// Bounded alive/exit observation of a freshly activated target. `Ok(None)`
/// means the target was still alive after the window; `Ok(Some(code))` means
/// it exited during the window with that exit code.
pub fn observe_early_exit(
    handle: &impl AsRawHandle,
    window_ms: u32,
) -> Result<Option<u32>, String> {
    let handle = handle.as_raw_handle() as HANDLE;
    match unsafe { WaitForSingleObject(handle, window_ms) } {
        WAIT_TIMEOUT => Ok(None),
        WAIT_OBJECT_0 => {
            let mut code = 0_u32;
            if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
                return Err(format!(
                    "APPX_TARGET_OBSERVATION_FAILED: GetExitCodeProcess failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Some(code))
        }
        WAIT_FAILED => Err(format!(
            "APPX_TARGET_OBSERVATION_FAILED: WaitForSingleObject failed: {}",
            std::io::Error::last_os_error()
        )),
        unexpected => Err(format!(
            "APPX_TARGET_OBSERVATION_FAILED: unexpected wait result {unexpected}"
        )),
    }
}

/// Compares two paths as the same Windows location. Both plain and
/// extended-length (`\\?\C:\...`, `\\?\UNC\server\share\...`) spellings of the
/// same location compare equal; comparison is ASCII case-insensitive like
/// Windows itself.
pub fn windows_paths_equal(left: &Path, right: &Path) -> bool {
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
    fn current_test_process_has_no_package_identity() {
        assert_eq!(
            query_pid_package_identity(std::process::id()).unwrap(),
            ProcessPackageIdentity::Unpackaged
        );
    }

    #[test]
    fn current_test_process_has_no_application_identity() {
        let handle = open_process_for_identity(std::process::id()).unwrap();
        assert_eq!(query_process_aumid(&handle).unwrap(), None);
    }

    #[test]
    fn invalid_pid_is_query_error_not_missing_identity() {
        assert!(
            open_process_for_identity(0)
                .unwrap_err()
                .contains("APPX_IDENTITY_QUERY_FAILED")
        );
    }

    #[test]
    fn current_test_process_reports_creation_time_and_elevation() {
        let handle = open_process_for_identity(std::process::id()).unwrap();
        let created = query_process_creation_time_unix_ms(&handle).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(
            created <= now + 5_000 && now.saturating_sub(created) < 24 * 3600 * 1000,
            "creation time {created} should be close to now {now}"
        );
        assert!(!query_process_elevation(&handle).unwrap());
    }

    #[test]
    fn current_test_process_is_alive_within_the_observation_window() {
        // Observation needs a SYNCHRONIZE-capable handle, like the
        // activation worker opens for a returned target.
        let handle = open_process_for_observation(std::process::id()).unwrap();
        assert_eq!(observe_early_exit(&handle, 10).unwrap(), None);
    }

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
                windows_paths_equal(Path::new(left), Path::new(right)),
                "{left} vs {right}"
            );
        }
    }
}
