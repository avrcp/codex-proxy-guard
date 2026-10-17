//! The bounded owned-process runner: only-inheritable duplicated pipe ends
//! via `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, concurrent stdout/stderr drain
//! threads, a cancellable stdin writer thread, hard settle of child and
//! threads before any verdict, and the exact failure taxonomy of the C++
//! `runOwnedProcess`. The runner only ever terminates processes it created
//! and still holds handles for.
//!
//! Reader/writer threads park in synchronous pipe I/O that terminating the
//! owned child cannot always break (a grandchild may still hold the write
//! end). Each I/O thread therefore registers its real native thread handle
//! so the settler can issue `CancelSynchronousIo`, which aborts the in-flight
//! `ReadFile`/`WriteFile` with `ERROR_OPERATION_ABORTED`; the loops treat
//! that abort as a stop once `stopping` is set. If even cancelled I/O cannot
//! be reaped within the settle budget the failure is reported explicitly
//! (`HELPER_CLEANUP_FAILED`) - threads are never detached and an unreaped
//! helper is never reported as success.

use crate::cancellation::Cancellation;
use crate::handles::OwnedHandle;
use cpg_domain::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{DuplicateHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetCurrentThread, GetExitCodeProcess,
    InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_CREATION_FLAGS,
    PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::PCWSTR;

pub const HELPER_REQUEST_LIMIT: usize = 16 * 1024;
const SETTLE_BUDGET: Duration = Duration::from_secs(3);
const CANCEL_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const WAIT_SLICE: Duration = Duration::from_millis(10);
const DRAIN_BUDGET: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default)]
pub struct ProcessResult {
    pub out: Vec<u8>,
    pub err: Vec<u8>,
    pub exit_code: i32,
    pub normal_exit: bool,
    pub submitted: bool,
}

fn fail<T>(code: &str, message: &str) -> Result<T> {
    Err(Error::new(code, message))
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().to_string_lossy().encode_utf16().collect()
}

/// Canonical Windows command-line quoting: 2n backslashes before a quote (or
/// at the end) become 2n+1; every argument is always wrapped in quotes.
pub fn append_quoted(command: &mut Vec<u16>, argument: &[u16]) {
    command.push('"' as u16);
    let mut backslashes = 0usize;
    for &unit in argument {
        if unit == '\\' as u16 {
            backslashes += 1;
            continue;
        }
        if unit == '"' as u16 {
            for _ in 0..(backslashes * 2 + 1) {
                command.push('\\' as u16);
            }
        } else {
            for _ in 0..backslashes {
                command.push('\\' as u16);
            }
        }
        backslashes = 0;
        command.push(unit);
    }
    for _ in 0..(backslashes * 2) {
        command.push('\\' as u16);
    }
    command.push('"' as u16);
}

struct Pipe {
    read: OwnedHandle,
    write: OwnedHandle,
}

fn create_pipe() -> Result<Pipe> {
    let mut read = windows::Win32::Foundation::HANDLE::default();
    let mut write = windows::Win32::Foundation::HANDLE::default();
    // SAFETY: local out-pointers; the pipes are non-inheritable by default.
    if unsafe { CreatePipe(&mut read, &mut write, None, 0) }.is_err() {
        return fail("HELPER_IO_FAILED", "Cannot create the helper pipes.");
    }
    let owned_read = OwnedHandle::new(read)
        .ok_or_else(|| Error::new("HELPER_IO_FAILED", "Cannot create the helper pipes."))?;
    let owned_write = OwnedHandle::new(write)
        .ok_or_else(|| Error::new("HELPER_IO_FAILED", "Cannot create the helper pipes."))?;
    Ok(Pipe {
        read: owned_read,
        write: owned_write,
    })
}

fn inheritable_duplicate(source: windows::Win32::Foundation::HANDLE) -> Result<OwnedHandle> {
    let mut duplicate = windows::Win32::Foundation::HANDLE::default();
    // SAFETY: duplicates within our own process; the new handle is
    // inheritable so only explicitly listed children can see it.
    let ok = unsafe {
        DuplicateHandle(
            windows::Win32::System::Threading::GetCurrentProcess(),
            source,
            windows::Win32::System::Threading::GetCurrentProcess(),
            &mut duplicate,
            0,
            true,
            windows::Win32::Foundation::DUPLICATE_SAME_ACCESS,
        )
    };
    if ok.is_err() {
        return fail("HELPER_IO_FAILED", "Cannot prepare the helper pipe ends.");
    }
    OwnedHandle::new(duplicate)
        .ok_or_else(|| Error::new("HELPER_IO_FAILED", "Cannot prepare the helper pipe ends."))
}

