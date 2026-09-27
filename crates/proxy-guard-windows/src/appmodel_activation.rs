//! Native Windows application-model activation backend.
//!
//! A registered Desktop application is activated through the OS-provided
//! `IApplicationActivationManager::ActivateApplication` with `AO_NONE`. This
//! is the normal registered-entry launch path — not a debugging context, not
//! an elevated helper, and never a naked EXE spawn inside WindowsApps.
//!
//! The COM call runs in a short-lived Guard-owned worker process (the same
//! executable, hidden `internal-activate-package` subcommand) so an
//! uncancellable activation can never wedge the TUI, and so apartment-bound
//! COM state is created and released on one clearly owned thread. The worker
//! itself needs no package identity and never runs in the OpenAI package
//! context. Communication is one bounded stdin request and one bounded stdout
//! receipt — no named pipes, no nonces, no listening sockets.
//!
//! Cancellation is reported honestly: cancelling before the request is
//! submitted guarantees no activation was requested; after submission the
//! outcome is unknown, because terminating Guard's own worker cannot revoke a
//! request already delivered to Windows. No code path ever falls back to a
//! bare EXE, a debug context, or elevation.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
/// Bounded single request from the parent to the worker.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;
/// Bounded single receipt from the worker to the parent.
pub const MAX_RECEIPT_BYTES: usize = 64 * 1024;
/// Total activation budget including target observation.
pub const ACTIVATION_BUDGET: Duration = Duration::from_secs(30);
/// Budget for terminating and reaping Guard's own worker child.
pub const WORKER_CLEANUP_BUDGET: Duration = Duration::from_secs(3);
/// Bounded window for observing whether a returned target exits immediately.
const EARLY_EXIT_WINDOW_MS: u32 = 150;
/// Slack when comparing the target creation time against the moment the
/// activation request was submitted, covering clock read granularity.
const CREATION_CLOCK_SLACK_MS: u64 = 250;

/// The one bounded request the parent may send. Every field must originate
/// from this launch's discovery result; the worker refuses anything else.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivationWorkerRequest {
    pub version: u32,
    /// Dynamically resolved application user model ID
    /// (`PackageFamilyName!ApplicationId`).
    pub aumid: String,
    /// Package full name the target must still have after activation.
    pub expected_package_full_name: String,
    /// Canonicalized registered executable, used only to verify the target.
    pub expected_executable: PathBuf,
    /// Whitelist-generated activation arguments (may be empty).
    pub arguments: String,
}

/// Everything the worker observed, delivered as one bounded receipt. Field
/// names deliberately separate "activation returned" from "identity matched"
/// from "target stayed alive".
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivationWorkerReceipt {
    pub version: u32,
    /// The request was rejected before COM was ever involved.
    pub failure: Option<String>,
    /// `ActivateApplication` returned a PID (a failed HRESULT keeps `false`).
    pub activation_returned: bool,
    /// Formatted `HRESULT` when the activation call itself failed.
    pub activation_hresult: Option<String>,
    pub pid: Option<u32>,
    pub instance: InstanceKind,
    pub package_identity: ObservationKind,
    pub observed_package_full_name: Option<String>,
    pub aumid: AumidKind,
    pub observed_aumid: Option<String>,
    pub elevation: Option<bool>,
    /// Exit code when the target died inside the observation window.
    pub early_exit_code: Option<u32>,
    /// Observation-level failure that does not change the activation fact.
    pub observation_failure: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InstanceKind {
    #[default]
    Unknown,
    Created,
    Reused,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    #[default]
    NotQueried,
    Matched,
    Missing,
    Mismatch,
    QueryFailed,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AumidKind {
    #[default]
    NotQueried,
    Matched,
    Missing,
    Mismatch,
    QueryFailed,
}

impl ActivationWorkerReceipt {
    fn rejected(failure: String) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            failure: Some(failure),
            activation_returned: false,
            activation_hresult: None,
            pid: None,
            instance: InstanceKind::Unknown,
            package_identity: ObservationKind::NotQueried,
            observed_package_full_name: None,
            aumid: AumidKind::NotQueried,
            observed_aumid: None,
            elevation: None,
            early_exit_code: None,
            observation_failure: None,
        }
    }
}

