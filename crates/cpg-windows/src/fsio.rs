//! Bounded handle-based file I/O with the C++ read/write semantics:
//! size-checked whole reads from position zero, seek-zero whole writes with
//! end-of-file truncation and flush.

use crate::handles::OwnedHandle;
use cpg_domain::{Error, Result};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_BEGIN, FlushFileBuffers, GetFileSizeEx, ReadFile, SetEndOfFile, SetFilePointerEx,
    WriteFile,
};

pub fn read_raw(handle: HANDLE, limit: usize, code: &str) -> Result<Vec<u8>> {
    let fail = |message: &str| Error::new(code, message);
    let mut size: i64 = 0;
    // SAFETY: the handle is owned by the caller and still open; the size
    // pointer is a live local.
    if unsafe { GetFileSizeEx(handle, &mut size) }.is_err()
        || size < 0
        || size as u64 > limit as u64
    {
        return Err(fail("File is unreadable or exceeds its size limit."));
    }
    let length = size as usize;
    // SAFETY: seek to the beginning before reading.
    if unsafe { SetFilePointerEx(handle, 0, None, FILE_BEGIN) }.is_err() {
        return Err(fail("Cannot seek the file."));
    }
    let mut bytes = vec![0u8; length];
    let mut got = 0u32;
    if length > 0 {
        // SAFETY: the buffer is exactly sized; the count pointer is a local.
        if unsafe { ReadFile(handle, Some(&mut bytes), Some(&mut got), None) }.is_err()
            || got as usize != length
        {
            return Err(fail("Cannot read the file completely."));
        }
    }
    Ok(bytes)
}

/// Whole-file write from position zero: truncate to the written length and
/// flush. Mirrors the C++ `writeHandle` success contract.
pub fn write_raw(handle: HANDLE, bytes: &[u8]) -> bool {
    // SAFETY: all pointers are locals; the handle is owned by the caller and
    // still open for the whole call.
    unsafe {
        if SetFilePointerEx(handle, 0, None, FILE_BEGIN).is_err() {
            return false;
        }
        if !bytes.is_empty() {
            let mut wrote = 0u32;
            match WriteFile(handle, Some(bytes), Some(&mut wrote), None) {
                Ok(()) if wrote as usize == bytes.len() => {}
                _ => return false,
            }
        }
        SetEndOfFile(handle).is_ok() && FlushFileBuffers(handle).is_ok()
    }
}

/// Handle-creating open with explicit share mode and disposition; the
/// canonical building block for the engine's configuration lease and
/// transactions.
pub fn create_file(
    path: &std::path::Path,
    access: u32,
    share: windows::Win32::Storage::FileSystem::FILE_SHARE_MODE,
    disposition: windows::Win32::Storage::FileSystem::FILE_CREATION_DISPOSITION,
) -> Result<OwnedHandle> {
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: null-terminated path; the returned handle is owned.
    let handle = unsafe {
        windows::Win32::Storage::FileSystem::CreateFileW(
            PCWSTR(wide.as_ptr()),
            access,
            share,
            None,
            disposition,
            windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    OwnedHandle::new(handle.unwrap_or(windows::Win32::Foundation::HANDLE::default())).ok_or_else(
        || {
            Error::new(
                "CONFIG_LOCK_FAILED",
                "Configuration is busy, missing, or not writable.",
            )
        },
    )
}

/// Read-only shared open returning the raw Win32 error code on failure so
/// callers can distinguish "absent" from genuine I/O errors.
pub fn try_open_read_shared(path: &std::path::Path) -> std::result::Result<OwnedHandle, u32> {
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: null-terminated path; the returned handle is owned.
    let result = unsafe {
        windows::Win32::Storage::FileSystem::CreateFileW(
            PCWSTR(wide.as_ptr()),
            windows::Win32::Storage::FileSystem::FILE_GENERIC_READ.0,
            windows::Win32::Storage::FileSystem::FILE_SHARE_READ
                | windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE
                | windows::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
            None,
            windows::Win32::Storage::FileSystem::OPEN_EXISTING,
            windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match result {
        Ok(handle) => OwnedHandle::new(handle).ok_or(0),
        Err(_) => Err(unsafe { windows::Win32::Foundation::GetLastError() }.0),
    }
}
