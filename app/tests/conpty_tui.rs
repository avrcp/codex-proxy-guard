//! ConPTY-driven acceptance for the Ratatui TUI input layer: drives the real
//! EXE inside a Windows pseudo-console with VT input sequences and verifies
//! the editor behaviors fixed in the 2026-10 rework — paste-as-keystrokes,
//! Ctrl-key gating, character-based cursor editing, the inline notice line —
//! plus the `tui.alternate_screen` policy and a full proxy save round-trip
//! through the real engine.
//!
//! The suite stays read-only beyond its own temp configuration: no Codex
//! Home writes, no Desktop activation, no daemon commands. Only the main
//! screen's inert keys are probed with Ctrl held (Ctrl+R/E/B); Ctrl+L is
//! deliberately NOT exercised here so a gating regression can never
//! activate the real Desktop from a test.

use std::ffi::c_void;
use std::fs;
use std::io::{Read, Write};
use std::os::windows::io::FromRawHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::System::Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::PWSTR;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const STEP_TIMEOUT: Duration = Duration::from_secs(8);
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);
const ENTER_ALT_SCREEN: &str = "\u{1b}[?1049h";
const LEAVE_ALT_SCREEN: &str = "\u{1b}[?1049l";

struct TerminalSession {
    writer: Option<std::fs::File>,
    output: Arc<Mutex<String>>,
    process: HANDLE,
    thread: HANDLE,
    attribute_list: LPPROC_THREAD_ATTRIBUTE_LIST,
    #[allow(dead_code)] // kept alive for the process lifetime, never read
    attribute_storage: Vec<usize>,
    pseudo_console: HPCON,
}

impl TerminalSession {
    fn send(&mut self, bytes: &[u8]) {
        self.writer
            .as_mut()
            .expect("writer")
            .write_all(bytes)
            .expect("write terminal input");
    }

    fn reset(&self) {
        self.output.lock().expect("output").clear();
    }