fn protocol_error(detail: impl AsRef<str>) -> String {
    format!("APPX_ACTIVATION_PROTOCOL_INVALID: {}", detail.as_ref())
}

/// Validates the exact shape Guard itself produces. The parent validates so
/// an internal bug cannot smuggle an oversized or malformed request; the
/// worker re-validates because stdin is still external input.
pub fn validate_request(request: &ActivationWorkerRequest) -> Result<(), String> {
    if request.version != PROTOCOL_VERSION {
        return Err(protocol_error("unsupported protocol version"));
    }
    let aumid = &request.aumid;
    if aumid.is_empty() || aumid.len() > 2048 || aumid.contains('\0') {
        return Err(protocol_error("invalid AUMID"));
    }
    if aumid
        .chars()
        .any(|c| c.is_whitespace() || !c.is_ascii_graphic())
    {
        return Err(protocol_error("invalid AUMID"));
    }
    let Some((family, application)) = aumid.split_once('!') else {
        return Err(protocol_error("AUMID must be FamilyName!ApplicationId"));
    };
    if family.is_empty() || application.is_empty() {
        return Err(protocol_error("AUMID must be FamilyName!ApplicationId"));
    }
    let package = &request.expected_package_full_name;
    if package.is_empty()
        || package.len() > 1024
        || package.contains('\0')
        || package
            .chars()
            .any(|c| c.is_whitespace() || !c.is_ascii_graphic())
    {
        return Err(protocol_error("invalid package full name"));
    }
    let executable = &request.expected_executable;
    if !executable.is_absolute()
        || executable.as_os_str().is_empty()
        || executable.as_os_str().to_string_lossy().contains('\0')
    {
        return Err(protocol_error("invalid expected executable"));
    }
    let arguments = &request.arguments;
    if arguments.len() > 4096 {
        return Err(protocol_error("arguments exceed the 4 KiB budget"));
    }
    if arguments.chars().any(|c| c != ' ' && !c.is_ascii_graphic()) {
        return Err(protocol_error("arguments must be printable ASCII"));
    }
    let encoded = serde_json::to_vec(request)
        .map_err(|error| protocol_error(format!("cannot serialize request: {error}")))?;
    if encoded.len() > MAX_REQUEST_BYTES {
        return Err(protocol_error("request exceeds the 16 KiB budget"));
    }
    Ok(())
}

/// Parses one bounded request from the worker's stdin.
fn parse_request(input: &[u8]) -> Result<ActivationWorkerRequest, String> {
    if input.len() > MAX_REQUEST_BYTES {
        return Err(protocol_error("request exceeds the 16 KiB budget"));
    }
    let request: ActivationWorkerRequest = serde_json::from_slice(input)
        .map_err(|error| protocol_error(format!("invalid JSON: {error}")))?;
    validate_request(&request)?;
    Ok(request)
}

/// The worker entry point: read one bounded request, run the activation and
/// its observations on this thread, and emit exactly one receipt line.
/// Returns `Err` only for pre-activation protocol failures, which the caller
/// reports on stderr with a non-zero exit; every activation outcome —
/// including a failed HRESULT — is a receipt.
#[cfg(windows)]
pub fn run_activation_worker(
    input: &mut dyn Read,
    output: &mut dyn std::io::Write,
) -> Result<(), String> {
    let mut buffer = Vec::new();
    input
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut buffer)
        .map_err(|error| format!("APPX_ACTIVATION_WORKER_IO: cannot read request: {error}"))?;
    let request = parse_request(&buffer)?;
    let receipt = activation_impl::activate_and_observe(&request);
    let mut line = serde_json::to_string(&receipt)
        .map_err(|error| protocol_error(format!("cannot serialize receipt: {error}")))?;
    line.push('\n');
    output
        .write_all(line.as_bytes())
        .and_then(|()| output.flush())
        .map_err(|error| format!("APPX_ACTIVATION_WORKER_IO: cannot write receipt: {error}"))
}