struct ReaderState {
    bytes: Mutex<Vec<u8>>,
    over_limit: AtomicBool,
    error: AtomicU32,
    eof: AtomicBool,
    done: AtomicBool,
}

impl ReaderState {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(ReaderState {
            bytes: Mutex::new(Vec::with_capacity(limit.min(1 << 20))),
            over_limit: AtomicBool::new(false),
            error: AtomicU32::new(0),
            eof: AtomicBool::new(false),
            done: AtomicBool::new(false),
        })
    }
}

/// A shared slot holding a real handle of one I/O thread. Rust
/// `JoinHandle`s expose no native thread handle, so each reader/writer
/// thread registers its own duplicated handle here for the settler's
/// `CancelSynchronousIo`.
#[derive(Default)]
struct IoThreadHandle {
    handle: Mutex<Option<OwnedHandle>>,
}

impl IoThreadHandle {
    /// Registers the calling thread's own handle (duplicated into a real
    /// handle valid process-wide). Best effort: when duplication fails the
    /// settler falls back to flag-based stopping only.
    fn register_current_thread(&self) {
        let mut duplicate = windows::Win32::Foundation::HANDLE::default();
        // SAFETY: duplicating the current thread's pseudo handle inside our
        // own process; the duplicate is owned by this slot.
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentThread(),
                GetCurrentProcess(),
                &mut duplicate,
                0,
                false,
                windows::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            )
        };
        if ok.is_ok()
            && let Some(owned) = OwnedHandle::new(duplicate)
        {
            *self.handle.lock().expect("io thread handle") = Some(owned);
        }
    }

    /// Cancels any synchronous I/O the thread is parked in; the aborted
    /// `ReadFile`/`WriteFile` returns `ERROR_OPERATION_ABORTED`, which the
    /// I/O loops treat as a stop once `stopping` is set.
    fn cancel_io(&self) {
        if let Ok(guard) = self.handle.lock()
            && let Some(handle) = guard.as_ref()
        {
            // SAFETY: a real handle of the live I/O thread owned by this slot.
            let _ = unsafe { CancelSynchronousIo(handle.raw()) };
        }
    }
}

fn spawn_reader(
    pipe_read: OwnedHandle,
    limit: usize,
    stopping: Arc<AtomicBool>,
) -> (
    std::thread::JoinHandle<()>,
    Arc<ReaderState>,
    Arc<IoThreadHandle>,
) {
    let state = ReaderState::new(limit);
    let worker_state = Arc::clone(&state);
    let io = Arc::new(IoThreadHandle::default());
    let thread_io = Arc::clone(&io);
    let handle = std::thread::spawn(move || {
        thread_io.register_current_thread();
        let mut buffer = [0u8; 4096];
        loop {
            if stopping.load(Ordering::SeqCst) {
                break;
            }
            let mut got = 0u32;
            // SAFETY: the read handle is owned by this thread; the buffer and
            // count pointer are locals.
            let result =
                unsafe { ReadFile(pipe_read.raw(), Some(&mut buffer), Some(&mut got), None) };
            match result {
                Ok(()) if got == 0 => {
                    worker_state.eof.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(()) => {
                    let mut bytes = worker_state.bytes.lock().expect("reader lock");
                    if got as usize > limit.saturating_sub(bytes.len()) {
                        worker_state.over_limit.store(true, Ordering::SeqCst);
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..got as usize]);
                }
                Err(error) => {
                    let code = (error.code().0 & 0xffff) as u32;
                    if code == windows::Win32::Foundation::ERROR_BROKEN_PIPE.0
                        || code == windows::Win32::Foundation::ERROR_NO_DATA.0
                    {
                        worker_state.eof.store(true, Ordering::SeqCst);
                    } else if !(code == windows::Win32::Foundation::ERROR_OPERATION_ABORTED.0
                        && stopping.load(Ordering::SeqCst))
                    {
                        worker_state.error.store(code, Ordering::SeqCst);
                    }
                    break;
                }
            }
        }
        worker_state.done.store(true, Ordering::SeqCst);
    });
    (handle, state, io)
}