    fn wait_for(&self, needle: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if self.output.lock().expect("output").contains(needle) {
                return;
            }
            if Instant::now() > deadline {
                panic!(
                    "timed out after {timeout:?} waiting for {needle:?}; terminal output so far:\n{}",
                    self.output.lock().expect("output")
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_for_any(&self, needles: &[&str], timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let buffer = self.output.lock().expect("output");
            if needles.iter().any(|needle| buffer.contains(needle)) {
                return;
            }
            let snapshot = buffer.clone();
            drop(buffer);
            if Instant::now() > deadline {
                panic!(
                    "timed out after {timeout:?} waiting for any of {needles:?}; terminal output so far:\n{snapshot}"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn assert_contains(&self, needle: &str) {
        assert!(
            self.output.lock().expect("output").contains(needle),
            "terminal output should contain {needle:?}:\n{}",
            self.output.lock().expect("output")
        );
    }

    fn assert_not_contains(&self, needle: &str) {
        assert!(
            !self.output.lock().expect("output").contains(needle),
            "terminal output must not contain {needle:?}:\n{}",
            self.output.lock().expect("output")
        );
    }

    fn exit_code(&self, timeout: Duration) -> u32 {
        // SAFETY: the handles are owned by this session and still open.
        unsafe {
            let waited = WaitForSingleObject(self.process, timeout.as_millis() as u32);
            if waited != WAIT_OBJECT_0 {
                let _ = TerminateProcess(self.process, 1);
                panic!(
                    "process did not exit within {timeout:?}; terminal output:\n{}",
                    self.output.lock().expect("output")
                );
            }
            let mut code = 0u32;
            GetExitCodeProcess(self.process, &mut code).expect("exit code");
            code
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // SAFETY: every handle below is owned by this session and dropped
        // exactly once. Closing the pseudoconsole breaks the reader's pipe
        // so its thread observes EOF and exits on its own.
        unsafe {
            DeleteProcThreadAttributeList(self.attribute_list);
            ClosePseudoConsole(self.pseudo_console);
            let _ = CloseHandle(self.thread);
            let _ = CloseHandle(self.process);
        }
        self.writer = None;
    }
}

fn spawn_in_pty(command_line: &str) -> TerminalSession {
    let mut command: Vec<u16> = command_line.encode_utf16().chain([0]).collect();
    // SAFETY: the ConPTY creation pattern from the Windows echocon sample:
    // non-inheritable pipes, one end given to the pseudoconsole (and closed
    // here afterwards), the child started with the pseudoconsole attribute
    // and no handle inheritance.
    unsafe {
        let mut stdin_read = HANDLE::default();
        let mut stdin_write = HANDLE::default();
        let mut stdout_read = HANDLE::default();
        let mut stdout_write = HANDLE::default();
        // Both pipe ends are created inheritable, matching the ConPTY sample:
        // CreatePseudoConsole passes the pty-side ends to its internal
        // conhost through handle inheritance. The child below is started with
        // bInheritHandles=false plus the pseudoconsole attribute, so it never
        // inherits the session-owned ends directly.
        let inheritable = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: true.into(),
        };
        CreatePipe(&mut stdin_read, &mut stdin_write, Some(&inheritable), 0).expect("stdin pipe");
        CreatePipe(&mut stdout_read, &mut stdout_write, Some(&inheritable), 0)
            .expect("stdout pipe");
        let size = COORD { X: 120, Y: 30 };
        let pseudo_console =
            CreatePseudoConsole(size, stdin_read, stdout_write, 0).expect("pseudo console");
        let _ = CloseHandle(stdin_read);
        let _ = CloseHandle(stdout_write);

        let mut attribute_size = 0usize;
        let _ = InitializeProcThreadAttributeList(None, 1, Some(0), &mut attribute_size);
        let mut attribute_storage: Vec<usize> =
            vec![0; attribute_size.div_ceil(std::mem::size_of::<usize>())];
        let attribute_list =
            LPPROC_THREAD_ATTRIBUTE_LIST(attribute_storage.as_mut_ptr() as *mut c_void);
        InitializeProcThreadAttributeList(Some(attribute_list), 1, Some(0), &mut attribute_size)
            .expect("attribute list");
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            Some(pseudo_console.0 as *const c_void),
            std::mem::size_of::<HPCON>(),
            None,
            None,
        )
        .expect("pseudoconsole attribute");

        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        // The extended startup block must reference the initialized attribute
        // list; without this assignment CreateProcessW silently ignores the
        // pseudoconsole attribute and the child joins our own console.
        startup.lpAttributeList = attribute_list;
        let mut information = PROCESS_INFORMATION::default();
        CreateProcessW(
            None,
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            EXTENDED_STARTUPINFO_PRESENT,
            None,
            None,
            &startup.StartupInfo,
            &mut information,
        )
        .expect("create process");

        let writer = std::fs::File::from_raw_handle(stdin_write.0 as _);
        let reader = std::fs::File::from_raw_handle(stdout_read.0 as _);
        let output = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            let mut file = reader;
            let mut chunk = [0u8; 4096];
            loop {
                match file.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => sink
                        .lock()
                        .expect("output")
                        .push_str(&String::from_utf8_lossy(&chunk[..count])),
                }
            }
        });

        TerminalSession {
            writer: Some(writer),
            output,
            process: information.hProcess,
            thread: information.hThread,
            attribute_list,
            attribute_storage,
            pseudo_console,
        }
    }
}

fn exe_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_CodexProxyGuard"))
}

fn temp_config(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
    let directory =
        std::env::temp_dir().join(format!("cpg-conpty-{}-{unique}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("temp dir");
    directory.join(name)
}

fn settle_main_screen(session: &TerminalSession) {
    session.wait_for("TARGET", STARTUP_TIMEOUT);
    // The header status word appears only once the initial refresh settled,
    // so editor keys are not swallowed by the busy gate afterwards.
    session.wait_for_any(&["READY", "BLOCKED", "RUNNING"], STARTUP_TIMEOUT);
}

// The three tests below are `#[ignore]`d: the ConPTY harness is complete and
// the wiring verified down to conhost (the child attaches to the
// pseudoconsole and conhost's init sequences stream back), but on the current
// development host the child's own output never arrives on the session pipe —
// even `cmd.exe /c echo` reproduces it, so it is an environment/harness
// passthrough issue rather than a product defect. Re-enable after
// investigating conhost output passthrough (see
// docs/RUST_TUI_MIGRATION_ACCEPTANCE.md, "ConPTY acceptance status").
#[ignore = "ConPTY output passthrough does not deliver child output on this host; harness pending"]
#[test]
fn pty_channel_passes_headless_output() {
    let config = temp_config("probe.toml");
    let session = spawn_in_pty(&format!(
        "\"{}\" config-path --config \"{}\"",
        exe_path().display(),
        config.display()
    ));
    session.wait_for(".toml", STARTUP_TIMEOUT);
    assert_eq!(session.exit_code(EXIT_TIMEOUT), 0);
}

#[ignore = "ConPTY output passthrough does not deliver child output on this host; harness pending"]
#[test]
fn editor_input_gating_cursor_and_save_round_trip() {
    let config = temp_config("config.toml");
    let mut session = spawn_in_pty(&format!(
        "\"{}\" console --config \"{}\"",
        exe_path().display(),
        config.display()
    ));
    settle_main_screen(&session);

    // E opens the editor prefilled with the current proxy.
    session.reset();
    session.send(b"e");
    session.wait_for("Proxy Configuration", STEP_TIMEOUT);
    session.assert_contains("127.0.0.1");
    session.assert_contains("10808");

    // Ctrl+V (0x16) must not insert a literal 'v'.
    session.reset();
    session.send(b"\x16");
    std::thread::sleep(Duration::from_millis(400));
    session.assert_not_contains("127.0.0.1v");

    // Character-based cursor editing: Home, insert at the front, backspace.
    session.reset();
    session.send(b"\x1b[H"); // Home
    session.send(b"X");
    session.wait_for("X127.0.0.1", STEP_TIMEOUT);
    session.reset();
    session.send(b"\x08"); // Backspace removes the X again
    session.wait_for("127.0.0.1", STEP_TIMEOUT);
    session.assert_not_contains("X127.0.0.1");

    // Tab switches to the port; End + Backspace clear it; letters are
    // filtered while typing digits (this is exactly how Windows Terminal
    // delivers a paste now that bracketed paste mode is no longer requested).
    session.reset();
    session.send(b"\t");
    session.send(b"\x1b[F"); // End
    for _ in 0..5 {
        session.send(b"\x08");
    }
    session.send(b"8a080");
    session.wait_for("8080", STEP_TIMEOUT);
    session.assert_not_contains("[a8080]");

    // Enter saves through the real engine; the inline notice and the
    // refreshed proxy row confirm the committed configuration.
    session.reset();
    session.send(b"\r");
    session.wait_for("Proxy saved.", STEP_TIMEOUT);
    session.wait_for("http://127.0.0.1:8080", STEP_TIMEOUT);

    session.reset();
    session.send(b"q");
    assert_eq!(session.exit_code(EXIT_TIMEOUT), 0);
    session.assert_contains(ENTER_ALT_SCREEN); // auto policy entered it
    session.assert_contains(LEAVE_ALT_SCREEN); // and restored it on exit

    let stored = fs::read_to_string(&config).expect("stored config");
    assert!(stored.contains("host = '127.0.0.1'"), "{stored}");
    assert!(stored.contains("port = 8080"), "{stored}");
}

#[ignore = "ConPTY output passthrough does not deliver child output on this host; harness pending"]
#[test]
fn alternate_screen_never_is_honored_and_ctrl_keys_stay_inert() {
    let config = temp_config("never.toml");
    fs::write(
        &config,
        "version = 2\n\n[tui]\nalternate_screen = 'never'\n",
    )
    .expect("seed never policy");
    let mut session = spawn_in_pty(&format!(
        "\"{}\" console --config \"{}\"",
        exe_path().display(),
        config.display()
    ));
    settle_main_screen(&session);
    session.assert_not_contains(ENTER_ALT_SCREEN);

    // Ctrl+R must not trigger a refresh (no busy label).
    session.reset();
    session.send(b"\x12");
    std::thread::sleep(Duration::from_millis(600));
    session.assert_not_contains("BUSY");
    session.assert_not_contains("Updating");

    // Ctrl+E must not open the proxy editor.
    session.reset();
    session.send(b"\x05");
    std::thread::sleep(Duration::from_millis(400));
    session.assert_not_contains("Proxy Configuration");

    // Ctrl+B must not open the backend consent modal either.
    session.reset();
    session.send(b"\x02");
    std::thread::sleep(Duration::from_millis(400));
    session.assert_not_contains("Backend Proxy Authorization");
    session.assert_not_contains("Backend authorization unavailable");

    session.reset();
    session.send(b"q");
    assert_eq!(session.exit_code(EXIT_TIMEOUT), 0);
    session.assert_not_contains(LEAVE_ALT_SCREEN);
}