#[cfg(not(windows))]
pub fn run_activation_worker(
    _input: &mut dyn Read,
    _output: &mut dyn std::io::Write,
) -> Result<(), String> {
    Err("APPX_ACTIVATION_UNSUPPORTED: application activation requires Windows".into())
}

/// The parent-side result of one activation attempt.
#[derive(Clone, Debug)]
pub struct ActivationOutcome {
    pub receipt: ActivationWorkerReceipt,
}

/// Runs the hidden worker and classifies its outcome. Never retries, never
/// falls back to another launch backend, and never terminates the target.
#[cfg(windows)]
pub async fn activate_registered_desktop(
    request: &ActivationWorkerRequest,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<ActivationOutcome, String> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    validate_request(request)?;
    let guard_executable = current_guard_executable()?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let payload = serde_json::to_vec(request)
        .map_err(|error| protocol_error(format!("cannot serialize request: {error}")))?;
    let mut command = tokio::process::Command::new(&guard_executable);
    command
        .arg("internal-activate-package")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .map_err(|error| format!("APPX_ACTIVATION_WORKER_START_FAILED: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or("APPX_ACTIVATION_WORKER_IO: worker stdin was not captured")?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let submitted = AtomicBool::new(false);

    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(cancel_outcome(submitted.load(Ordering::Acquire))),
        result = async {
            if let Err(error) = stdin.write_all(&payload).await {
                // A failed write never delivered complete JSON, so the worker
                // cannot have activated anything from it.
                return Err(format!("APPX_ACTIVATION_WORKER_IO: cannot send request: {error}"));
            }
            // The complete request has been handed over; from here on, any
            // failure or cancellation leaves the activation outcome unknown.
            submitted.store(true, Ordering::Release);
            if let Err(error) = stdin.flush().await {
                return Err(format!("APPX_ACTIVATION_WORKER_IO: cannot flush request: {error}"));
            }
            drop(stdin);
            let mut stdout_bytes = Vec::new();
            let mut stderr_bytes = Vec::new();
            if let Some(stream) = stdout.as_mut() {
                stream
                    .take((MAX_RECEIPT_BYTES + 1) as u64)
                    .read_to_end(&mut stdout_bytes)
                    .await
                    .map_err(|error| {
                        format!("APPX_ACTIVATION_WORKER_IO: cannot read receipt: {error}")
                    })?;
            }
            if let Some(stream) = stderr.as_mut() {
                // Diagnostics only; a bounded tail keeps hostile output small.
                let _ = stream.take(2048_u64).read_to_end(&mut stderr_bytes).await;
            }
            let status = child.wait().await.map_err(|error| {
                format!("APPX_ACTIVATION_WORKER_IO: cannot wait for worker: {error}")
            })?;
            Ok((stdout_bytes, stderr_bytes, status))
        } => match result {
            Ok((stdout_bytes, stderr_bytes, status)) => classify_worker_output(
                &stdout_bytes,
                &stderr_bytes,
                status,
                submitted.load(Ordering::Acquire),
            ),
            Err(error) => Err(outcome_unknown_or_plain(
                error,
                submitted.load(Ordering::Acquire),
            )),
        },
        _ = tokio::time::sleep(ACTIVATION_BUDGET) => Err(format!(
            "{}: activation worker did not finish within 30 seconds",
            outcome_prefix(submitted.load(Ordering::Acquire))
        )),
    };

    // Reap Guard's own short-lived worker; the activated Desktop is never in
    // a Guard job and is never terminated here.
    let _ = tokio::time::timeout(WORKER_CLEANUP_BUDGET, child.wait()).await;
    let _ = child.start_kill();
    result
}

#[cfg(not(windows))]
pub async fn activate_registered_desktop(
    _request: &ActivationWorkerRequest,
    _cancellation: &tokio_util::sync::CancellationToken,
) -> Result<ActivationOutcome, String> {
    Err("APPX_ACTIVATION_UNSUPPORTED: application activation requires Windows".into())
}

#[cfg(windows)]
fn current_guard_executable() -> Result<PathBuf, String> {
    let path = std::env::current_exe().map_err(|error| {
        format!("APPX_ACTIVATION_WORKER_PATH_FAILED: cannot resolve Guard executable: {error}")
    })?;
    if !path.is_absolute() || !path.is_file() {
        return Err(
            "APPX_ACTIVATION_WORKER_PATH_FAILED: Guard executable is not an absolute file".into(),
        );
    }
    Ok(path)
}

#[cfg(windows)]
fn classify_worker_output(
    stdout_bytes: &[u8],
    stderr_bytes: &[u8],
    status: std::process::ExitStatus,
    submitted: bool,
) -> Result<ActivationOutcome, String> {
    if stdout_bytes.len() > MAX_RECEIPT_BYTES {
        return Err(outcome_unknown_or_plain(
            protocol_error("receipt exceeds the 64 KiB budget"),
            submitted,
        ));
    }
    let text = String::from_utf8_lossy(stdout_bytes);
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        let detail = if !stderr_bytes.is_empty() {
            format!(
                "; worker said: {}",
                proxy_guard_core::redact_text(&String::from_utf8_lossy(stderr_bytes))
                    .trim()
                    .replace(['\r', '\n'], " ")
            )
        } else if !status.success() {
            format!("; worker exited with {status}")
        } else {
            String::new()
        };
        return Err(outcome_unknown_or_plain(
            format!("APPX_ACTIVATION_WORKER_NO_RECEIPT: the worker produced no receipt{detail}"),
            submitted,
        ));
    }
    let receipt: ActivationWorkerReceipt = serde_json::from_str(line).map_err(|error| {
        outcome_unknown_or_plain(
            protocol_error(format!("invalid receipt JSON: {error}")),
            submitted,
        )
    })?;
    if let Some(failure) = &receipt.failure {
        // A worker-side pre-activation rejection is safe to report plainly.
        return Err(failure.clone());
    }
    Ok(ActivationOutcome { receipt })
}

