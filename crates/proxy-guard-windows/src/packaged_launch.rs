//! Explicit, single-use package-context launch candidate for FullTrust Desktop.
//!
//! This is deliberately a small private protocol, not a general process
//! launcher.  The parent creates one local pipe and only sends the launch
//! permit after it has authenticated the package-context helper.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use proxy_guard_core::{DesktopAppInfo, PackageApplication};
use tokio_util::sync::CancellationToken;

use crate::environment::ProxyEnvironment;

const MAX_MESSAGE_BYTES: usize = 16 * 1024;
const PROTOCOL_VERSION: u8 = 1;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Message {
    Hello {
        version: u8,
        nonce: String,
        package_full_name: String,
    },
    Launch {
        version: u8,
        nonce: String,
        package_full_name: String,
        executable: PathBuf,
        manifest_executable: PathBuf,
        proxy_url: String,
        no_proxy: String,
        codex_home: Option<PathBuf>,
    },
    Result {
        version: u8,
        nonce: String,
        result: HelperResult,
    },
    Ack {
        version: u8,
        nonce: String,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "status", rename_all = "snake_case")]
enum HelperResult {
    Created { pid: u32, package_full_name: String },
    Failed { code: String },
}

fn protocol_error(detail: impl AsRef<str>) -> String {
    format!("APPX_PACKAGE_PROTOCOL_INVALID: {}", detail.as_ref())
}

fn validate_text(value: &str, field: &str, max: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > max || value.contains('\0') {
        return Err(protocol_error(format!("invalid {field}")));
    }
    Ok(())
}

fn validate_argument_token(value: &str, field: &str, max: usize) -> Result<(), String> {
    validate_text(value, field, max)?;
    if value.chars().any(char::is_whitespace) {
        return Err(protocol_error(format!("invalid {field}")));
    }
    Ok(())
}

fn is_loopback_http_proxy(value: &str) -> bool {
    let Some(authority) = value.strip_prefix("http://") else {
        return false;
    };
    if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
        return false;
    }
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    if !matches!(port.parse::<u16>(), Ok(1..=u16::MAX)) {
        return false;
    }
    let host = host.trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Once the permit boundary is crossed, absence of a complete success receipt
/// is never evidence that Desktop was not created.  Keep an explicit helper
/// failure distinct because it is a receipt from the authenticated helper,
/// while still warning callers that `CreateProcess` may already have happened.
fn classify_after_permit(error: String, permit_sent: bool) -> String {
    if !permit_sent || error.starts_with("APPX_PACKAGE_LAUNCH_OUTCOME_UNKNOWN:") {
        return error;
    }
    if error.starts_with("APPX_PACKAGE_HELPER_FAILED:") {
        return format!("{error}; Desktop may already have been created");
    }
    format!(
        "APPX_PACKAGE_LAUNCH_OUTCOME_UNKNOWN: launch permit was sent; confirmation failed at {error}"
    )
}

fn format_context_helper_failure(exit_code: &str, stderr: &str) -> String {
    let summary = proxy_guard_core::redact_text(stderr).replace(['\r', '\n'], " ");
    if summary.trim().is_empty() {
        format!("APPX_CONTEXT_HELPER_FAILED: PowerShell exited with {exit_code}")
    } else {
        format!(
            "APPX_CONTEXT_HELPER_FAILED: PowerShell exited with {exit_code}; {}",
            summary.trim()
        )
    }
}

