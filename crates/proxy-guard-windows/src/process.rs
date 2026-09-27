use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use proxy_guard_core::{
    AumidObservation, BackendProxyConfig, DaemonPreparation, DesktopAppInfo, DesktopProcessState,
    DesktopTargetKind, GuardConfig, LaunchOptions, LaunchReceipt, PackageIdentityObservation,
    PackageRuntimeKind, ProxyDelivery,
};
use sysinfo::System;
use tokio_util::sync::CancellationToken;

use crate::appmodel_activation::{
    ActivationWorkerRequest, ObservationKind, PROTOCOL_VERSION, activate_registered_desktop,
};
use crate::codex_daemon::{CodexCli, DaemonStopBudget, resolve_codex_cli, stop_codex_daemon};
use crate::environment::{apply_proxy_environment, proxy_environment};
use crate::package_identity::windows_paths_equal;
use crate::proxy_env_file::ProxyEnvValues;
use crate::proxy_launch_plan::proxy_launch_plan;

struct StartupLock {
    _file: File,
}

impl StartupLock {
    fn acquire(busy_window: Duration) -> Result<Self, String> {
        let path = std::env::temp_dir().join("codex-proxy-guard-startup.lock");
        Self::acquire_at(&path, busy_window)
    }

    fn acquire_at(path: &Path, busy_window: Duration) -> Result<Self, String> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let mut lock = options
            .open(path)
            .map(|file| Self { _file: file })
            .map_err(|_| {
                "LAUNCH_BUSY: another Codex Proxy Guard instance is currently launching Desktop"
                    .to_string()
            })?;
        let mut text = String::new();
        let _ = lock._file.read_to_string(&mut text);
        let now = now_unix_ms();
        let window_ms = busy_window.as_millis() as u64;
        if text
            .trim()
            .parse::<u64>()
            .is_ok_and(|last| last <= now && now - last < window_ms)
        {
            return Err(
                "LAUNCH_BUSY: another Codex Proxy Guard instance just launched Desktop; refresh and retry"
                    .into(),
            );
        }
        Ok(lock)
    }

    fn mark_spawned(&mut self) {
        if self._file.rewind().is_ok() && self._file.set_len(0).is_ok() {
            let _ = write!(self._file, "{}", now_unix_ms());
            let _ = self._file.flush();
        }
    }
}

pub fn desktop_process_state(info: &DesktopAppInfo) -> DesktopProcessState {
    let mut system = System::new_all();
    system.refresh_all();
    let mut saw_unconfirmed_target = false;
    for (pid, process) in system.processes() {
        if process.start_time() == 0 {
            continue;
        }
        if process.cmd().iter().any(|part| {
            part.to_string_lossy()
                .to_ascii_lowercase()
                .contains("--type=")
        }) {
            continue;
        }
        match classify_process(process.exe(), process.name(), &info.executable) {
            CandidateVerdict::Target => {
                return DesktopProcessState::Running { pid: pid.as_u32() };
            }
            // A likely target whose executable path cannot be read must not be
            // reported as "stopped"; unrelated unreadable processes are ignored.
            CandidateVerdict::UnconfirmedTarget => saw_unconfirmed_target = true,
            CandidateVerdict::Unrelated => {}
        }
    }
    if saw_unconfirmed_target {
        DesktopProcessState::Unknown
    } else {
        DesktopProcessState::Stopped
    }
}

enum CandidateVerdict {
    Target,
    UnconfirmedTarget,
    Unrelated,
}

fn classify_process(
    exe: Option<&Path>,
    name: &OsStr,
    target_executable: &Path,
) -> CandidateVerdict {
    match exe {
        Some(exe) if windows_paths_equal(exe, target_executable) => CandidateVerdict::Target,
        Some(_) => CandidateVerdict::Unrelated,
        None => {
            let matches_by_name = target_executable
                .file_name()
                .is_some_and(|target| name.eq_ignore_ascii_case(target));
            if matches_by_name {
                CandidateVerdict::UnconfirmedTarget
            } else {
                CandidateVerdict::Unrelated
            }
        }
    }
}

/// Injection points for tests: the CLI resolver can be faked and the stop
/// budget shortened without waiting for production durations.
pub struct LaunchHooks<'a> {
    pub resolve_cli: &'a (dyn Fn(&GuardConfig) -> Result<Option<CodexCli>, String> + Sync),
    pub stop_budget: DaemonStopBudget,
    /// How long a freshly recorded spawn keeps blocking other Guard launches.
    /// Production uses 5 s; tests shrink it so serialized launches do not trip
    /// the anti-race window.
    pub post_spawn_busy_window: Duration,
}