fn outcome_prefix(submitted: bool) -> &'static str {
    if submitted {
        "APPX_ACTIVATION_OUTCOME_UNKNOWN"
    } else {
        "APPX_ACTIVATION_WORKER_FAILED"
    }
}

fn outcome_unknown_or_plain(error: String, submitted: bool) -> String {
    if !submitted {
        // The request never left Guard; retrying this launch is safe.
        return error;
    }
    // The request reached Windows; it may already have activated.
    format!(
        "APPX_ACTIVATION_OUTCOME_UNKNOWN: the activation request was submitted but the \
         outcome is unknown ({error}); refresh the Desktop state instead of retrying \
         automatically"
    )
}

fn cancel_outcome(submitted: bool) -> String {
    if submitted {
        "APPX_ACTIVATION_OUTCOME_UNKNOWN: cancelled after the activation request was \
         submitted; Windows may still have activated the application"
            .into()
    } else {
        "LAUNCH_CANCELLED: Guard is shutting down before the activation request was submitted"
            .into()
    }
}

/// COM activation and target observation, isolated on the worker's thread.
#[cfg(windows)]
mod activation_impl {
    use ::windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use ::windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use ::windows::Win32::UI::Shell::{
        AO_NONE, ApplicationActivationManager, IApplicationActivationManager,
    };
    use ::windows::core::HRESULT;

    use super::*;
    use crate::package_identity::{
        ProcessPackageIdentity, observe_early_exit, open_process_for_observation,
        query_process_aumid, query_process_creation_time_unix_ms, query_process_elevation,
        query_process_image_path, query_process_package_identity, query_system_time_unix_ms,
        windows_paths_equal,
    };

    /// Performs the activation and every post-activation observation on the
    /// calling thread, with COM initialized and released here and nowhere else.
    pub fn activate_and_observe(request: &ActivationWorkerRequest) -> ActivationWorkerReceipt {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr != HRESULT(0) && hr != RPC_E_CHANGED_MODE {
            let mut receipt = ActivationWorkerReceipt::rejected(String::new());
            receipt.failure = Some(format!(
                "APPX_ACTIVATION_COM_INIT_FAILED: CoInitializeEx returned 0x{:08X}",
                hr.0 as u32
            ));
            return receipt;
        }
        let receipt = activate_and_observe_com(request);
        unsafe { CoUninitialize() };
        receipt
    }

