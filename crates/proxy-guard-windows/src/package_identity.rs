//! Queries the OS-assigned package identity of an exact process handle.
//! A missing identity is distinct from an unavailable query.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE};
use windows_sys::Win32::Storage::Packaging::Appx::GetPackageFullName;
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

const APPMODEL_ERROR_NO_PACKAGE: u32 = 15700;
const MAX_PACKAGE_NAME_UNITS: u32 = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessPackageIdentity {
    Packaged(String),
    Unpackaged,
}

pub fn query_process_package_identity(
    handle: &impl AsRawHandle,
) -> Result<ProcessPackageIdentity, String> {
    query_handle(handle.as_raw_handle() as HANDLE)
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

pub fn query_pid_package_identity(pid: u32) -> Result<ProcessPackageIdentity, String> {
    let handle = open_process_for_identity(pid)?;
    query_process_package_identity(&handle)
}

fn query_handle(handle: HANDLE) -> Result<ProcessPackageIdentity, String> {
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

pub fn require_expected_identity(
    handle: &impl AsRawHandle,
    expected_full_name: &str,
    pid: u32,
) -> Result<(), String> {
    match query_process_package_identity(handle)? {
        ProcessPackageIdentity::Packaged(actual) if actual == expected_full_name => Ok(()),
        ProcessPackageIdentity::Packaged(actual) => Err(format!(
            "APPX_IDENTITY_MISMATCH: target PID {pid} has package {actual}, expected {expected_full_name}; Desktop may already have been created"
        )),
        ProcessPackageIdentity::Unpackaged => Err(format!(
            "APPX_IDENTITY_MISSING: target PID {pid} has no package identity; Desktop may already have been created"
        )),
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
    fn invalid_pid_is_query_error_not_missing_identity() {
        assert!(
            open_process_for_identity(0)
                .unwrap_err()
                .contains("APPX_IDENTITY_QUERY_FAILED")
        );
    }
}