impl Default for LaunchHooks<'_> {
    fn default() -> Self {
        Self {
            resolve_cli: &resolve_codex_cli,
            stop_budget: DaemonStopBudget::default(),
            post_spawn_busy_window: Duration::from_secs(5),
        }
    }
}

/// The consented Codex Home scope for the backend proxy block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendProxyScope {
    /// The user has not authorized Guard to manage the `.env` block. The
    /// registered launch still proceeds with Chromium arguments only, and the
    /// receipt reports the missing layer instead of silently claiming it.
    NotAuthorized,
    /// The explicitly confirmed absolute Codex Home bound to the consent.
    Confirmed(PathBuf),
}

/// Pure scope decision shared by the launch and consent paths. An enabled
/// consent without a usable bound home is `BACKEND_PROXY_SCOPE_UNCONFIRMED`:
/// Guard never guesses (and never derives this from its own CODEX_HOME).
pub fn backend_proxy_scope(config: &GuardConfig) -> Result<BackendProxyScope, String> {
    if !config.codex.manage_codex_proxy_env {
        return Ok(BackendProxyScope::NotAuthorized);
    }
    let home = &config.codex.proxy_env_home;
    if home.as_os_str().is_empty() || !home.is_absolute() {
        return Err(
            "BACKEND_PROXY_SCOPE_UNCONFIRMED: the backend proxy consent is enabled but no \
             absolute Codex Home is bound; confirm the displayed home (press B in the TUI). \
             Nothing was written"
                .into(),
        );
    }
    Ok(BackendProxyScope::Confirmed(home.clone()))
}

/// Launches Desktop through the backend its target kind requires.
///
/// Registered package applications are activated through the Windows
/// application model (`ActivateApplication`, `AO_NONE`) — never a bare EXE
/// spawn inside WindowsApps. Ordinary executables are created as a new
/// process with the proxy environment injected.
///
/// Normal launches are side-effect free with respect to Codex: the CLI is not
/// resolved and no daemon command runs. The explicit repair launch
/// (`LaunchOptions::refresh_codex_daemon`) additionally stops the shared
/// daemon through the official lifecycle command after the user's one-shot
/// confirmation.
///
/// Cancellation contract: once cancellation has been observed before the
/// activation request is submitted, no request is made; after submission the
/// outcome is reported as unknown and never retried. After a spawn or
/// activation, Guard never kills the Desktop. Guard never terminates
/// Desktop, the daemon, or any process it did not create (its own short-lived
/// activation worker being the only exception, on timeout or cancellation).
pub async fn launch_codex(
    info: &DesktopAppInfo,
    config: &GuardConfig,
    options: LaunchOptions,
    cancellation: &CancellationToken,
) -> Result<LaunchReceipt, String> {
    launch_codex_with(info, config, options, cancellation, &LaunchHooks::default()).await
}