    fn activate_and_observe_com(request: &ActivationWorkerRequest) -> ActivationWorkerReceipt {
        let mut receipt = ActivationWorkerReceipt::rejected(String::new());
        receipt.failure = None;

        let manager: IApplicationActivationManager = match unsafe {
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
        } {
            Ok(manager) => manager,
            Err(error) => {
                receipt.failure = Some(format!(
                    "APPX_ACTIVATION_MANAGER_UNAVAILABLE: CoCreateInstance failed: {}",
                    format_hresult(error.code())
                ));
                return receipt;
            }
        };

        let submitted_at = query_system_time_unix_ms();
        let aumid = ::windows::core::HSTRING::from(request.aumid.as_str());
        let arguments = ::windows::core::HSTRING::from(request.arguments.as_str());
        let pid = match unsafe { manager.ActivateApplication(&aumid, &arguments, AO_NONE) } {
            Ok(pid) => pid,
            Err(error) => {
                receipt.activation_hresult = Some(format_hresult(error.code()));
                return receipt;
            }
        };
        receipt.activation_returned = true;
        receipt.pid = Some(pid);
        observe_target(request, pid, submitted_at, &mut receipt);
        receipt
    }

    /// Holds one handle to the exact returned PID for every observation, so a
    /// recycled PID cannot substitute a different process mid-verification.
    fn observe_target(
        request: &ActivationWorkerRequest,
        pid: u32,
        submitted_at: u64,
        receipt: &mut ActivationWorkerReceipt,
    ) {
        let handle = match open_process_for_observation(pid) {
            Ok(handle) => handle,
            Err(error) => {
                receipt.observation_failure = Some(error);
                receipt.instance = InstanceKind::Unknown;
                receipt.package_identity = ObservationKind::QueryFailed;
                receipt.aumid = AumidKind::QueryFailed;
                return;
            }
        };
        match query_process_package_identity(&handle) {
            Ok(ProcessPackageIdentity::Packaged(actual)) => {
                receipt.observed_package_full_name = Some(actual.clone());
                receipt.package_identity = if actual == request.expected_package_full_name {
                    ObservationKind::Matched
                } else {
                    ObservationKind::Mismatch
                };
            }
            Ok(ProcessPackageIdentity::Unpackaged) => {
                receipt.package_identity = ObservationKind::Missing;
            }
            Err(error) => {
                receipt.observation_failure = Some(error);
                receipt.package_identity = ObservationKind::QueryFailed;
            }
        }
        match query_process_aumid(&handle) {
            Ok(Some(actual)) => {
                receipt.observed_aumid = Some(actual.clone());
                receipt.aumid = if actual == request.aumid {
                    AumidKind::Matched
                } else {
                    AumidKind::Mismatch
                };
            }
            Ok(None) => receipt.aumid = AumidKind::Missing,
            Err(error) => {
                receipt.observation_failure = Some(error);
                receipt.aumid = AumidKind::QueryFailed;
            }
        }
        receipt.elevation = query_process_elevation(&handle).ok();
        match query_process_creation_time_unix_ms(&handle) {
            Ok(created) => {
                receipt.instance = if created + CREATION_CLOCK_SLACK_MS < submitted_at {
                    InstanceKind::Reused
                } else {
                    InstanceKind::Created
                };
            }
            Err(error) => {
                receipt.observation_failure = Some(error);
                receipt.instance = InstanceKind::Unknown;
            }
        }
        // The executable check is extra evidence on top of package and
        // application identity; a failed query does not invalidate those.
        if let Ok(path) = query_process_image_path(&handle)
            && !windows_paths_equal(&path, &request.expected_executable)
        {
            receipt.observation_failure.get_or_insert_with(|| {
                "APPX_TARGET_IMAGE_MISMATCH: the activated target's executable does not \
                     match the registered Desktop entry"
                    .into()
            });
        }
        match observe_early_exit(&handle, EARLY_EXIT_WINDOW_MS) {
            Ok(Some(code)) => receipt.early_exit_code = Some(code),
            Ok(None) => {}
            Err(error) => {
                receipt.observation_failure.get_or_insert(error);
            }
        }
    }