fn spawn_writer(
    pipe_write: OwnedHandle,
    stdin_bytes: Vec<u8>,
    stopping: Arc<AtomicBool>,
    cancel: Cancellation,
    submitted: Arc<AtomicBool>,
    write_error: Arc<AtomicU32>,
) -> (std::thread::JoinHandle<()>, Arc<IoThreadHandle>) {
    let io = Arc::new(IoThreadHandle::default());
    let thread_io = Arc::clone(&io);
    let handle = std::thread::spawn(move || {
        thread_io.register_current_thread();
        let mut written = 0usize;
        while written < stdin_bytes.len() && !stopping.load(Ordering::SeqCst) && !cancel.cancelled()
        {
            let chunk = (stdin_bytes.len() - written).min(4096);
            let mut count = 0u32;
            // SAFETY: the write handle is owned by this thread.
            let ok = unsafe {
                WriteFile(
                    pipe_write.raw(),
                    Some(&stdin_bytes[written..written + chunk]),
                    Some(&mut count),
                    None,
                )
            };
            match ok {
                Ok(()) => {
                    if count == 0 || count as usize > chunk {
                        write_error.store(
                            windows::Win32::Foundation::ERROR_WRITE_FAULT.0,
                            Ordering::SeqCst,
                        );
                        break;
                    }
                    written += count as usize;
                }
                Err(error) => {
                    if !stopping.load(Ordering::SeqCst) {
                        let code = (error.code().0 & 0xffff) as u32;
                        write_error.store(
                            if code == 0 {
                                windows::Win32::Foundation::ERROR_WRITE_FAULT.0
                            } else {
                                code
                            },
                            Ordering::SeqCst,
                        );
                    }
                    break;
                }
            }
        }
        // A successful final write remains submitted even if cancellation
        // raced with its completion or the owner has started settling.
        if written == stdin_bytes.len() {
            submitted.store(true, Ordering::SeqCst);
        }
        // Drop closes the write end so a blocked reader can finish.
    });
    (handle, io)
}

#[derive(Clone, Copy, PartialEq)]
enum Failure {
    None,
    Io,
    Output,
    Cancelled,
    Timeout,
    Drain,
}

/// Every failure surfaces the same public error code and message as the C++;
/// an already-submitted activation degrades to `APPX_ACTIVATION_OUTCOME_UNKNOWN`.
fn verdict_error(failure: Failure, activation_submission: bool, submitted: bool) -> Option<Error> {
    let code_message: Option<(&str, &str)> = match failure {
        Failure::None => None,
        Failure::Io => Some((
            "HELPER_IO_FAILED",
            "Cannot observe the owned helper completion",
        )),
        Failure::Output => Some((
            "HELPER_OUTPUT_LIMIT",
            "Owned helper exceeded its output budget",
        )),
        Failure::Cancelled => Some(("LAUNCH_CANCELLED", "Operation cancelled")),
        Failure::Timeout => Some(("HELPER_TIMEOUT", "Owned helper exceeded its time budget")),
        Failure::Drain => Some((
            "HELPER_DRAIN_TIMEOUT",
            "Owned helper output did not close within its drain budget",
        )),
    };
    let error = code_message.map(|(code, message)| Error::new(code, message))?;
    if activation_submission && submitted {
        return Some(Error::new(
            "APPX_ACTIVATION_OUTCOME_UNKNOWN",
            format!(
                "Activation request was submitted; refresh Desktop state before retrying. {}",
                error.message
            ),
        ));
    }
    Some(error)
}

pub struct ProcessRequest<'a> {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub stdin_bytes: &'a [u8],
    pub environment: &'a mut crate::environment::UnicodeEnvironment,
    pub timeout: Duration,
    pub stdout_limit: usize,
    pub stderr_limit: usize,
    pub create_no_window: bool,
    pub activation_submission: bool,
}

struct Cleanup {
    failed: bool,
    stage: &'static str,
    error: u32,
}