pub async fn launch_codex_with(
    info: &DesktopAppInfo,
    config: &GuardConfig,
    options: LaunchOptions,
    cancellation: &CancellationToken,
    hooks: &LaunchHooks<'_>,
) -> Result<LaunchReceipt, String> {
    config.validate().map_err(|error| error.to_string())?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }
    crate::elevation::ensure_non_elevated()?;
    let mut lock = StartupLock::acquire(hooks.post_spawn_busy_window)?;
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }

    // The repair path always refuses an already-running Desktop; a normal
    // launch may allow it only when explicitly configured. A registered
    // application must never be activated on top of a live instance — a
    // single-instance shell would simply ignore the new proxy arguments.
    let refuse_if_running = matches!(&info.target_kind, DesktopTargetKind::RegisteredPackage(_))
        || options.refresh_codex_daemon
        || config.codex.refuse_if_running;
    if refuse_if_running {
        match desktop_process_state(info) {
            DesktopProcessState::Running { .. } => {
                return Err(
                    "CODEX_ALREADY_RUNNING: Desktop is already running; fully exit it before launching through Guard"
                        .into(),
                );
            }
            DesktopProcessState::Unknown => {
                return Err(
                    "CODEX_RUNNING_UNKNOWN: cannot confirm whether Desktop is running; it was not launched"
                        .into(),
                );
            }
            DesktopProcessState::Stopped => {}
        }
    }
    if !info.executable.is_file() {
        return Err(format!(
            "CODEX_EXECUTABLE_MISSING: {} no longer exists; refresh and retry",
            info.executable.display()
        ));
    }

    // Reject an unusable registered-application identity before any side
    // effect. A registered Desktop must never fall through to a naked EXE
    // spawn.
    match &info.target_kind {
        DesktopTargetKind::RegisteredPackage(package) => {
            if package.runtime_kind != PackageRuntimeKind::FullTrustDesktop
                || package.package_full_name.is_empty()
                || package.package_family_name.is_empty()
                || package.application_id.is_empty()
                || package.manifest_executable.is_empty()
                || package.app_user_model_id
                    != format!("{}!{}", package.package_family_name, package.application_id)
            {
                return Err("APPX_METADATA_INCOMPLETE: registered Desktop package has no verified FullTrust application identity".into());
            }
            // The repair path spends a long, cancellable window in the daemon
            // stop; re-read the registration before that window instead of
            // launching a stale entry. The normal path has no await between
            // its fresh pipeline discovery and the activation submit, so the
            // pipeline discovery is already the current registration.
            if options.refresh_codex_daemon {
                let current = crate::appx::discover_desktop_app(config, None, cancellation).await?;
                if current.target_kind != info.target_kind || current.executable != info.executable
                {
                    return Err(
                        "APPX_PACKAGE_CHANGED: Desktop package changed before launch; refresh and retry"
                            .into(),
                    );
                }
            }
        }
        DesktopTargetKind::UnpackagedExecutable => {}
    }

    let mut pinned_home = None;
    let daemon_preparation = if options.refresh_codex_daemon {
        if cancellation.is_cancelled() {
            return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
        }
        let cli = (hooks.resolve_cli)(config)?;
        let Some(cli) = cli else {
            return Err(
                "CODEX_CLI_UNAVAILABLE: no official Codex CLI was found for the repair launch; \
                 start a normal launch instead, or set codex.cli_executable_override / CODEX_HOME"
                    .into(),
            );
        };
        if cancellation.is_cancelled() {
            return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
        }
        let preparation = stop_codex_daemon(&cli, &hooks.stop_budget, cancellation).await?;
        pinned_home = cli.pinned_home;
        // The stop may take a while: re-verify everything that was checked
        // before the wait window, still under the startup lock.
        if cancellation.is_cancelled() {
            return Err(match preparation {
                DaemonPreparation::Stopped => {
                    "LAUNCH_CANCELLED: preparation cancelled after the shared Codex background \
                     server was stopped; Desktop was not started"
                        .into()
                }
                _ => "LAUNCH_CANCELLED: Guard is shutting down".into(),
            });
        }
        if !info.executable.is_file() {
            return Err(format!(
                "CODEX_EXECUTABLE_MISSING: {} disappeared while preparing the launch; refresh and retry",
                info.executable.display()
            ));
        }
        if matches!(
            desktop_process_state(info),
            DesktopProcessState::Running { .. } | DesktopProcessState::Unknown
        ) {
            return Err(
                "CODEX_ALREADY_RUNNING: Desktop appeared to be running while preparing the \
                 launch; no second instance was started"
                    .into(),
            );
        }
        preparation
    } else {
        DaemonPreparation::Skipped
    };

    if matches!(&info.target_kind, DesktopTargetKind::RegisteredPackage(_)) {
        if options.refresh_codex_daemon {
            let current = crate::appx::discover_desktop_app(config, None, cancellation).await?;
            if current.target_kind != info.target_kind || current.executable != info.executable {
                return Err("APPX_PACKAGE_CHANGED: Desktop package changed during preparation; refresh and retry".into());
            }
        }
        if !matches!(desktop_process_state(info), DesktopProcessState::Stopped) {
            return Err("CODEX_ALREADY_RUNNING: Desktop appeared before package launch; no second instance was started".into());
        }
    }

    // Final cancellation checkpoint. No `await` is allowed between here and
    // submitting the native spawn or activation request; each backend owns
    // its later permit boundary.
    if cancellation.is_cancelled() {
        return Err("LAUNCH_CANCELLED: Guard is shutting down".into());
    }

    match &info.target_kind {
        DesktopTargetKind::RegisteredPackage(package) => {
            launch_registered(
                info,
                package,
                config,
                options,
                daemon_preparation,
                cancellation,
                &mut lock,
            )
            .await
        }
        DesktopTargetKind::UnpackagedExecutable => {
            let environment = proxy_environment(config);
            let mut command = Command::new(&info.executable);
            apply_proxy_environment(&mut command, &environment);
            if let Some(home) = &pinned_home {
                command.env("CODEX_HOME", home);
            }
            let child = command.spawn().map_err(|error| {
                daemon_failure_context(
                    format!("CODEX_LAUNCH_FAILED: Desktop could not be started: {error}"),
                    daemon_preparation,
                )
            })?;
            lock.mark_spawned();
            let target_elevation = query_child_elevation(&child);
            Ok(LaunchReceipt {
                pid: child.id(),
                proxy_endpoint: Some(environment.proxy_url),
                daemon_preparation,
                launch_method: proxy_guard_core::LaunchMethod::NativeProcess,
                activation_state: proxy_guard_core::ActivationState::NotSubmitted,
                instance: proxy_guard_core::InstanceObservation::Created,
                package_identity: PackageIdentityObservation::NotApplicable,
                aumid: AumidObservation::NotApplicable,
                proxy_delivery: if options.activation_only {
                    ProxyDelivery::NotEstablished
                } else {
                    ProxyDelivery::ProcessEnvironment
                },
                backend_proxy_config: BackendProxyConfig::NotApplicable,
                target_elevation,
                desktop: info.into(),
            })
        }
    }
}