fn validate_launch(
    expected_package: &str,
    nonce: &str,
    message: Message,
) -> Result<(PathBuf, PathBuf, ProxyEnvironment, Option<PathBuf>), String> {
    let Message::Launch {
        version,
        nonce: received_nonce,
        package_full_name,
        executable,
        manifest_executable,
        proxy_url,
        no_proxy,
        codex_home,
    } = message
    else {
        return Err(protocol_error("expected launch request"));
    };
    if version != PROTOCOL_VERSION
        || received_nonce != nonce
        || package_full_name != expected_package
    {
        return Err(protocol_error(
            "launch request does not match this operation",
        ));
    }
    validate_text(&proxy_url, "proxy URL", 2048)?;
    if !is_loopback_http_proxy(&proxy_url) {
        return Err(protocol_error("proxy URL must be loopback HTTP"));
    }
    validate_text(&no_proxy, "NO_PROXY", 4096)?;
    if !executable.is_absolute() || executable.as_os_str().to_string_lossy().contains('\0') {
        return Err(protocol_error("invalid executable"));
    }
    if manifest_executable.is_absolute()
        || manifest_executable.as_os_str().is_empty()
        || manifest_executable.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(protocol_error("invalid manifest executable"));
    }
    if codex_home
        .as_ref()
        .is_some_and(|home| !home.is_absolute() || home.as_os_str().is_empty())
    {
        return Err(protocol_error("invalid CODEX_HOME"));
    }
    Ok((
        executable,
        manifest_executable,
        ProxyEnvironment {
            proxy_url,
            no_proxy,
        },
        codex_home,
    ))
}

#[cfg(not(windows))]
pub async fn launch_packaged(
    _: &DesktopAppInfo,
    _: &PackageApplication,
    _: &ProxyEnvironment,
    _: Option<&Path>,
    _: &CancellationToken,
) -> Result<u32, String> {
    Err("APPX_PROXY_LAUNCH_UNSUPPORTED: package-context launch requires Windows".into())
}

#[cfg(not(windows))]
pub async fn run_package_helper(_: &str, _: &str) -> Result<(), String> {
    Err("APPX_PROXY_LAUNCH_UNSUPPORTED: package-context helper requires Windows".into())
}

