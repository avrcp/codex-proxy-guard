//! Cooperative cancellation: a manual-reset Win32 event plus an atomic flag.
//! The event lets cancellation join `WaitForMultipleObjects` waits; copying
//! shares the state, mirroring the C++ semantics.

use crate::handles::OwnedHandle;
use cpg_domain::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{CreateEventW, SetEvent, WAIT_OBJECT_0};
use windows::Win32::System::Threading::WaitForSingleObject;

pub struct Cancellation {
    state: Arc<State>,
}

struct State {
    cancelled: AtomicBool,
    event: Mutex<Option<OwnedHandle>>,
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

impl Cancellation {
    pub fn new() -> Self {
        Cancellation {
            state: Arc::new(State {
                cancelled: AtomicBool::new(false),
                event: Mutex::new(None),
            }),
        }
    }

    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.state.event.lock() {
            if let Some(event) = guard.as_ref() {
                // SAFETY: the event handle is owned and valid.
                unsafe { SetEvent(event.raw()) };
            }
        }
    }

    pub fn cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<()> {
        if self.cancelled() {
            return Err(Error::new("LAUNCH_CANCELLED", "Operation cancelled."));
        }
        Ok(())
    }

    fn ensure_event(&self) -> Option<OwnedHandle> {
        let mut guard = self.state.event.lock().ok()?;
        if let Some(event) = guard.as_ref() {
            return Some(OwnedHandle::new(event.raw()).expect("known-valid"));
        }
        // SAFETY: manual-reset event creation with null security and owned
        // handles; failure simply degrades waits to flag polling.
        let handle = unsafe {
            CreateEventW(None, true, false, windows::core::w!(""))
        }
        .ok()
        .and_then(|h| OwnedHandle::new(h.0));
        *guard = handle.clone();
        handle
    }

    /// Raw event handle for `WaitForMultipleObjects`, or null when event
    /// creation failed (callers then poll the flag).
    pub fn event_handle(&self) -> Option<OwnedHandle> {
        self.ensure_event()
    }

    /// Waits up to `millis` for the event; returns true when cancelled.
    pub fn wait(&self, millis: u32) -> bool {
        let Some(event) = self.ensure_event() else {
            std::thread::sleep(std::time::Duration::from_millis(millis.min(10) as u64));
            return self.cancelled();
        };
        // SAFETY: owned valid handle.
        let result = unsafe { WaitForSingleObject(event.raw(), millis) };
        result == WAIT_OBJECT_0 || self.cancelled()
    }
}

impl Clone for Cancellation {
    fn clone(&self) -> Self {
        Cancellation {
            state: Arc::clone(&self.state),
        }
    }
}