pub fn run_owned_process(
    request: ProcessRequest<'_>,
    cancel: &Cancellation,
) -> Result<ProcessResult> {
    let started = Instant::now();
    cancel.check()?;
    if request.timeout.is_zero()
        || request.stdout_limit == 0
        || request.stderr_limit == 0
        || request.stdin_bytes.len() > HELPER_REQUEST_LIMIT
    {
        return fail("HELPER_BUDGET_INVALID", "Invalid helper input or budget");
    }
    let deadline = started + request.timeout;

    let stdin_pipe = create_pipe()?;
    let stdout_pipe = create_pipe()?;
    let stderr_pipe = create_pipe()?;
    let child_stdin = inheritable_duplicate(stdin_pipe.read.raw())?;
    let child_stdout = inheritable_duplicate(stdout_pipe.write.raw())?;
    let child_stderr = inheritable_duplicate(stderr_pipe.write.raw())?;

    let program_wide = wide(&request.program);
    let mut command_line: Vec<u16> = Vec::new();
    append_quoted(&mut command_line, &program_wide);
    for argument in &request.arguments {
        command_line.push(' ' as u16);
        append_quoted(
            &mut command_line,
            &argument.encode_utf16().collect::<Vec<u16>>(),
        );
    }
    command_line.push(0);
    let program_wide = {
        let mut terminated = program_wide;
        terminated.push(0);
        terminated
    };
    let environment_block: Vec<u16> = request.environment.block().to_vec();

    let mut attribute_size = 0usize;
    // SAFETY: sizing call with no list writes the required size.
    unsafe {
        let _ = InitializeProcThreadAttributeList(None, 1, Some(0), &mut attribute_size);
    }
    if attribute_size == 0 {
        return fail(
            "HELPER_START_FAILED",
            "Cannot size the helper startup attributes",
        );
    }
    let mut attribute_storage = vec![0u8; attribute_size];
    let attribute_list = windows::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST(
        attribute_storage.as_mut_ptr().cast(),
    );
    // SAFETY: the storage is exactly the sized buffer.
    if unsafe {
        InitializeProcThreadAttributeList(Some(attribute_list), 1, Some(0), &mut attribute_size)
    }
    .is_err()
    {
        return fail(
            "HELPER_START_FAILED",
            "Cannot prepare the helper startup attributes",
        );
    }
    struct AttributeScope(windows::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for AttributeScope {
        fn drop(&mut self) {
            // SAFETY: paired with the successful initialize above.
            unsafe { DeleteProcThreadAttributeList(self.0) };
        }
    }
    let attributes = AttributeScope(attribute_list);
    let mut inherited_handles = [child_stdin.raw(), child_stdout.raw(), child_stderr.raw()];
    // SAFETY: exactly three HANDLE entries; the attribute list is initialized.
    if unsafe {
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(inherited_handles.as_mut_ptr().cast()),
            std::mem::size_of_val(&inherited_handles),
            None,
            None,
        )
    }
    .is_err()
    {
        return fail(
            "HELPER_START_FAILED",
            "Cannot restrict helper handle inheritance",
        );
    }

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = child_stdin.raw();
    startup.StartupInfo.hStdOutput = child_stdout.raw();
    startup.StartupInfo.hStdError = child_stderr.raw();
    startup.lpAttributeList = attribute_list;
    let mut flags = CREATE_UNICODE_ENVIRONMENT.0 | EXTENDED_STARTUPINFO_PRESENT.0;
    if request.create_no_window {
        flags |= CREATE_NO_WINDOW.0;
    }
    #[allow(clippy::erasing_op, clippy::identity_op)]
    let flags = flags;

    cancel.check()?;
    if Instant::now() >= deadline {
        return fail("HELPER_TIMEOUT", "Owned helper exceeded its setup budget");
    }
    let mut information = PROCESS_INFORMATION::default();
    // SAFETY: application/command/environment pointers are null-terminated;
    // the returned process/thread handles are owned below.
    let created = unsafe {
        CreateProcessW(
            PCWSTR(program_wide.as_ptr()),
            Some(windows::core::PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            true,
            PROCESS_CREATION_FLAGS(flags),
            Some(environment_block.as_ptr().cast()),
            PCWSTR::null(),
            &startup.StartupInfo,
            &mut information,
        )
    };
    if created.is_err() {
        return fail("HELPER_START_FAILED", "Cannot start owned helper");
    }
    drop(attributes);
    let process = Arc::new(
        OwnedHandle::new(information.hProcess)
            .ok_or_else(|| Error::new("HELPER_START_FAILED", "Cannot start owned helper"))?,
    );
    let _primary_thread = OwnedHandle::new(information.hThread); // observation only; closed on drop
    drop(child_stdin);
    drop(child_stdout);
    drop(child_stderr);
    drop(stdin_pipe.read);
    drop(stdout_pipe.write);
    drop(stderr_pipe.write);

    let stopping = Arc::new(AtomicBool::new(false));
    let submitted = Arc::new(AtomicBool::new(false));
    let write_error = Arc::new(AtomicU32::new(0));
    let (stdout_thread, stdout_state, stdout_cancel) = spawn_reader(
        stdout_pipe.read,
        request.stdout_limit,
        Arc::clone(&stopping),
    );
    let (stderr_thread, stderr_state, stderr_cancel) = spawn_reader(
        stderr_pipe.read,
        request.stderr_limit,
        Arc::clone(&stopping),
    );
    // Actual submission boundary, after reader setup: a late cancellation
    // still settles the already-created child before reporting.
    if cancel.check().is_err() || Instant::now() >= deadline {
        stopping.store(true, Ordering::SeqCst);
        // The child may already have spawned descendants holding the pipe
        // write ends; cancel parked reader I/O before the unbounded joins.
        stdout_cancel.cancel_io();
        stderr_cancel.cancel_io();
        // SAFETY: terminating only the child this runner created above.
        let _ = unsafe { TerminateProcess(process.raw(), 1) };
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        return fail("LAUNCH_CANCELLED", "Operation cancelled");
    }
    let (writer_thread, writer_cancel) = if !request.stdin_bytes.is_empty() {
        let (thread, io) = spawn_writer(
            stdin_pipe.write,
            request.stdin_bytes.to_vec(),
            Arc::clone(&stopping),
            cancel.clone(),
            Arc::clone(&submitted),
            Arc::clone(&write_error),
        );
        (Some(thread), Some(io))
    } else {
        drop(stdin_pipe.write);
        (None, None)
    };

    struct Owned {
        process: Arc<OwnedHandle>,
        stopping: Arc<AtomicBool>,
        stdout_thread: Option<std::thread::JoinHandle<()>>,
        stderr_thread: Option<std::thread::JoinHandle<()>>,
        writer_thread: Option<std::thread::JoinHandle<()>>,
        stdout_cancel: Arc<IoThreadHandle>,
        stderr_cancel: Arc<IoThreadHandle>,
        writer_cancel: Option<Arc<IoThreadHandle>>,
        settled: bool,
        termination_requested: bool,
        cleanup: Cleanup,
    }
    impl Owned {
        fn cancel_all_io(&self) {
            self.stdout_cancel.cancel_io();
            self.stderr_cancel.cancel_io();
            if let Some(writer) = self.writer_cancel.as_ref() {
                writer.cancel_io();
            }
        }
        fn settle(&mut self, mut abort: bool) {
            if self.settled {
                return;
            }
            if abort {
                self.stopping.store(true, Ordering::SeqCst);
                // Abort any pipe I/O the threads are already parked in;
                // terminating the owned child alone cannot break a pipe a
                // grandchild still holds.
                self.cancel_all_io();
            }
            let cleanup_started = Instant::now();
            let mut cancel_retry_at = Instant::now() + CANCEL_RETRY_INTERVAL;
            let mut terminate_attempted = false;
            loop {
                // SAFETY: owned, valid process handle; zero timeout polls.
                let process_wait = unsafe { WaitForSingleObject(self.process.raw(), 0) };
                let mut complete = process_wait == WAIT_OBJECT_0;
                if process_wait != WAIT_OBJECT_0 && process_wait != WAIT_TIMEOUT {
                    self.cleanup.failed = true;
                    self.cleanup.stage = "process_wait";
                    self.cleanup.error = unsafe { windows::Win32::Foundation::GetLastError() }.0;
                }
                if abort && !complete && !terminate_attempted {
                    terminate_attempted = true;
                    self.termination_requested = true;
                    // SAFETY: terminates only the process this runner created
                    // and still owns — never a shared or external process.
                    let terminated = unsafe { TerminateProcess(self.process.raw(), 1) };
                    if terminated.is_err() {
                        // The child may already be exiting; completion below,
                        // not this racing request, decides whether it settled.
                        self.cleanup.stage = "terminate";
                        self.cleanup.error =
                            unsafe { windows::Win32::Foundation::GetLastError() }.0;
                    }
                }
                for thread in [
                    &mut self.stdout_thread,
                    &mut self.stderr_thread,
                    &mut self.writer_thread,
                ] {
                    match thread {
                        None => continue,
                        Some(handle) if handle.is_finished() => continue,
                        Some(_) => complete = false,
                    }
                }
                if Instant::now() - cleanup_started >= SETTLE_BUDGET {
                    self.cleanup.failed = true;
                    self.cleanup.stage = "settle_budget";
                    self.stopping.store(true, Ordering::SeqCst);
                    // Keep re-issuing the OS-level I/O cancellation: a
                    // registration race (cancel before the thread registered
                    // its handle) or a latched cancel consumed by an earlier
                    // read must not turn into an unbounded wait.
                    self.cancel_all_io();
                    cancel_retry_at = Instant::now() + CANCEL_RETRY_INTERVAL;
                    abort = true;
                    // A kernel/driver that refuses the cancellation cannot be
                    // made hard-real-time: retain ownership until it settles;
                    // never detach or manufacture an idle/success result.
                }
                if !complete && abort && Instant::now() >= cancel_retry_at {
                    self.cancel_all_io();
                    cancel_retry_at = Instant::now() + CANCEL_RETRY_INTERVAL;
                }
                if complete {
                    break;
                }
                std::thread::sleep(WAIT_SLICE);
            }
            for thread in [
                self.stdout_thread.take(),
                self.stderr_thread.take(),
                self.writer_thread.take(),
            ]
            .into_iter()
            .flatten()
            {
                let _ = thread.join();
            }
            self.settled = true;
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            self.settle(true);
        }
    }
    let mut owned = Owned {
        process: Arc::clone(&process),
        stopping: Arc::clone(&stopping),
        stdout_thread: Some(stdout_thread),
        stderr_thread: Some(stderr_thread),
        writer_thread,
        stdout_cancel,
        stderr_cancel,
        writer_cancel,
        settled: false,
        termination_requested: false,
        cleanup: Cleanup {
            failed: false,
            stage: "none",
            error: 0,
        },
    };

    let mut failure = Failure::None;
    let mut exit_code: u32 = 0;
    let mut process_exited = false;
    let mut drain_deadline = deadline;
    loop {
        if !process_exited {
            // SAFETY: owned handle; zero timeout polls.
            let wait = unsafe { WaitForSingleObject(owned.process.raw(), 0) };
            if wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT {
                failure = Failure::Io;
                break;
            }
            if wait == WAIT_OBJECT_0 {
                process_exited = true;
                // SAFETY: owned handle.
                if unsafe { GetExitCodeProcess(owned.process.raw(), &mut exit_code) }.is_err() {
                    failure = Failure::Io;
                    break;
                }
                drain_deadline = Instant::now() + DRAIN_BUDGET;
            }
        }
        if stdout_state.over_limit.load(Ordering::SeqCst)
            || stderr_state.over_limit.load(Ordering::SeqCst)
        {
            failure = Failure::Output;
            break;
        }
        if stdout_state.error.load(Ordering::SeqCst) != 0
            || stderr_state.error.load(Ordering::SeqCst) != 0
            || write_error.load(Ordering::SeqCst) != 0
        {
            failure = Failure::Io;
            break;
        }
        let readers_done = stdout_state.done.load(Ordering::SeqCst)
            && stderr_state.done.load(Ordering::SeqCst)
            && owned.writer_thread.as_ref().is_none_or(|t| t.is_finished());
        if process_exited && readers_done {
            break;
        }
        if cancel.cancelled() {
            failure = Failure::Cancelled;
            break;
        }
        if Instant::now() >= deadline {
            failure = Failure::Timeout;
            break;
        }
        if process_exited && Instant::now() >= drain_deadline {
            failure = Failure::Drain;
            break;
        }
        if let Some(event) = cancel.event_handle() {
            // SAFETY: owned event handle; short slice keeps the loop live.
            unsafe { WaitForSingleObject(event.raw(), WAIT_SLICE.as_millis() as u32) };
        } else {
            std::thread::sleep(WAIT_SLICE);
        }
    }

    owned.settle(!matches!(failure, Failure::None));
    let termination_requested = owned.termination_requested;
    let cleanup_failed = owned.cleanup.failed;
    let cleanup_stage = owned.cleanup.stage;
    let cleanup_error = owned.cleanup.error;
    drop(owned);

    let mut result = ProcessResult {
        submitted: submitted.load(Ordering::SeqCst),
        ..ProcessResult::default()
    };
    // The authoritative exit code is re-read after everything settles, like
    // the C++; the shared handle is still open.
    // SAFETY: owned, valid, settled process handle.
    if unsafe { GetExitCodeProcess(process.raw(), &mut exit_code) }.is_err() {
        failure = Failure::Io;
    }
    result.normal_exit = exit_code < 0x8000_0000;
    result.exit_code = if result.normal_exit {
        exit_code as i32
    } else {
        -1
    };
    let _ = process_exited;

    // Late failure refinement: a fully-collected natural completion stays
    // known even when cancellation or timeout raced with it.
    if (failure == Failure::Cancelled || failure == Failure::Timeout)
        && !termination_requested
        && stdout_state.eof.load(Ordering::SeqCst)
        && stderr_state.eof.load(Ordering::SeqCst)
        && (request.stdin_bytes.is_empty() || result.submitted)
    {
        failure = Failure::None;
    }
    if failure == Failure::None && !request.stdin_bytes.is_empty() && !result.submitted {
        failure = if cancel.cancelled() {
            Failure::Cancelled
        } else {
            Failure::Io
        };
    }
    if cleanup_failed {
        let message = format!(
            "Owned helper cleanup failed: stage={cleanup_stage}; native_error={cleanup_error}"
        );
        if request.activation_submission && result.submitted {
            return Err(Error::new(
                "APPX_ACTIVATION_OUTCOME_UNKNOWN",
                format!(
                    "Activation request was submitted; refresh Desktop state before retrying. {message}"
                ),
            ));
        }
        return Err(Error::new("HELPER_CLEANUP_FAILED", message));
    }
    if let Some(error) = verdict_error(failure, request.activation_submission, result.submitted) {
        return Err(error);
    }
    result.out = stdout_state.bytes.lock().expect("reader lock").clone();
    result.err = stderr_state.bytes.lock().expect("reader lock").clone();
    Ok(result)
}

/// Convenience wrapper mirroring the C++ `runHelper`.
#[derive(Debug)]
pub struct HelperResult {
    pub out: Vec<u8>,
    pub err: Vec<u8>,
    pub exit_code: i32,
    pub submitted: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run_helper(
    program: &Path,
    arguments: &[String],
    input: &[u8],
    cancel: &Cancellation,
    timeout: Duration,
    output_limit: usize,
    environment: &mut crate::environment::UnicodeEnvironment,
    activation_submission: bool,
) -> Result<HelperResult> {
    let request = ProcessRequest {
        program: program.to_path_buf(),
        arguments: arguments.to_vec(),
        stdin_bytes: input,
        environment,
        timeout,
        stdout_limit: output_limit,
        stderr_limit: output_limit,
        create_no_window: true,
        activation_submission,
    };
    let result = run_owned_process(request, cancel)?;
    Ok(HelperResult {
        out: result.out,
        err: result.err,
        exit_code: result.exit_code,
        submitted: result.submitted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system32(name: &str) -> PathBuf {
        PathBuf::from(std::env::var("SYSTEMROOT").expect("SYSTEMROOT set"))
            .join("System32")
            .join(name)
    }

    /// `cmd /d /c waitfor /t 25 <signal>` keeps a silent grandchild alive for
    /// 25 s that inherits the direct child's stdout (this runner's pipe write
    /// end). Terminating the owned `cmd` cannot break the pipe, so reader
    /// threads park in `ReadFile` until the I/O is actually cancelled - the
    /// audit F02 stuck-pipe shape. The whole command is one argument because
    /// the runner quotes every argument (cmd only strips the outer quotes of
    /// a single fully-quoted /c string). The signal name is unique per
    /// invocation: waitfor signals are system-global and survive this
    /// runner's cancellation for the grandchild's full lifetime, so reusing a
    /// name collides with the previous test run's leftover waiter.
    fn stuck_pipe_arguments(kind: &str) -> Vec<String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        vec![format!(
            "/d /c waitfor /t 25 CpgStuck{kind}{}{unique}",
            std::process::id()
        )]
    }

    #[test]
    fn stuck_pipe_descendant_cleanup_is_bounded() {
        assert!(
            system32("waitfor.exe").is_file(),
            "waitfor.exe fixture missing"
        );
        let cancel = Cancellation::new();
        let mut environment = crate::environment::UnicodeEnvironment::current().unwrap();
        let started = Instant::now();
        let error = run_helper(
            &system32("cmd.exe"),
            &stuck_pipe_arguments("Timeout"),
            b"",
            &cancel,
            Duration::from_secs(5),
            64 * 1024,
            &mut environment,
            false,
        )
        .unwrap_err();
        let elapsed = started.elapsed();
        assert_eq!(error.code, "HELPER_TIMEOUT");
        assert!(
            elapsed < Duration::from_secs(15),
            "cleanup waited {elapsed:?} for a silent pipe-holding descendant"
        );
    }

    #[test]
    fn cancel_during_stuck_pipe_cleanup_is_bounded() {
        assert!(
            system32("waitfor.exe").is_file(),
            "waitfor.exe fixture missing"
        );
        let cancel = Cancellation::new();
        let mut environment = crate::environment::UnicodeEnvironment::current().unwrap();
        let canceller = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                cancel.cancel();
            })
        };
        let started = Instant::now();
        let error = run_helper(
            &system32("cmd.exe"),
            &stuck_pipe_arguments("Cancel"),
            b"",
            &cancel,
            Duration::from_secs(30),
            64 * 1024,
            &mut environment,
            false,
        )
        .unwrap_err();
        let elapsed = started.elapsed();
        canceller.join().expect("canceller");
        assert_eq!(error.code, "LAUNCH_CANCELLED");
        assert!(
            elapsed < Duration::from_secs(15),
            "cancelled cleanup waited {elapsed:?} for a silent pipe-holding descendant"
        );
    }

    #[test]
    fn command_quoting_matches_windows_rules() {
        let mut command = Vec::new();
        append_quoted(
            &mut command,
            &"C:\\Program Files\\x.exe"
                .encode_utf16()
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            String::from_utf16_lossy(&command),
            "\"C:\\Program Files\\x.exe\""
        );
        let mut command = Vec::new();
        append_quoted(&mut command, &"a\"b\\c".encode_utf16().collect::<Vec<_>>());
        // Backslashes double only before a quote or at the very end.
        assert_eq!(String::from_utf16_lossy(&command), "\"a\\\"b\\c\"");
        let mut command = Vec::new();
        append_quoted(
            &mut command,
            &"trailing\\".encode_utf16().collect::<Vec<_>>(),
        );
        assert_eq!(String::from_utf16_lossy(&command), "\"trailing\\\\\"");
    }

    #[test]
    fn collects_stdout_and_stderr_of_native_helpers() {
        let cancel = Cancellation::new();
        let mut environment = crate::environment::UnicodeEnvironment::current().unwrap();
        // whoami: exit 0 with stdout. ping with a bad flag: usage on stderr.
        let result = run_helper(
            &system32("whoami.exe"),
            &[],
            b"",
            &cancel,
            Duration::from_secs(10),
            64 * 1024,
            &mut environment,
            false,
        )
        .expect("helper completes");
        assert_eq!(result.exit_code, 0);
        assert!(!result.out.is_empty());
        assert!(result.err.is_empty());

        let result = run_helper(
            &system32("find.exe"),
            &[],
            b"",
            &cancel,
            Duration::from_secs(10),
            64 * 1024,
            &mut environment,
            false,
        )
        .expect("helper completes");
        assert_ne!(result.exit_code, 0);
        assert!(
            !result.err.is_empty(),
            "find with no arguments reports usage on stderr"
        );
    }

    #[test]
    fn timeout_terminates_only_the_owned_helper() {
        let cancel = Cancellation::new();
        let mut environment = crate::environment::UnicodeEnvironment::current().unwrap();
        let error = run_helper(
            &system32("ping.exe"),
            &["-n".into(), "30".into(), "127.0.0.1".into()],
            b"",
            &cancel,
            Duration::from_millis(300),
            64 * 1024,
            &mut environment,
            false,
        )
        .unwrap_err();
        assert_eq!(error.code, "HELPER_TIMEOUT");
    }

    #[test]
    fn output_limit_is_enforced() {
        let cancel = Cancellation::new();
        let mut environment = crate::environment::UnicodeEnvironment::current().unwrap();
        let error = run_helper(
            &system32("ping.exe"),
            &["-n".into(), "1".into(), "127.0.0.1".into()],
            b"",
            &cancel,
            Duration::from_secs(10),
            4,
            &mut environment,
            false,
        )
        .unwrap_err();
        assert_eq!(error.code, "HELPER_OUTPUT_LIMIT");
    }

    #[test]
    fn cancellation_fails_fast_with_cancelled_code() {
        let cancel = Cancellation::new();
        let mut environment = crate::environment::UnicodeEnvironment::current().unwrap();
        let cancel_clone = cancel.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            cancel_clone.cancel();
        });
        let error = run_helper(
            &system32("ping.exe"),
            &["-n".into(), "30".into(), "127.0.0.1".into()],
            b"",
            &cancel,
            Duration::from_secs(10),
            64 * 1024,
            &mut environment,
            false,
        )
        .unwrap_err();
        canceller.join().expect("canceller");
        assert_eq!(error.code, "LAUNCH_CANCELLED");
    }

    #[test]
    fn stdin_round_trip() {
        let cancel = Cancellation::new();
        let mut environment = crate::environment::UnicodeEnvironment::current().unwrap();
        let result = run_helper(
            &system32("find.exe"),
            &["xyz".into()],
            b"line-with-xyz\n",
            &cancel,
            Duration::from_secs(10),
            64 * 1024,
            &mut environment,
            false,
        )
        .expect("helper completes");
        assert_eq!(result.exit_code, 0, "find reports a match");
        assert!(String::from_utf8_lossy(&result.out).contains("line-with-xyz"));
    }
}
