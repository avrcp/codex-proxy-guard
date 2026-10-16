//! Cooperative cancellation: a manual-reset Win32 event plus an atomic flag.
//! The event lets cancellation join waits; copying shares the state,
//! mirroring the C++ semantics.

use crate::handles::OwnedHandle;
use cpg_domain::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

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
        if let Ok(guard) = self.state.event.lock()
            && let Some(event) = guard.as_ref()
        {
            // SAFETY: the event handle is owned and valid.
            unsafe { SetEvent(event.raw()) }.ok();
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

    fn ensure_event(&self) -> Option<EventRef> {
        let mut guard = self.state.event.lock().ok()?;
        if guard.is_none() {
            // SAFETY: manual-reset event creation with no name; the returned
            // handle is owned. Failure degrades waits to flag polling.
            let handle = unsafe { CreateEventW(None, true, false, None) }
                .ok()
                .and_then(OwnedHandle::new);
            *guard = handle;
        }
        guard.as_ref().map(|event| EventRef(event.raw()))
    }

    /// Raw event handle for waits, or `None` when creation failed
    /// (callers then poll the flag). The handle stays owned by the
    /// cancellation state.
    pub fn event_handle(&self) -> Option<EventRef> {
        self.ensure_event()
    }

    /// Waits up to `millis` for the event; returns true when cancelled.
    pub fn wait(&self, millis: u32) -> bool {
        let Some(event) = self.ensure_event() else {
            std::thread::sleep(std::time::Duration::from_millis(millis.min(10) as u64));
            return self.cancelled();
        };
        // SAFETY: the state-owned handle is valid for the whole call.
        let result = unsafe { WaitForSingleObject(event.raw(), millis) };
        result == WAIT_OBJECT_0 || self.cancelled()
    }
}

/// Borrowed view of the cancellation event handle; ownership stays with the
/// `Cancellation` state.
pub struct EventRef(windows::Win32::Foundation::HANDLE);

impl EventRef {
    pub fn raw(&self) -> windows::Win32::Foundation::HANDLE {
        self.0
    }
}

impl Clone for Cancellation {
    fn clone(&self) -> Self {
        Cancellation {
            state: Arc::clone(&self.state),
        }
    }
}

#[allow(dead_code)]
fn sink() {
    let _ = WAIT_TIMEOUT;
}