    fn format_hresult(code: HRESULT) -> String {
        format!("0x{:08X}", code.0 as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ActivationWorkerRequest {
        ActivationWorkerRequest {
            version: PROTOCOL_VERSION,
            aumid: "OpenAI.Codex_2p2nqsd0c76g0!App".into(),
            expected_package_full_name: "OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0".into(),
            expected_executable: PathBuf::from(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
            ),
            arguments: "--proxy-server=\"http://127.0.0.1:10808\" --proxy-bypass-list=\"localhost;127.0.0.1;[::1]\"".into(),
        }
    }

    #[test]
    fn accepts_the_shape_guard_produces() {
        assert!(validate_request(&request()).is_ok());
        let mut no_arguments = request();
        no_arguments.arguments.clear();
        assert!(validate_request(&no_arguments).is_ok());
    }

    #[test]
    fn rejects_malformed_identity_fields() {
        let mut bad = request();
        bad.aumid = "no-bang".into();
        assert!(validate_request(&bad).is_err());
        bad.aumid = "family!".into();
        assert!(validate_request(&bad).is_err());
        bad.aumid = "family!app extra".into();
        assert!(validate_request(&bad).is_err());
        bad = request();
        bad.expected_executable = PathBuf::from(r"relative\ChatGPT.exe");
        assert!(validate_request(&bad).is_err());
        bad = request();
        bad.version = 99;
        assert!(validate_request(&bad).is_err());
    }

    #[test]
    fn rejects_non_printable_or_oversized_arguments() {
        let mut bad = request();
        bad.arguments = "--proxy-server=\"http://127.0.0.1:10808\"\n".into();
        assert!(validate_request(&bad).is_err());
        bad.arguments = "x".repeat(4097);
        assert!(validate_request(&bad).is_err());
        bad.arguments = "--flag=日本".into();
        assert!(validate_request(&bad).is_err());
    }

    #[test]
    fn parses_a_bounded_request_from_worker_stdin_shape() {
        let encoded = serde_json::to_vec(&request()).unwrap();
        assert!(parse_request(&encoded).is_ok());
        let oversized = vec![b'x'; MAX_REQUEST_BYTES + 1];
        assert!(
            parse_request(&oversized)
                .unwrap_err()
                .contains("16 KiB budget")
        );
    }

    #[test]
    fn pre_submission_worker_failure_is_plain_but_post_submission_is_unknown() {
        assert_eq!(
            outcome_unknown_or_plain(
                "APPX_ACTIVATION_WORKER_IO: cannot send request: broken pipe".into(),
                false
            ),
            "APPX_ACTIVATION_WORKER_IO: cannot send request: broken pipe"
        );
        let unknown = outcome_unknown_or_plain(
            "APPX_ACTIVATION_WORKER_IO: cannot read receipt: closed".into(),
            true,
        );
        assert!(unknown.starts_with("APPX_ACTIVATION_OUTCOME_UNKNOWN:"));
        assert!(unknown.contains("refresh the Desktop state"));
    }

    #[test]
    fn cancellation_before_submission_guarantees_no_request() {
        assert!(cancel_outcome(false).starts_with("LAUNCH_CANCELLED"));
        assert!(cancel_outcome(true).starts_with("APPX_ACTIVATION_OUTCOME_UNKNOWN"));
    }

    #[test]
    fn timeout_prefix_reflects_submission_state() {
        assert_eq!(outcome_prefix(false), "APPX_ACTIVATION_WORKER_FAILED");
        assert_eq!(outcome_prefix(true), "APPX_ACTIVATION_OUTCOME_UNKNOWN");
    }

    #[cfg(windows)]
    #[test]
    fn worker_rejects_invalid_requests_before_com_without_activating() {
        let mut bad = request();
        bad.aumid = "malformed aumid".into();
        let encoded = serde_json::to_vec(&bad).unwrap();
        let mut input = encoded.as_slice();
        let mut output = Vec::new();
        let error = run_activation_worker(&mut input, &mut output).unwrap_err();
        assert!(
            error.contains("APPX_ACTIVATION_PROTOCOL_INVALID"),
            "{error}"
        );
        assert!(
            output.is_empty(),
            "no receipt may be emitted for a rejected request"
        );
    }
}
