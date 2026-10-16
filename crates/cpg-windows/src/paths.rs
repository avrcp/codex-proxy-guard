//! Windows path primitives: known folders, QDir-cleanPath-equivalent
//! normalization with case folding, canonical paths via final-path queries
//! on held handles, containment checks, and the current executable path.

use crate::handles::OwnedHandle;
use cpg_domain::{Error, Result};
use std::path::{Path, PathBuf};
use windows::Win32::Storage::FileSystem::GetFileInformationByHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_CREATION_DISPOSITION, FILE_FLAG_BACKUP_SEMANTICS, FILE_NAME_NORMALIZED,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFinalPathNameByHandleW, OPEN_EXISTING,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::UI::Shell::{FOLDERID_Profile, FOLDERID_RoamingAppData, SHGetKnownFolderPath};
use windows::core::PCWSTR;

pub fn known_folder_roaming_app_data() -> Result<PathBuf> {
    known_folder(&FOLDERID_RoamingAppData)
}

pub fn default_config_path() -> Result<PathBuf> {
    Ok(known_folder_roaming_app_data()?
        .join("codex-proxy-guard")
        .join("config.toml"))
}

pub fn default_home() -> Result<PathBuf> {
    Ok(known_folder(&FOLDERID_Profile)?.join(".codex"))
}

fn known_folder(id: &windows::core::GUID) -> Result<PathBuf> {
    // SAFETY: read-only known-folder query with a null token; the returned
    // CoTaskMem string is freed below exactly once.
    let wide = unsafe { SHGetKnownFolderPath(id, Default::default(), None) }.map_err(|_| {
        Error::new(
            "CONFIG_PATH_UNAVAILABLE",
            "Cannot resolve the user directory.",
        )
    })?;
    let path = unsafe { wide.to_string() }.map_err(|_| {
        Error::new(
            "CONFIG_PATH_UNAVAILABLE",
            "Cannot resolve the user directory.",
        )
    })?;
    // SAFETY: freeing the allocation returned by SHGetKnownFolderPath.
    unsafe { CoTaskMemFree(Some(wide.as_ptr() as *const core::ffi::c_void)) };
    Ok(PathBuf::from(path))
}

/// QDir::cleanPath over the backslash-folded form, then invariant lowercase
/// — the comparison form used everywhere in the product.
pub fn normalized_windows_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut folded = text;
    if folded.len() > 8 && folded[..8].eq_ignore_ascii_case("//?/unc/") {
        folded = format!("//{}", &folded[8..]);
    } else if folded.len() > 4 && folded[..4].eq_ignore_ascii_case("//?/") {
        folded = folded[4..].to_owned();
    }
    fold_case(&clean_forward_path(&folded))
}

fn fold_case(text: &str) -> String {
    // LCMapStringW(LOCALE_INVARIANT, LCMAP_LOWERCASE) is an ASCII-fold for
    // the invariant locale — exactly the path comparison we need.
    text.to_lowercase()
}

fn clean_forward_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let leading_slashes = path.chars().take_while(|c| *c == '/').count();
    let rooted = leading_slashes > 0;
    let raw: Vec<&str> = path[leading_slashes..]
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let mut kept: Vec<&str> = Vec::new();
    let drive_root = !raw.is_empty() && raw[0].len() == 2 && raw[0].ends_with(':');
    let root_size = if leading_slashes >= 2 {
        2
    } else if drive_root {
        1
    } else {
        0
    };
    for segment in raw {
        if segment == "." {
            continue;
        }
        if segment == ".." {
            if kept.len() > root_size && kept.last() != Some(&"..") {
                kept.pop();
                continue;
            }
            if rooted || (root_size > 0 && kept.len() >= root_size) {
                continue;
            }
            kept.push(segment);
            continue;
        }
        kept.push(segment);
    }
    let mut result = String::new();
    if leading_slashes >= 2 {
        result = "//".to_owned();
    } else if leading_slashes == 1 {
        result = "/".to_owned();
    }
    for segment in kept {
        if !result.is_empty() && !result.ends_with('/') {
            result.push('/');
        }
        result.push_str(segment);
    }
    if result.is_empty() {
        return if rooted {
            if leading_slashes >= 2 {
                "//".to_owned()
            } else {
                "/".to_owned()
            }
        } else {
            ".".to_owned()
        };
    }
    result
}

pub fn path_within(root: &Path, child: &Path) -> bool {
    let root = normalized_windows_path(root);
    let child = normalized_windows_path(child);
    root == child
        || (child.len() > root.len()
            && child.starts_with(&root)
            && child.as_bytes()[root.len()] == b'/')
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect()
}

fn open_attributes(path: &Path) -> Option<OwnedHandle> {
    let wide = wide_path(path);
    // SAFETY: null-terminated path; the returned handle is owned.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            FILE_CREATION_DISPOSITION(OPEN_EXISTING.0),
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .ok()?;
    OwnedHandle::new(handle)
}

fn information_of(
    handle: &OwnedHandle,
) -> Option<windows::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    let mut info = windows::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: owned handle; the info pointer is a live local.
    if unsafe { GetFileInformationByHandle(handle.raw(), &mut info) }.is_err() {
        return None;
    }
    Some(info)
}

/// Canonicalized final path of an existing file or directory. When
/// `directory` is set the entry must be a real directory; otherwise it must
/// be a regular file that is not a reparse point (junction/symlink).
pub fn canonical_path_of(path: &Path, directory: bool) -> Option<PathBuf> {
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    };
    let handle = open_attributes(path)?;
    let info = information_of(&handle)?;
    if directory {
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == FILE_ATTRIBUTE_DIRECTORY.0 {
            // ok
        } else {
            return None;
        }
    } else if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != FILE_ATTRIBUTE_DIRECTORY.0
        && info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != FILE_ATTRIBUTE_REPARSE_POINT.0
    {
        // ok
    } else {
        return None;
    }
    final_path_of(handle)
}

