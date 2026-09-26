//! Elevation policy for launching Desktop.
//!
//! Codex 0.157+ refuses to start its shared background server from an elevated
//! process, so Guard must run non-elevated as well. This mirrors the official
//! check (`OpenProcessToken` / `GetTokenInformation` / `TokenElevation`) instead
//! of shelling out to `whoami` or `net session`.

/// The message shown when Launch is attempted from an elevated Guard instance.
pub const ELEVATED_LAUNCH_UNSUPPORTED: &str = concat!(
    "ELEVATED_LAUNCH_UNSUPPORTED: Codex 0.157+ background server must be started ",
    "by a non-elevated process. Close this administrator instance and start ",
    "Codex Proxy Guard normally."
);

/// Returns `true` only when the current process token is elevated.
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned_length = 0u32;
        let queried = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_length,
        );
        CloseHandle(token);
        queried != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

/// Blocks Launch when Guard itself runs elevated. Guard never bypasses the
/// Codex daemon elevation requirement: no UAC automation, no token tricks.
pub fn ensure_non_elevated() -> Result<(), String> {
    if is_elevated() {
        Err(ELEVATED_LAUNCH_UNSUPPORTED.into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_elevated_process_is_allowed() {
        // Unit tests run non-elevated; real elevated behavior is verified by
        // the manual "Run as administrator" acceptance case.
        assert!(!is_elevated());
        assert!(ensure_non_elevated().is_ok());
    }

    #[test]
    fn blocked_message_is_actionable() {
        assert!(ELEVATED_LAUNCH_UNSUPPORTED.contains("ELEVATED_LAUNCH_UNSUPPORTED"));
        assert!(ELEVATED_LAUNCH_UNSUPPORTED.contains("non-elevated"));
        assert!(ELEVATED_LAUNCH_UNSUPPORTED.contains("start Codex Proxy Guard normally"));
    }
}
