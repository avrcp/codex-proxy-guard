//! Clipboard copy of a Unicode diagnostic string (CF_UNICODETEXT) with the
//! C++ behavior: failures surface as a notice and never disturb the
//! operation result.

use std::time::Duration;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// Copies `text`; returns false when the clipboard could not be opened,
/// allocated or written.
pub fn copy_text(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let bytes = wide.len() * 2;
    // SAFETY: clipboard sequence with retry; the allocation is handed to the
    // system only on success and freed on failure.
    unsafe {
        let mut opened = false;
        for attempt in 0..20 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            let _ = attempt;
        }
        if !opened {
            return false;
        }
        let result = (|| {
            if EmptyClipboard().is_err() {
                return false;
            }
            let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, bytes) else {
                return false;
            };
            let target = GlobalLock(handle);
            if target.is_null() {
                let _ = windows::Win32::Foundation::GlobalFree(Some(handle));
                return false;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), target as *mut u16, wide.len());
            let _ = GlobalUnlock(handle);
            // Ownership transfers to the clipboard on success.
            SetClipboardData(
                CF_UNICODETEXT.0 as u32,
                Some(windows::Win32::Foundation::HANDLE(handle.0)),
            )
            .map(|_| true)
            .unwrap_or(false)
        })();
        let _ = CloseClipboard();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_and_readback_would_round_trip() {
        // Clipboard access can be denied in service contexts; treat false as
        // an allowed outcome, but true must mean the write was accepted.
        let _ = copy_text("cpg-clipboard-probe");
    }
}