/// Registered-application launch through the native AppModel activation
/// backend, with the proxy plan and the authorized home configuration
/// prepared honestly and reported as separate layers.
async fn launch_registered(
    info: &DesktopAppInfo,
    package: &proxy_guard_core::PackageApplication,
    config: &GuardConfig,
    options: LaunchOptions,
    daemon_preparation: DaemonPreparation,
    cancellation: &CancellationToken,
    lock: &mut StartupLock,
) -> Result<LaunchReceipt, String> {
    let mut proxy_endpoint = None;
    let mut arguments = String::new();
    let mut proxy_delivery = ProxyDelivery::NotEstablished;
    let mut backend_proxy_config = BackendProxyConfig::NotApplicable;

    if options.activation_only {
        // Diagnostic identity path: no proxy arguments, no home configuration,
        // and a receipt that says so.
    } else {
        let plan = proxy_launch_plan(config)?;
        arguments = plan.chromium_arguments;
        proxy_endpoint = Some(plan.proxy_url);
        match backend_proxy_scope(config)? {
            BackendProxyScope::NotAuthorized => {
                proxy_delivery = ProxyDelivery::ActivationArguments;
                backend_proxy_config = BackendProxyConfig::NotAuthorized;
            }
            BackendProxyScope::Confirmed(home) => {
                let path = crate::proxy_env_file::env_path(&home);
                let values = ProxyEnvValues::from_config(config);
                // Preparation failure aborts the launch: a "proxied launch"
                // must not silently degrade into an unproxied activation. The
                // block persists after this launch by design; revocation is
                // the user's explicit action.
                tokio::task::spawn_blocking(move || crate::proxy_env_file::prepare(&path, &values))
                    .await
                    .map_err(|error| format!("BACKEND_PROXY_ENV_TASK_FAILED: {error}"))?
                    .map_err(|error| {
                        daemon_failure_context(
                            format!("{error}; the activation was not attempted"),
                            daemon_preparation,
                        )
                    })?;
                proxy_delivery = ProxyDelivery::ActivationArgumentsAndHomeConfig;
                backend_proxy_config = BackendProxyConfig::Prepared;
            }
        }
    }

    let request = ActivationWorkerRequest {
        version: PROTOCOL_VERSION,
        aumid: package.app_user_model_id.clone(),
        expected_package_full_name: package.package_full_name.clone(),
        expected_executable: info.executable.clone(),
        arguments,
    };

    // The backend owns its cancellation/permit boundary. Mark the anti-race
    // window before entering it: an outcome-unknown result must not invite an
    // immediate duplicate launch.
    lock.mark_spawned();
    let outcome = activate_registered_desktop(&request, cancellation)
        .await
        .map_err(|error| daemon_failure_context(error, daemon_preparation))?;
    let receipt = outcome.receipt;

    if let Some(failure) = &receipt.failure {
        return Err(daemon_failure_context(failure.clone(), daemon_preparation));
    }
    if let Some(hresult) = &receipt.activation_hresult {
        return Err(daemon_failure_context(
            format!(
                "APPX_ACTIVATION_FAILED: ActivateApplication returned {hresult} for {}; no \
                 fallback launch was attempted",
                package.app_user_model_id
            ),
            daemon_preparation,
        ));
    }
    if !receipt.activation_returned {
        return Err(daemon_failure_context(
            "APPX_ACTIVATION_OUTCOME_UNKNOWN: the activation call produced no result; do not \
             retry automatically — refresh the Desktop state instead"
                .into(),
            daemon_preparation,
        ));
    }
    let pid = receipt.pid.unwrap_or(0);
    if pid == 0 {
        return Err(daemon_failure_context(
            "APPX_ACTIVATION_OUTCOME_UNKNOWN: the activation returned without a target PID".into(),
            daemon_preparation,
        ));
    }
    let package_identity = match receipt.package_identity {
        ObservationKind::Matched => PackageIdentityObservation::Matched,
        ObservationKind::Missing => {
            return Err(daemon_failure_context(
                format!(
                    "APPX_IDENTITY_MISSING: the activated target PID {pid} has no package \
                     identity; the application may already have been created"
                ),
                daemon_preparation,
            ));
        }
        ObservationKind::Mismatch => {
            return Err(daemon_failure_context(
                format!(
                    "APPX_IDENTITY_MISMATCH: the activated target PID {pid} has package {}, \
                     expected {}; Desktop may already have been created",
                    receipt
                        .observed_package_full_name
                        .as_deref()
                        .unwrap_or("<unavailable>"),
                    package.package_full_name
                ),
                daemon_preparation,
            ));
        }
        ObservationKind::QueryFailed => {
            return Err(daemon_failure_context(
                format!(
                    "APPX_IDENTITY_QUERY_FAILED: the identity of target PID {pid} could not be \
                     queried{}; Desktop may already have been created",
                    detail_suffix(&receipt)
                ),
                daemon_preparation,
            ));
        }
        ObservationKind::NotQueried => {
            return Err(daemon_failure_context(
                "APPX_ACTIVATION_OUTCOME_UNKNOWN: the target identity was never observed".into(),
                daemon_preparation,
            ));
        }
    };
    let aumid = match receipt.aumid {
        crate::appmodel_activation::AumidKind::Matched => AumidObservation::Matched,
        crate::appmodel_activation::AumidKind::Missing => {
            return Err(daemon_failure_context(
                format!(
                    "APPX_AUMID_MISSING: the activated target PID {pid} has no application \
                     identity (started outside its registered entry); Desktop may already have \
                     been created"
                ),
                daemon_preparation,
            ));
        }
        crate::appmodel_activation::AumidKind::Mismatch => {
            return Err(daemon_failure_context(
                format!(
                    "APPX_AUMID_MISMATCH: the activated target PID {pid} has AUMID {}, expected \
                     {}; Desktop may already have been created",
                    receipt.observed_aumid.as_deref().unwrap_or("<unavailable>"),
                    package.app_user_model_id
                ),
                daemon_preparation,
            ));
        }
        crate::appmodel_activation::AumidKind::QueryFailed => {
            return Err(daemon_failure_context(
                format!(
                    "APPX_AUMID_QUERY_FAILED: the application identity of target PID {pid} \
                     could not be queried{}; Desktop may already have been created",
                    detail_suffix(&receipt)
                ),
                daemon_preparation,
            ));
        }
        crate::appmodel_activation::AumidKind::NotQueried => {
            return Err(daemon_failure_context(
                "APPX_ACTIVATION_OUTCOME_UNKNOWN: the target application identity was never \
                 observed"
                    .into(),
                daemon_preparation,
            ));
        }
    };
    if let Some(code) = receipt.early_exit_code {
        return Err(daemon_failure_context(
            format!(
                "APPX_TARGET_EXITED_EARLY: the activated target PID {pid} exited during the \
                 observation window with exit code {code}"
            ),
            daemon_preparation,
        ));
    }
    if let Some(failure) = &receipt.observation_failure {
        return Err(daemon_failure_context(
            format!(
                "{failure}; the activation returned PID {pid} but its observations are not clean"
            ),
            daemon_preparation,
        ));
    }
    let instance = match receipt.instance {
        crate::appmodel_activation::InstanceKind::Created => {
            proxy_guard_core::InstanceObservation::Created
        }
        crate::appmodel_activation::InstanceKind::Reused => {
            proxy_guard_core::InstanceObservation::Reused
        }
        crate::appmodel_activation::InstanceKind::Unknown => {
            proxy_guard_core::InstanceObservation::Unknown
        }
    };

    Ok(LaunchReceipt {
        pid,
        proxy_endpoint,
        daemon_preparation,
        launch_method: proxy_guard_core::LaunchMethod::AppmodelActivation,
        activation_state: proxy_guard_core::ActivationState::Returned,
        instance,
        package_identity,
        aumid,
        proxy_delivery,
        backend_proxy_config,
        target_elevation: receipt.elevation,
        desktop: info.into(),
    })
}