#[cfg(windows)]
mod windows {
    use std::{
        ffi::OsString,
        os::windows::{
            ffi::OsStringExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
            process::CommandExt,
        },
        process::Stdio,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use tokio::{
        io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
        net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions},
        process::Command,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
        Storage::Packaging::Appx::GetPackagePathByFullName,
        System::{
            Pipes::GetNamedPipeClientProcessId,
            Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
                WaitForSingleObject,
            },
        },
    };

    use super::*;
    use crate::{
        environment::apply_proxy_environment,
        package_identity::{
            ProcessPackageIdentity, query_pid_package_identity, query_process_package_identity,
        },
    };

    const LAUNCH_BUDGET: Duration = Duration::from_secs(30);
    const CLEANUP_BUDGET: Duration = Duration::from_secs(3);
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    const HELPER_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$packageFamilyName = $env:CPG_PACKAGE_FAMILY_NAME
$appId = $env:CPG_APPLICATION_ID
$guardExecutable = $env:CPG_GUARD_EXECUTABLE
$pipe = $env:CPG_PIPE
$nonce = $env:CPG_NONCE
if ([string]::IsNullOrWhiteSpace($packageFamilyName) -or [string]::IsNullOrWhiteSpace($appId) -or
    [string]::IsNullOrWhiteSpace($guardExecutable) -or [string]::IsNullOrWhiteSpace($pipe) -or
    [string]::IsNullOrWhiteSpace($nonce)) {
  throw 'APPX_HELPER_START_FAILED: missing bounded package helper input'
}
$helperArgs = 'package-helper --pipe ' + $pipe + ' --nonce ' + $nonce
Invoke-CommandInDesktopPackage -PackageFamilyName $packageFamilyName -AppId $appId -Command $guardExecutable -Args $helperArgs -PreventBreakaway
"#;

    fn current_guard_executable() -> Result<PathBuf, String> {
        let path = std::env::current_exe().map_err(|error| {
            format!("APPX_HELPER_PATH_FAILED: cannot resolve Guard executable: {error}")
        })?;
        if !path.is_absolute() || !path.is_file() {
            return Err("APPX_HELPER_PATH_FAILED: Guard executable is not an absolute file".into());
        }
        Ok(path)
    }

    fn random_nonce() -> Result<String, String> {
        let mut bytes = [0_u8; 32];
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(format!(
                "APPX_HELPER_RANDOM_FAILED: BCryptGenRandom returned {status}"
            ));
        }
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    fn system_powershell() -> Result<PathBuf, String> {
        crate::system_tools::system_powershell()
            .map_err(|error| format!("APPX_HELPER_START_FAILED: {error}"))
    }

    async fn send_message(
        pipe: &mut (impl AsyncWriteExt + Unpin),
        message: &Message,
    ) -> Result<(), String> {
        let body = serde_json::to_vec(message)
            .map_err(|error| protocol_error(format!("serialize failed: {error}")))?;
        if body.len() > MAX_MESSAGE_BYTES {
            return Err(protocol_error("message exceeds 16 KiB"));
        }
        pipe.write_u32_le(body.len() as u32)
            .await
            .map_err(|error| format!("APPX_PACKAGE_PIPE_IO: write length failed: {error}"))?;
        pipe.write_all(&body)
            .await
            .map_err(|error| format!("APPX_PACKAGE_PIPE_IO: write body failed: {error}"))?;
        pipe.flush()
            .await
            .map_err(|error| format!("APPX_PACKAGE_PIPE_IO: flush failed: {error}"))
    }

    async fn receive_message(pipe: &mut (impl AsyncReadExt + Unpin)) -> Result<Message, String> {
        let length = pipe
            .read_u32_le()
            .await
            .map_err(|error| format!("APPX_PACKAGE_PIPE_IO: read length failed: {error}"))?
            as usize;
        if length == 0 || length > MAX_MESSAGE_BYTES {
            return Err(protocol_error("message size is out of bounds"));
        }
        let mut body = vec![0; length];
        pipe.read_exact(&mut body)
            .await
            .map_err(|error| format!("APPX_PACKAGE_PIPE_IO: read body failed: {error}"))?;
        serde_json::from_slice(&body)
            .map_err(|error| protocol_error(format!("invalid JSON: {error}")))
    }

    async fn drain_bounded(
        mut reader: impl AsyncRead + Unpin + Send + 'static,
        limit: usize,
    ) -> String {
        let mut buffer = [0_u8; 1024];
        let mut retained = Vec::with_capacity(limit);
        loop {
            let count = match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            let remaining = limit.saturating_sub(retained.len());
            retained.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        String::from_utf8_lossy(&retained).into_owned()
    }

    fn context_helper_failure(status: std::process::ExitStatus, stderr: &str) -> String {
        let code = status
            .code()
            .map_or_else(|| "terminated".to_string(), |value| value.to_string());
        format_context_helper_failure(&code, stderr)
    }

    async fn finish_capture(task: &mut tokio::task::JoinHandle<String>) -> String {
        match tokio::time::timeout(CLEANUP_BUDGET, &mut *task).await {
            Ok(Ok(value)) => value,
            _ => {
                task.abort();
                String::new()
            }
        }
    }

    fn client_pid(pipe: &NamedPipeServer) -> Result<u32, String> {
        let mut pid = 0u32;
        let ok = unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle() as HANDLE, &mut pid) };
        if ok == 0 || pid == 0 {
            return Err("APPX_HELPER_AUTH_FAILED: cannot identify pipe client".into());
        }
        Ok(pid)
    }

    fn process_image(pid: u32) -> Result<PathBuf, String> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return Err(format!(
                "APPX_HELPER_AUTH_FAILED: cannot open helper PID {pid}"
            ));
        }
        struct Handle(HANDLE);
        impl Drop for Handle {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        let _handle = Handle(handle);
        let mut size = 32_768u32;
        let mut buffer = vec![0u16; size as usize];
        let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
        if ok == 0 || size == 0 || size as usize > buffer.len() {
            return Err("APPX_HELPER_AUTH_FAILED: cannot query helper executable".into());
        }
        Ok(PathBuf::from(OsString::from_wide(&buffer[..size as usize])))
    }

    fn process_image_handle(handle: &impl AsRawHandle) -> Result<PathBuf, String> {
        let handle = handle.as_raw_handle() as HANDLE;
        let mut size = 32_768u32;
        let mut buffer = vec![0u16; size as usize];
        let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
        if ok == 0 || size == 0 || size as usize > buffer.len() {
            return Err("APPX_TARGET_VERIFY_FAILED: cannot query target executable".into());
        }
        Ok(PathBuf::from(OsString::from_wide(&buffer[..size as usize])))
    }

    fn open_target_handle(pid: u32) -> Result<OwnedHandle, String> {
        if pid == 0 {
            return Err("APPX_TARGET_VERIFY_FAILED: invalid target PID".into());
        }
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE_ACCESS,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return Err(format!(
                "APPX_TARGET_VERIFY_FAILED: cannot open target PID {pid} immediately after creation"
            ));
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    fn equal_path(left: &Path, right: &Path) -> Result<bool, String> {
        let left = std::fs::canonicalize(left).map_err(|error| {
            format!(
                "APPX_PATH_VERIFY_FAILED: cannot canonicalize {}: {error}",
                left.display()
            )
        })?;
        let right = std::fs::canonicalize(right).map_err(|error| {
            format!(
                "APPX_PATH_VERIFY_FAILED: cannot canonicalize {}: {error}",
                right.display()
            )
        })?;
        fn normalize(path: &Path) -> String {
            let value = path.as_os_str().to_string_lossy().replace('/', "\\");
            if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
                format!(r"\\{rest}")
            } else if let Some(rest) = value.strip_prefix(r"\\?\") {
                rest.to_owned()
            } else {
                value
            }
        }
        Ok(normalize(&left).eq_ignore_ascii_case(&normalize(&right)))
    }

    fn installed_package_path(package_full_name: &str) -> Result<PathBuf, String> {
        let mut full_name: Vec<u16> = package_full_name.encode_utf16().chain(Some(0)).collect();
        let mut length = 0_u32;
        let status = unsafe {
            GetPackagePathByFullName(full_name.as_mut_ptr(), &mut length, std::ptr::null_mut())
        };
        if status != 122 || !(2..=32_768).contains(&length) {
            return Err(format!(
                "APPX_HELPER_IDENTITY_FAILED: GetPackagePathByFullName size query returned {status}"
            ));
        }
        let mut path = vec![0_u16; length as usize];
        let status = unsafe {
            GetPackagePathByFullName(full_name.as_mut_ptr(), &mut length, path.as_mut_ptr())
        };
        if status != 0
            || length < 2
            || length as usize > path.len()
            || path[(length - 1) as usize] != 0
        {
            return Err(format!(
                "APPX_HELPER_IDENTITY_FAILED: GetPackagePathByFullName returned {status}"
            ));
        }
        Ok(PathBuf::from(OsString::from_wide(
            &path[..(length - 1) as usize],
        )))
    }

    fn authenticate_helper(
        pipe: &NamedPipeServer,
        expected_package: &str,
        expected_exe: &Path,
    ) -> Result<u32, String> {
        let pid = client_pid(pipe)?;
        match query_pid_package_identity(pid)? {
            ProcessPackageIdentity::Packaged(actual) if actual == expected_package => {}
            ProcessPackageIdentity::Packaged(actual) => {
                return Err(format!(
                    "APPX_HELPER_AUTH_FAILED: helper PID {pid} has package {actual}"
                ));
            }
            ProcessPackageIdentity::Unpackaged => {
                return Err(format!(
                    "APPX_HELPER_AUTH_FAILED: helper PID {pid} has no package identity"
                ));
            }
        }
        let actual_exe = process_image(pid)?;
        if !equal_path(&actual_exe, expected_exe)? {
            return Err(
                "APPX_HELPER_AUTH_FAILED: helper executable does not match this Guard".into(),
            );
        }
        Ok(pid)
    }

    fn verify_target(
        handle: &OwnedHandle,
        pid: u32,
        expected_package: &str,
        expected_exe: &Path,
    ) -> Result<(), String> {
        match query_process_package_identity(handle) {
            Ok(ProcessPackageIdentity::Packaged(actual)) if actual == expected_package => {}
            Ok(ProcessPackageIdentity::Packaged(actual)) => {
                return Err(format!(
                    "APPX_IDENTITY_MISMATCH: target PID {pid} has package {actual}"
                ));
            }
            Ok(ProcessPackageIdentity::Unpackaged) => {
                return Err(format!(
                    "APPX_IDENTITY_MISSING: target PID {pid} has no package identity"
                ));
            }
            Err(_) => {
                return Err(format!(
                    "APPX_LAUNCH_EARLY_EXIT_OR_UNQUERYABLE: target PID {pid} was unavailable after creation"
                ));
            }
        }
        let actual_exe = process_image_handle(handle)?;
        if !equal_path(&actual_exe, expected_exe)? {
            return Err(format!(
                "APPX_TARGET_MISMATCH: target PID {pid} executable does not match the registered Desktop entry"
            ));
        }
        // A successful CreateProcess can precede application initialization.
        // Waiting on this exact process object cannot observe a reused PID.
        let wait = unsafe { WaitForSingleObject(handle.as_raw_handle() as HANDLE, 150) };
        interpret_target_wait(wait, pid)
    }

    fn interpret_target_wait(wait: u32, pid: u32) -> Result<(), String> {
        match wait {
            WAIT_TIMEOUT => Ok(()),
            WAIT_OBJECT_0 => Err(format!(
                "APPX_LAUNCH_EARLY_EXIT: target PID {pid} exited during the 150 ms observation"
            )),
            WAIT_FAILED => Err(format!(
                "APPX_TARGET_VERIFY_FAILED: could not wait on target PID {pid}: {}",
                std::io::Error::last_os_error()
            )),
            _ => Err(format!(
                "APPX_TARGET_VERIFY_FAILED: unexpected wait result {wait} for target PID {pid}"
            )),
        }
    }

    #[test]
    fn target_wait_distinguishes_exit_from_query_failure() {
        assert!(interpret_target_wait(WAIT_TIMEOUT, 42).is_ok());
        assert!(
            interpret_target_wait(WAIT_OBJECT_0, 42)
                .unwrap_err()
                .starts_with("APPX_LAUNCH_EARLY_EXIT:")
        );
        assert!(
            interpret_target_wait(WAIT_FAILED, 42)
                .unwrap_err()
                .starts_with("APPX_TARGET_VERIFY_FAILED:")
        );
        assert!(
            interpret_target_wait(99, 42)
                .unwrap_err()
                .starts_with("APPX_TARGET_VERIFY_FAILED:")
        );
    }

    async fn terminate_helper(child: &mut tokio::process::Child) {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(CLEANUP_BUDGET, child.wait()).await;
    }

    pub async fn launch_packaged(
        info: &DesktopAppInfo,
        package: &PackageApplication,
        environment: &ProxyEnvironment,
        codex_home: Option<&Path>,
        cancellation: &CancellationToken,
    ) -> Result<u32, String> {
        if cancellation.is_cancelled() {
            return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
        }
        let guard_exe = current_guard_executable()?;
        let nonce = random_nonce()?;
        validate_argument_token(&nonce, "nonce", 256)?;
        let pipe_name = format!(r"\\.\pipe\codex-proxy-guard-package-{nonce}");
        validate_argument_token(&pipe_name, "pipe", 512)?;
        let mut pipe = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&pipe_name)
            .map_err(|error| {
                format!("APPX_PACKAGE_PIPE_FAILED: cannot create local pipe: {error}")
            })?;
        let mut command = Command::new(system_powershell()?);
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                HELPER_SCRIPT,
            ])
            .env("CPG_PACKAGE_FAMILY_NAME", &package.package_family_name)
            .env("CPG_APPLICATION_ID", &package.application_id)
            .env("CPG_GUARD_EXECUTABLE", &guard_exe)
            .env("CPG_PIPE", &pipe_name)
            .env("CPG_NONCE", &nonce)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
        let mut helper = command.spawn().map_err(|error| {
            format!("APPX_HELPER_START_FAILED: cannot start package helper: {error}")
        })?;
        let stdout = helper.stdout.take().ok_or_else(|| {
            "APPX_HELPER_START_FAILED: PowerShell stdout was not captured".to_string()
        })?;
        let stderr = helper.stderr.take().ok_or_else(|| {
            "APPX_HELPER_START_FAILED: PowerShell stderr was not captured".to_string()
        })?;
        let stdout_task = tokio::spawn(drain_bounded(stdout, 4 * 1024));
        let mut stderr_task = tokio::spawn(drain_bounded(stderr, 8 * 1024));
        let permit_sent = Arc::new(AtomicBool::new(false));
        let permit_for_operation = permit_sent.clone();
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(if permit_sent.load(Ordering::Acquire) {
                "APPX_PACKAGE_LAUNCH_OUTCOME_UNKNOWN: cancellation observed after the launch permit".to_string()
            } else {
                "LAUNCH_CANCELLED: package helper was not authorized".to_string()
            }),
            result = tokio::time::timeout(LAUNCH_BUDGET, async {
                tokio::select! {
                    result = pipe.connect() => result.map_err(|error| format!("APPX_PACKAGE_PIPE_FAILED: helper did not connect: {error}"))?,
                    status = helper.wait() => {
                        let status = status.map_err(|error| format!("APPX_CONTEXT_HELPER_FAILED: cannot wait for PowerShell: {error}"))?;
                        if !status.success() {
                            let stderr = finish_capture(&mut stderr_task).await;
                            return Err(context_helper_failure(status, &stderr));
                        }
                        // Invoke may have detached the authenticated helper. A clean
                        // PowerShell exit therefore does not decide the operation.
                        pipe.connect().await.map_err(|error| format!("APPX_PACKAGE_PIPE_FAILED: helper did not connect: {error}"))?;
                    }
                }
                authenticate_helper(&pipe, &package.package_full_name, &guard_exe)?;
                let hello = receive_message(&mut pipe).await?;
                match hello {
                    Message::Hello { version, nonce: received, package_full_name } if version == PROTOCOL_VERSION && received == nonce && package_full_name == package.package_full_name => {},
                    _ => return Err("APPX_HELPER_AUTH_FAILED: helper handshake does not match this launch".into()),
                }
                if cancellation.is_cancelled() { return Err("LAUNCH_CANCELLED: package helper was not authorized".into()); }
                let request = Message::Launch { version: PROTOCOL_VERSION, nonce: nonce.clone(), package_full_name: package.package_full_name.clone(), executable: info.executable.clone(), manifest_executable: PathBuf::from(&package.manifest_executable), proxy_url: environment.proxy_url.clone(), no_proxy: environment.no_proxy.clone(), codex_home: codex_home.map(Path::to_path_buf) };
                permit_for_operation.store(true, Ordering::Release);
                // Store before the first pipe byte.  A write may partially
                // succeed before yielding an error, so it is already an
                // irreversible launch permit from Guard's perspective.
                send_message(&mut pipe, &request).await?;
                let response = receive_message(&mut pipe).await?;
                let response_result = match response {
                    Message::Result { version, nonce: received, result: HelperResult::Created { pid, package_full_name } } if version == PROTOCOL_VERSION && received == nonce && pid != 0 && package_full_name == package.package_full_name => {
                        open_target_handle(pid).and_then(|target| {
                            verify_target(&target, pid, &package.package_full_name, &info.executable)
                                .map(|()| pid)
                        })
                    },
                    Message::Result { version, nonce: received, result: HelperResult::Failed { code } }
                        if version == PROTOCOL_VERSION && received == nonce => Err(format!("APPX_PACKAGE_HELPER_FAILED: {code}")),
                    _ => Err("APPX_PACKAGE_PROTOCOL_INVALID: invalid helper result".into()),
                };
                // Let the helper release its exact Child handle even when this
                // side's verification rejected the result.  Ack failure cannot
                // change the launch outcome already determined above.
                let _ = send_message(&mut pipe, &Message::Ack { version: PROTOCOL_VERSION, nonce: nonce.clone() }).await;
                response_result
            }) => result.map_err(|_| {
                if permit_sent.load(Ordering::Acquire) {
                    "APPX_PACKAGE_LAUNCH_OUTCOME_UNKNOWN: launch permit was sent but Desktop was not confirmed within 30 seconds".to_string()
                } else {
                    "APPX_PACKAGE_LAUNCH_TIMEOUT: package helper was not authorized within 30 seconds".to_string()
                }
            })?,
        };
        let result = result
            .map_err(|error| classify_after_permit(error, permit_sent.load(Ordering::Acquire)));
        // A response or permit was sent by now.  Killing this short-lived
        // PowerShell/helper pair cannot terminate Desktop; it is never in a Guard job.
        terminate_helper(&mut helper).await;
        stdout_task.abort();
        stderr_task.abort();
        result
    }

    pub async fn run_package_helper(pipe_name: &str, nonce: &str) -> Result<(), String> {
        crate::elevation::ensure_non_elevated()?;
        validate_argument_token(pipe_name, "pipe", 512)?;
        validate_argument_token(nonce, "nonce", 256)?;
        let own_pid = std::process::id();
        let own_package = match query_pid_package_identity(own_pid)? {
            ProcessPackageIdentity::Packaged(name) => name,
            ProcessPackageIdentity::Unpackaged => {
                return Err(
                    "APPX_HELPER_IDENTITY_MISSING: package helper has no package identity".into(),
                );
            }
        };
        let mut pipe = ClientOptions::new().open(pipe_name).map_err(|error| {
            format!("APPX_PACKAGE_PIPE_FAILED: cannot connect to Guard: {error}")
        })?;
        send_message(
            &mut pipe,
            &Message::Hello {
                version: PROTOCOL_VERSION,
                nonce: nonce.to_string(),
                package_full_name: own_package.clone(),
            },
        )
        .await?;
        let request = receive_message(&mut pipe).await?;
        let (executable, manifest_executable, environment, codex_home) =
            validate_launch(&own_package, nonce, request)?;
        let expected_executable = installed_package_path(&own_package)?.join(manifest_executable);
        if !equal_path(&executable, &expected_executable)? {
            return Err("APPX_PACKAGE_PROTOCOL_INVALID: executable does not match the registered Desktop entry".into());
        }
        let mut command = std::process::Command::new(&executable);
        // The PowerShell context helper uses captured output for bounded
        // diagnostics. Desktop must not inherit those pipe handles.
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        apply_proxy_environment(&mut command, &environment);
        for key in [
            "CPG_PACKAGE_FAMILY_NAME",
            "CPG_APPLICATION_ID",
            "CPG_GUARD_EXECUTABLE",
            "CPG_PIPE",
            "CPG_NONCE",
        ] {
            command.env_remove(key);
        }
        if let Some(home) = codex_home {
            command.env("CODEX_HOME", home);
        }
        let child = command.spawn().map_err(|error| {
            format!("CODEX_LAUNCH_FAILED: Desktop could not be started: {error}")
        })?;
        let pid = child.id();
        let result = match query_process_package_identity(&child) {
            Ok(ProcessPackageIdentity::Packaged(actual)) if actual == own_package => {
                HelperResult::Created {
                    pid,
                    package_full_name: actual,
                }
            }
            Ok(ProcessPackageIdentity::Packaged(actual)) => HelperResult::Failed {
                code: format!("APPX_IDENTITY_MISMATCH: target PID {pid} has package {actual}"),
            },
            Ok(ProcessPackageIdentity::Unpackaged) => HelperResult::Failed {
                code: format!("APPX_IDENTITY_MISSING: target PID {pid} has no package identity"),
            },
            Err(error) => HelperResult::Failed {
                code: format!("APPX_IDENTITY_QUERY_FAILED: target PID {pid}: {error}"),
            },
        };
        send_message(
            &mut pipe,
            &Message::Result {
                version: PROTOCOL_VERSION,
                nonce: nonce.to_string(),
                result,
            },
        )
        .await?;
        // Keep the exact Child handle until Guard either acknowledges the
        // handoff, disconnects, or this bounded wait elapses. Dropping a
        // std::process::Child never kills Desktop.
        match tokio::time::timeout(Duration::from_secs(3), receive_message(&mut pipe)).await {
            Ok(Ok(Message::Ack {
                version,
                nonce: received,
            })) if version == PROTOCOL_VERSION && received == nonce => {}
            // Disconnect, malformed data, and timeout all release only this
            // helper's observation handle; they never affect Desktop.
            _ => {}
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use windows::{launch_packaged, run_package_helper};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_request_requires_exact_one_shot_binding() {
        let message = Message::Launch {
            version: PROTOCOL_VERSION,
            nonce: "n".into(),
            package_full_name: "pkg".into(),
            executable: PathBuf::from(r"C:\\Apps\\Desktop.exe"),
            manifest_executable: PathBuf::from(r"app\\Desktop.exe"),
            proxy_url: "http://127.0.0.1:10808".into(),
            no_proxy: "localhost".into(),
            codex_home: None,
        };
        assert!(validate_launch("pkg", "n", message).is_ok());
    }

    #[test]
    fn launch_request_rejects_wrong_nonce_before_spawn() {
        let message = Message::Launch {
            version: PROTOCOL_VERSION,
            nonce: "wrong".into(),
            package_full_name: "pkg".into(),
            executable: PathBuf::from(r"C:\\Apps\\Desktop.exe"),
            manifest_executable: PathBuf::from(r"app\\Desktop.exe"),
            proxy_url: "http://127.0.0.1:10808".into(),
            no_proxy: "localhost".into(),
            codex_home: None,
        };
        assert!(
            validate_launch("pkg", "n", message)
                .unwrap_err()
                .contains("does not match")
        );
    }

    #[test]
    fn protocol_rejects_overlong_sensitive_text() {
        assert!(validate_text(&"x".repeat(2049), "proxy URL", 2048).is_err());
    }

    #[test]
    fn protocol_accepts_only_loopback_http_proxy_urls() {
        assert!(is_loopback_http_proxy("http://127.0.0.1:10808"));
        assert!(is_loopback_http_proxy("http://[::1]:10808"));
        assert!(!is_loopback_http_proxy("https://127.0.0.1:10808"));
        assert!(!is_loopback_http_proxy("http://example.com:10808"));
    }

    #[test]
    fn helper_argument_tokens_cannot_contain_whitespace() {
        assert!(validate_argument_token(r"\\.\pipe\one", "pipe", 512).is_ok());
        assert!(validate_argument_token("one two", "pipe", 512).is_err());
    }

    #[test]
    fn pre_permit_failure_remains_safe_to_retry() {
        assert_eq!(
            classify_after_permit(
                "APPX_PACKAGE_PIPE_FAILED: helper did not connect".into(),
                false
            ),
            "APPX_PACKAGE_PIPE_FAILED: helper did not connect"
        );
    }

    #[test]
    fn post_permit_pipe_failure_is_outcome_unknown() {
        let result = classify_after_permit("APPX_PACKAGE_PIPE_IO: read body failed".into(), true);
        assert!(result.starts_with("APPX_PACKAGE_LAUNCH_OUTCOME_UNKNOWN:"));
        assert!(result.contains("APPX_PACKAGE_PIPE_IO"));
    }

    #[test]
    fn explicit_helper_failure_warns_that_desktop_may_exist() {
        let result = classify_after_permit(
            "APPX_PACKAGE_HELPER_FAILED: APPX_IDENTITY_MISSING".into(),
            true,
        );
        assert!(result.contains("Desktop may already have been created"));
    }

    #[test]
    fn context_helper_failure_is_bounded_format_with_exit_code() {
        let result = format_context_helper_failure("1", "package activation failed\nmore detail");
        assert!(result.starts_with("APPX_CONTEXT_HELPER_FAILED: PowerShell exited with 1;"));
        assert!(!result.contains('\n'));
    }

    #[test]
    fn protocol_ack_is_bound_to_the_same_one_shot_nonce() {
        let ack = Message::Ack {
            version: PROTOCOL_VERSION,
            nonce: "operation-nonce".into(),
        };
        assert!(matches!(
            ack,
            Message::Ack { version: PROTOCOL_VERSION, nonce }
                if nonce == "operation-nonce"
        ));
    }
}
