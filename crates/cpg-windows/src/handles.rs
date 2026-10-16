//! Owned Win32 HANDLE with RAII close and an explicit null/invalid split,
//! mirroring the C++ `UniqueHandle`.

use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// Takes ownership of a handle that is not `INVALID_HANDLE_VALUE`; a
    /// null handle is a legitimate "absent" observation and stays distinct.
    pub fn new(handle: HANDLE) -> Option<Self> {
        (handle != HANDLE::default() && handle != INVALID_HANDLE_VALUE).then(|| OwnedHandle(handle))
    }

    pub fn invalid() -> Self {
        OwnedHandle(INVALID_HANDLE_VALUE)
    }

    pub fn is_valid(&self) -> bool {
        self.0 != HANDLE::default() && self.0 != INVALID_HANDLE_VALUE
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.is_valid() {
            // SAFETY: the handle is non-null/non-invalid and owned exclusively
            // by this instance; every constructor path comes from a Win32 API
            // that returned an owned handle to this process.
            unsafe { CloseHandle(self.0) };
        }
    }
}

impl Send for OwnedHandle {}
impl Sync for OwnedHandle {}