fn detail_suffix(receipt: &crate::appmodel_activation::ActivationWorkerReceipt) -> String {
    match &receipt.observation_failure {
        Some(failure) => format!(" ({failure})"),
        None => String::new(),
    }
}

#[cfg(windows)]
fn query_child_elevation(child: &std::process::Child) -> Option<bool> {
    crate::package_identity::query_process_elevation(child).ok()
}

#[cfg(not(windows))]
fn query_child_elevation(_child: &std::process::Child) -> Option<bool> {
    None
}

fn daemon_failure_context(error: String, preparation: DaemonPreparation) -> String {
    if preparation == DaemonPreparation::Stopped {
        format!("{error}; note: the shared Codex background server was stopped before this failure")
    } else {
        error
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_locations_do_not_match_by_file_name() {
        assert!(!windows_paths_equal(
            Path::new(r"C:\Apps\ChatGPT\ChatGPT.exe"),
            Path::new(r"D:\Other\ChatGPT.exe"),
        ));
        assert!(!windows_paths_equal(
            Path::new(r"\\?\C:\Apps\ChatGPT.exe"),
            Path::new(r"\\?\UNC\host\C$\Apps\ChatGPT.exe"),
        ));
    }

    #[test]
    fn classify_processes_by_path_then_name() {
        let target = Path::new(r"\\?\C:\Apps\OpenAI\ChatGPT.exe");
        assert!(matches!(
            classify_process(
                Some(Path::new(r"C:\Apps\OpenAI\ChatGPT.exe")),
                OsStr::new("other.exe"),
                target
            ),
            CandidateVerdict::Target
        ));
        assert!(matches!(
            classify_process(
                Some(Path::new(r"C:\Elsewhere\codex.exe")),
                OsStr::new("codex.exe"),
                target
            ),
            CandidateVerdict::Unrelated
        ));
        assert!(matches!(
            classify_process(None, OsStr::new("ChatGPT.EXE"), target),
            CandidateVerdict::UnconfirmedTarget
        ));
        assert!(matches!(
            classify_process(None, OsStr::new("unrelated.exe"), target),
            CandidateVerdict::Unrelated
        ));
    }

    #[test]
    fn startup_lock_is_exclusive_on_windows() {
        let path = std::env::temp_dir().join(format!(
            "codex-proxy-guard-lock-test-{}.lock",
            std::process::id()
        ));
        let window = Duration::from_secs(5);
        let first = StartupLock::acquire_at(&path, window).unwrap();
        #[cfg(windows)]
        assert!(StartupLock::acquire_at(&path, window).is_err());
        drop(first);
        assert!(StartupLock::acquire_at(&path, window).is_ok());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn backend_proxy_scope_never_guesses_a_home() {
        let mut config = GuardConfig::default();
        assert_eq!(
            backend_proxy_scope(&config).unwrap(),
            BackendProxyScope::NotAuthorized
        );
        config.codex.manage_codex_proxy_env = true;
        let error = backend_proxy_scope(&config).unwrap_err();
        assert!(
            error.starts_with("BACKEND_PROXY_SCOPE_UNCONFIRMED:"),
            "{error}"
        );
        assert!(error.contains("Nothing was written"));
        config.codex.proxy_env_home = PathBuf::from(r"C:\Users\fixture\.codex");
        assert_eq!(
            backend_proxy_scope(&config).unwrap(),
            BackendProxyScope::Confirmed(PathBuf::from(r"C:\Users\fixture\.codex"))
        );
    }
}