fn final_path_of(handle: OwnedHandle) -> Option<PathBuf> {
    // SAFETY: owned handle; the slice carries its own length in and the
    // written length out.
    let size = unsafe {
        let mut probe = [0u16; 1];
        GetFinalPathNameByHandleW(handle.raw(), &mut probe, FILE_NAME_NORMALIZED)
    };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; size as usize];
    let written =
        unsafe { GetFinalPathNameByHandleW(handle.raw(), &mut buffer, FILE_NAME_NORMALIZED) };
    if written == 0 || written as usize >= buffer.len() {
        return None;
    }
    buffer.truncate(written as usize);
    let text = String::from_utf16(&buffer).ok()?;
    let text = if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = text.strip_prefix("\\\\?\\") {
        rest.to_owned()
    } else {
        text
    };
    Some(PathBuf::from(text))
}

/// The running executable path (`GetModuleFileNameW`, growing buffer).
pub fn current_executable_path() -> Result<PathBuf> {
    let mut capacity = 260usize;
    loop {
        let mut buffer = vec![0u16; capacity];
        // SAFETY: module null means the current executable; the slice
        // carries the buffer capacity.
        let size = unsafe { GetModuleFileNameW(None, &mut buffer) };
        if size == 0 {
            return Err(Error::new(
                "EXECUTABLE_PATH_UNAVAILABLE",
                "Cannot resolve the running executable.",
            ));
        }
        if (size as usize) < capacity {
            buffer.truncate(size as usize);
            return Ok(PathBuf::from(String::from_utf16_lossy(&buffer)));
        }
        capacity *= 2;
    }
}

/// Production `FsProbe` used by discovery selection.
pub struct NativeFsProbe;

impl cpg_domain::discovery::FsProbe for NativeFsProbe {
    fn is_directory(&self, path: &Path) -> bool {
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
        open_attributes(path)
            .as_ref()
            .and_then(information_of)
            .is_some_and(|info| {
                info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == FILE_ATTRIBUTE_DIRECTORY.0
            })
    }

    fn is_regular_file(&self, path: &Path) -> bool {
        use windows::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        };
        open_attributes(path)
            .as_ref()
            .and_then(information_of)
            .is_some_and(|info| {
                info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != FILE_ATTRIBUTE_DIRECTORY.0
                    && info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0
                        != FILE_ATTRIBUTE_REPARSE_POINT.0
            })
    }

    fn canonical_file(&self, path: &Path) -> Option<PathBuf> {
        canonical_path_of(path, false)
    }

    fn canonical_directory(&self, path: &Path) -> Option<PathBuf> {
        canonical_path_of(path, true)
    }

    fn same_normalized_path(&self, left: &Path, right: &Path) -> bool {
        normalized_windows_path(left) == normalized_windows_path(right)
    }

    fn path_within(&self, root: &Path, child: &Path) -> bool {
        path_within(root, child)
    }
}

/// The per-user temp directory (`GetTempPathW`).
pub fn temp_path() -> Result<PathBuf> {
    // SAFETY: sizing call into the local buffer.
    let mut buffer = vec![0u16; 261];
    let size = unsafe { windows::Win32::Storage::FileSystem::GetTempPathW(Some(&mut buffer)) };
    if size == 0 || size as usize >= buffer.len() {
        return Err(Error::new("LAUNCH_BUSY", "Another Guard instance is launching, or the startup lock is unavailable."));
    }
    buffer.truncate(size as usize);
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_matches_cpp_forms() {
        assert_eq!(
            normalized_windows_path(Path::new(r"C:\Users\X\.codex")),
            "c:/users/x/.codex"
        );
        assert_eq!(normalized_windows_path(Path::new(r"\\?\C:\A\B\")), "c:/a/b");
        assert_eq!(
            normalized_windows_path(Path::new(r"\\?\UNC\server\share\p")),
            "//server/share/p"
        );
        assert_eq!(
            normalized_windows_path(Path::new(r"C:\A\.\B\..\C")),
            "c:/a/c"
        );
        assert_eq!(normalized_windows_path(Path::new(r"..\x")), "../x");
    }

    #[test]
    fn containment_rules() {
        assert!(path_within(Path::new(r"C:\A"), Path::new(r"c:\a\b\c")));
        assert!(path_within(Path::new(r"C:\A"), Path::new(r"C:\A")));
        assert!(!path_within(Path::new(r"C:\A"), Path::new(r"C:\AB")));
    }

    #[test]
    fn known_folders_resolve() {
        let config = default_config_path().unwrap();
        assert!(config.is_absolute());
        assert!(config.ends_with("config.toml"));
        let home = default_home().unwrap();
        assert!(home.ends_with(".codex"));
    }

    #[test]
    fn canonical_paths_of_real_files() {
        let exe = current_executable_path().unwrap();
        assert!(canonical_path_of(&exe, false).is_some());
        assert!(canonical_path_of(Path::new("C:\\nonexistent-dir"), true).is_none());
        use cpg_domain::discovery::FsProbe as _;
        assert!(NativeFsProbe.is_regular_file(&exe));
        assert!(!NativeFsProbe.is_directory(&exe));
        assert!(NativeFsProbe.is_directory(exe.parent().unwrap()));
    }
}
