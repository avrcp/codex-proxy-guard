//! Locate trusted Windows tools from the OS-reported system directory.

use std::{os::windows::ffi::OsStringExt, path::PathBuf};

use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

pub(crate) fn system_powershell() -> Result<PathBuf, String> {
    // The directory is OS-owned metadata. WINDIR and SYSTEMROOT are inherited
    // process variables and are not suitable for authenticating an executable.
    let required = unsafe { GetSystemDirectoryW(std::ptr::null_mut(), 0) };
    if required == 0 || required > 32_767 {
        return Err("cannot query the Windows system directory".into());
    }
    let mut buffer = vec![0u16; required as usize + 1];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return Err("Windows system directory changed during query".into());
    }
    buffer.truncate(length as usize);
    let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer))
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    if !path.is_file() {
        return Err("system Windows PowerShell was not found".into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::os::windows::ffi::OsStringExt;

    #[test]
    fn powershell_is_resolved_from_os_system_directory() {
        let resolved = super::system_powershell().expect("system PowerShell");
        let required = unsafe {
            windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
                std::ptr::null_mut(),
                0,
            )
        };
        let mut buffer = vec![0u16; required as usize + 1];
        let length = unsafe {
            windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            )
        };
        buffer.truncate(length as usize);
        let system_dir = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer));
        assert!(resolved.starts_with(system_dir));
        assert_eq!(resolved.file_name().unwrap(), "powershell.exe");
    }
}
