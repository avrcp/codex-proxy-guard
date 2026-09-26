//! Elevation policy for launching Desktop.
//!
//! Codex's managed background server refuses to start from an elevated
//! process, so Guard adopts the conservative policy of running non-elevated.
//! The check mirrors the official implementation (`OpenProcessToken` /
//! `GetTokenInformation` / `TokenElevation`) and is fallible: a failed query
//! blocks the launch instead of being interpreted as "not elevated".

/// The message shown when Launch is attempted from an elevated Guard instance.
pub const ELEVATED_LAUNCH_UNSUPPORTED: &str = concat!(
    "ELEVATED_LAUNCH_UNSUPPORTED: Codex 0.157+ background server must be started ",
    "by a non-elevated process. Close this administrator instance and start ",
    "Codex Proxy Guard normally."
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElevationState {
    Elevated,
    NotElevated,
}

/// Queries the current process token. Every Win32 failure — including a
/// truncated result — is an error; it must never be folded into
/// `NotElevated`.
#[cfg(windows)]
pub fn query_elevation() -> Result<ElevationState, String> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    struct TokenHandle(windows_sys::Win32::Foundation::HANDLE);

    impl Drop for TokenHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    unsafe {
        let mut raw_token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) == 0 {
            return Err(format!(
                "OpenProcessToken failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let token = TokenHandle(raw_token);
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned_length = 0u32;
        if GetTokenInformation(
            token.0,
            TokenElevation,
            &mut elevation as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_length,
        ) == 0
        {
            return Err(format!(
                "GetTokenInformation failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        if (returned_length as usize) < std::mem::size_of::<TOKEN_ELEVATION>() {
            return Err(
                "GetTokenInformation returned an unexpectedly short TokenElevation result".into(),
            );
        }
        Ok(if elevation.TokenIsElevated != 0 {
            ElevationState::Elevated
        } else {
            ElevationState::NotElevated
        })
    }
}

#[cfg(not(windows))]
pub fn query_elevation() -> Result<ElevationState, String> {
    Ok(ElevationState::NotElevated)
}

/// Maps a query outcome onto the launch decision. Pure so the three rows of
/// the policy can be unit-tested without depending on the test host's token.
pub fn elevation_gate(state: Result<ElevationState, String>) -> Result<(), String> {
    match state {
        Ok(ElevationState::NotElevated) => Ok(()),
        Ok(ElevationState::Elevated) => Err(ELEVATED_LAUNCH_UNSUPPORTED.into()),
        Err(detail) => Err(format!("ELEVATION_QUERY_FAILED: {detail}")),
    }
}

/// Blocks Launch unless Guard verifiably runs non-elevated. Guard never
/// bypasses the Codex elevation requirement: no UAC automation, no token
/// tricks.
pub fn ensure_non_elevated() -> Result<(), String> {
    elevation_gate(query_elevation())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_confirmed_non_elevated_state_allows_launch() {
        assert!(elevation_gate(Ok(ElevationState::NotElevated)).is_ok());
        let elevated = elevation_gate(Ok(ElevationState::Elevated)).unwrap_err();
        assert!(elevated.contains("ELEVATED_LAUNCH_UNSUPPORTED"));
        let failed = elevation_gate(Err("OpenProcessToken failed: 5".into())).unwrap_err();
        assert!(failed.contains("ELEVATION_QUERY_FAILED"));
        assert!(failed.contains("OpenProcessToken"));
    }

    #[test]
    fn non_elevated_test_host_passes_the_real_query() {
        // Real Win32 query smoke for the non-elevated case; the elevated and
        // failed rows are covered by the pure gate test above and by the
        // manual "Run as administrator" acceptance case.
        assert_eq!(
            query_elevation(),
            Ok(ElevationState::NotElevated),
            "tests must run non-elevated; the manual acceptance case covers elevation"
        );
    }

    #[test]
    fn blocked_message_is_actionable() {
        assert!(ELEVATED_LAUNCH_UNSUPPORTED.contains("ELEVATED_LAUNCH_UNSUPPORTED"));
        assert!(ELEVATED_LAUNCH_UNSUPPORTED.contains("non-elevated"));
        assert!(ELEVATED_LAUNCH_UNSUPPORTED.contains("start Codex Proxy Guard normally"));
    }
}
