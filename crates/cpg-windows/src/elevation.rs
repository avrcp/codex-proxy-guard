//! Token elevation queries and the anti-elevation launch gate.

use crate::handles::OwnedHandle;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `not_elevated` / `elevated` / `unknown`; a failed query is never treated
/// as safe.
fn token_elevation_of(process: windows::Win32::Foundation::HANDLE) -> &'static str {
    let mut token = windows::Win32::Foundation::HANDLE::default();
    // SAFETY: the handle is valid and owned by the caller; the out-pointer is
    // a local.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.is_err() {
        return "unknown";
    }
    let token = OwnedHandle::new(token).expect("OpenProcessToken returned a valid handle");
    let mut elevation = TOKEN_ELEVATION::default();
    let mut size = 0u32;
    // SAFETY: owned token handle; the buffer is exactly TOKEN_ELEVATION-sized.
    let ok = unsafe {
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
    };
    if ok.is_err() || size as usize != std::mem::size_of::<TOKEN_ELEVATION>() {
        return "unknown";
    }
    if elevation.TokenIsElevated != 0 {
        "elevated"
    } else {
        "not_elevated"
    }
}

/// `true` when elevated, `false` when verifiably not, `None` on query
/// failure (the C++ processElevation JSON observation).
pub fn process_elevated(process: windows::Win32::Foundation::HANDLE) -> Option<bool> {
    match token_elevation_of(process) {
        "elevated" => Some(true),
        "not_elevated" => Some(false),
        _ => None,
    }
}

pub fn elevation() -> &'static str {
    // SAFETY: pseudo-handle for the current process.
    token_elevation_of(unsafe { GetCurrentProcess() })
}

/// The launch gate: refuse when elevated, and refuse when the query itself
/// failed — mirroring `requireNonElevated`.
pub fn require_not_elevated() -> cpg_domain::Result<()> {
    match elevation() {
        "elevated" => Err(cpg_domain::Error::new(
            "GUARD_ELEVATED",
            "Launch requires a non-elevated Guard process",
        )),
        "not_elevated" => Ok(()),
        _ => Err(cpg_domain::Error::new(
            "GUARD_ELEVATION_QUERY_FAILED",
            "Cannot verify that Guard is not elevated",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_definite_state_for_this_process() {
        // The invariant is that the query itself resolves (never unknown);
        // the launch gate only allows the verifiably non-elevated case.
        match elevation() {
            "not_elevated" => assert!(require_not_elevated().is_ok()),
            "elevated" => {
                let error = require_not_elevated().unwrap_err();
                assert_eq!(error.code, "GUARD_ELEVATED");
            }
            other => panic!("elevation query must resolve, got {other}"),
        }
    }
}
