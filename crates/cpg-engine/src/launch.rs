//! The launch pipeline: proxy argument planning, environment construction,
//! CLI resolution, the daemon-stop helper, the cross-process startup lock
//! and the single authoritative `launch_pipeline`. Port of `launch.cpp`.

use crate::config_store::{self, ConfigLease};
use cpg_domain::Result;
use cpg_domain::config::Config;
use cpg_domain::discovery::Desktop;
use cpg_domain::error::Error;
use cpg_domain::json::{Json, json_dump};
use cpg_domain::proxy::bypass_entry;
use cpg_domain::text::public_text;
use cpg_domain::worker::daemon_stop_status;
use cpg_windows::cancellation::Cancellation;
use cpg_windows::elevation;
use cpg_windows::environment::UnicodeEnvironment;
use cpg_windows::handles::OwnedHandle;
use cpg_windows::{appmodel, fsio, paths, process};
use std::path::{Path, PathBuf};
use std::time::Duration;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    OPEN_ALWAYS,
};
use windows::core::PCWSTR;

pub const DAEMON_OUTPUT_LIMIT: usize = 64 * 1024;
const DAEMON_BUDGET: Duration = Duration::from_secs(720);

#[derive(Clone, Copy, Debug, Default)]
pub struct LaunchOptions {
    pub repair: bool,
    pub activation_only: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ProxyPlan {
    pub endpoint: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Default)]
pub struct CodexCli {
    pub executable: PathBuf,
    pub home: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct LaunchFacts {
    pub activation_state: &'static str,
    pub daemon_preparation: &'static str,
    pub backend_proxy_config: &'static str,
}

impl LaunchFacts {
    pub fn fresh() -> LaunchFacts {
        LaunchFacts {
            activation_state: "not_submitted",
            daemon_preparation: "skipped",
            backend_proxy_config: "unknown",
        }
    }
}

fn fail<T>(code: &str, message: &str) -> Result<T> {
    Err(Error::new(code, message))
}

pub fn proxy_plan(config: &Config) -> Result<ProxyPlan> {
    config.validate()?;
    let mut bypass = String::new();
    for entry in &config.no_proxy {
        if !bypass.is_empty() {
            bypass.push(';');
        }
        bypass.push_str(&bypass_entry(entry)?);
    }
    let endpoint = config.proxy_url();
    if endpoint.len() > 2048 || bypass.len() > 4096 {
        return fail(
            "PROXY_LAUNCH_PLAN_INVALID",
            "Proxy launch arguments exceed the size limit.",
        );
    }
    Ok(ProxyPlan {
        arguments: format!("--proxy-server=\"{endpoint}\" --proxy-bypass-list=\"{bypass}\""),
        endpoint,
    })
}

pub fn proxy_environment(config: &Config) -> Result<UnicodeEnvironment> {
    config.validate()?;
    let mut environment = UnicodeEnvironment::current()?;
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        environment.set(key, &config.proxy_url());
    }
    environment.set("NO_PROXY", &config.no_proxy_value());
    environment.set("no_proxy", &config.no_proxy_value());
    environment.remove("ALL_PROXY");
    environment.remove("all_proxy");
    Ok(environment)
}

fn absolute_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if text.is_empty() || text.contains('\0') {
        return false;
    }
    let bytes = text.as_bytes();
    let drive = text.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\');
    drive || text.starts_with("\\\\") || text.starts_with("//")
}

fn canonical_exe(path: &Path) -> Option<PathBuf> {
    if !absolute_path(path) || !path.is_file() {
        return None;
    }
    let extension = path.extension()?.to_string_lossy();
    if extension.len() != 4 || !extension.eq_ignore_ascii_case(".exe") {
        return None;
    }
    paths::canonical_path_of(path, false)
}

/// Port of `detail::resolveCliFrom` (fixture seam: injectable probes).
pub fn resolve_cli_from(
    explicit_home: &Path,
    user_home: &Path,
    path_text: &str,
    override_path: &Path,
    cwd: &Path,
) -> Result<CodexCli> {
    let mut home = PathBuf::new();
    if !explicit_home.as_os_str().is_empty() {
        let resolved = if absolute_path(explicit_home) {
            explicit_home.to_path_buf()
        } else {
            cwd.join(explicit_home)
        };
        if !resolved.is_dir() {
            return fail(
                "CODEX_HOME_INVALID",
                "Explicit CODEX_HOME must resolve to an existing directory.",
            );
        }
        home = paths::canonical_path_of(&resolved, true).ok_or_else(|| {
            Error::new(
                "CODEX_HOME_INVALID",
                "Explicit CODEX_HOME must resolve to an existing directory.",
            )
        })?;
    } else {
        if user_home.as_os_str().is_empty() {
            return fail(
                "CODEX_HOME_UNRESOLVED",
                "Cannot determine the user Home for Codex CLI resolution.",
            );
        }
        let candidate = user_home.join(".codex");
        if candidate.is_dir()
            && let Some(canonical) = paths::canonical_path_of(&candidate, true)
        {
            home = canonical;
        }
    }
    // Pin every resolved scope so subsequent cwd or environment changes
    // cannot redirect this operation to a different Home.
    if !override_path.as_os_str().is_empty() {
        let executable = canonical_exe(override_path).ok_or_else(|| {
            Error::new(
                "CODEX_CLI_OVERRIDE_INVALID",
                "Codex CLI override must be an existing absolute native .exe, not a shell shim.",
            )
        })?;
        return Ok(CodexCli { executable, home });
    }
    if !home.as_os_str().is_empty() {
        for relative in [
            "packages/app-server-daemon/current/bin/codex.exe",
            "packages/standalone/current/bin/codex.exe",
            "packages/standalone/current/codex.exe",
        ] {
            if let Some(executable) = canonical_exe(&home.join(relative)) {
                return Ok(CodexCli { executable, home });
            }
        }
    }
    let cwd_canonical = paths::canonical_path_of(cwd, true);
    let current = cwd_canonical
        .as_ref()
        .map(|path| paths::normalized_windows_path(path))
        .unwrap_or_default();
    let mut offset = 0usize;
    let bytes = path_text.as_bytes();
    while offset <= bytes.len() {
        let stop = bytes[offset..]
            .iter()
            .position(|b| *b == b';')
            .map(|p| p + offset)
            .unwrap_or(bytes.len());
        let mut entry = &path_text[offset..stop];
        offset = if stop == bytes.len() {
            bytes.len() + 1
        } else {
            stop + 1
        };
        if entry.len() > 1 && entry.starts_with('"') && entry.ends_with('"') {
            entry = &entry[1..entry.len() - 1];
        }
        let directory = PathBuf::from(entry);
        if !absolute_path(&directory) {
            continue;
        }
        let tail = directory.to_string_lossy();
        if tail.ends_with("\\.") || tail.ends_with("/.") {
            continue;
        }
        if !directory.is_dir() {
            continue;
        }
        let Some(canonical) = paths::canonical_path_of(&directory, true) else {
            continue;
        };
        if !current.is_empty() && paths::normalized_windows_path(&canonical) == current {
            continue;
        }
        if let Some(executable) = canonical_exe(&canonical.join("codex.exe")) {
            return Ok(CodexCli { executable, home });
        }
    }
    fail(
        "CODEX_CLI_UNAVAILABLE",
        "No native Codex CLI was found in the controlled locations; use normal launch or configure an absolute CLI override.",
    )
}

pub fn resolve_codex_cli(config: &Config) -> Result<CodexCli> {
    // For registered repair the CLI must operate on the same explicitly
    // authorized Home whose backend block was prepared, never Guard's Home.
    let explicit_environment = std::env::var("CODEX_HOME").ok();
    let explicit_home = if config.manage_backend {
        config.home.clone()
    } else {
        explicit_environment.map(PathBuf::from).unwrap_or_default()
    };
    let user_home = std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(PathBuf::from))
        .unwrap_or_default();
    let path_text = std::env::var("PATH").unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    resolve_cli_from(
        &explicit_home,
        &user_home,
        &path_text,
        &config.cli_override,
        &cwd,
    )
}

/// Port of `StartupLock`: a cross-process 5-second launch spacing lock in
/// the user temp directory. `mark_submitted` is the submission boundary.
struct StartupLock {
    handle: OwnedHandle,
}

impl StartupLock {
    fn open() -> Result<StartupLock> {
        let path = paths::temp_path()?.join("codex-proxy-guard-startup.lock");
        let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
        // SAFETY: null-terminated path; exclusive sharing implements the lock.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ,
                None,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .ok()
        .and_then(OwnedHandle::new);
        let handle = handle.ok_or_else(|| {
            Error::new(
                "LAUNCH_BUSY",
                "Another Guard instance is launching, or the startup lock is unavailable.",
            )
        })?;
        let previous = fsio::read_raw(handle.raw(), 64, "LAUNCH_BUSY")?;
        let text = String::from_utf8_lossy(&previous).trim().to_string();
        if !text.is_empty() && text.len() <= 20 && text.bytes().all(|b| b.is_ascii_digit()) {
            let last: i64 = text.parse().unwrap_or(0);
            let now = milliseconds_since_epoch() as i64;
            if last <= now && now - last < 5000 {
                return fail(
                    "LAUNCH_BUSY",
                    "Another Guard instance just launched Desktop; refresh before retrying.",
                );
            }
        }
        Ok(StartupLock { handle })
    }

    fn mark_submitted(&self) -> Result<()> {
        let timestamp = milliseconds_since_epoch().to_string();
        fsio::write_raw(self.handle.raw(), timestamp.as_bytes())
            .then_some(())
            .ok_or_else(|| {
                Error::new(
                    "LAUNCH_BUSY",
                    "Cannot record the startup guard; Desktop was not submitted.",
                )
            })
    }
}

fn milliseconds_since_epoch() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn require_stopped(desktop: &Desktop) -> Result<()> {
    let state = cpg_windows::process_state::desktop_process_state(desktop);
    match state.state {
        "stopped" => Ok(()),
        "running" => fail(
            "CODEX_ALREADY_RUNNING",
            "Fully exit Desktop before launching it through Guard.",
        ),
        _ => fail(
            "CODEX_RUNNING_UNKNOWN",
            "Cannot confirm whether Desktop is running; nothing was launched.",
        ),
    }
}

fn same_target(first: &Desktop, second: &Desktop) -> bool {
    first.registered == second.registered
        && paths::normalized_windows_path(&first.executable)
            == paths::normalized_windows_path(&second.executable)
        && first.package_full_name == second.package_full_name
        && first.package_family_name == second.package_family_name
        && first.aumid == second.aumid
        && first.application_id == second.application_id
        && first.runtime_kind == second.runtime_kind
        && first.manifest_executable == second.manifest_executable
}

/// Port of `detail::activationReceipt` — worker receipt to public receipt.
pub fn activation_receipt(worker: &Json) -> Result<Json> {
    let failed = |name: &str, code: &str, message: &str| -> Result<()> {
        if !worker.at(name).is_null() {
            return Err(Error::new(code, message));
        }
        Ok(())
    };
    failed(
        "failure",
        "APPX_ACTIVATION_FAILED",
        "The activation worker reported failure; Desktop may already have been created.",
    )?;
    failed(
        "activation_hresult",
        "APPX_ACTIVATION_FAILED",
        "Windows application activation failed; no fallback was attempted.",
    )?;
    let pid_valid = worker.at("pid").as_u64().is_some_and(|pid| pid > 0);
    let returned = worker.at("activation_returned").as_bool().unwrap_or(false);
    if !returned || !pid_valid {
        return fail(
            "APPX_ACTIVATION_OUTCOME_UNKNOWN",
            "Activation produced no confirmed target; refresh Desktop state and do not retry automatically.",
        );
    }
    for name in ["package_identity", "aumid"] {
        let observation = worker.at(name).as_str().unwrap_or_default();
        if observation != "matched" {
            let prefix = if name == "aumid" {
                "APPX_AUMID_"
            } else {
                "APPX_IDENTITY_"
            };
            let suffix = match observation {
                "missing" => "MISSING",
                "mismatch" => "MISMATCH",
                _ => "QUERY_FAILED",
            };
            return Err(Error::new(
                format!("{prefix}{suffix}").as_str(),
                "Activated Desktop identity could not be confirmed; the application may already exist.",
            ));
        }
    }
    failed(
        "early_exit_code",
        "APPX_TARGET_EXITED_EARLY",
        "The activated Desktop exited during observation.",
    )?;
    failed(
        "observation_failure",
        "APPX_ACTIVATION_OUTCOME_UNKNOWN",
        "Activated Desktop observations were incomplete; do not retry automatically.",
    )?;
    let elevated = worker.at("elevation");
    match elevated {
        Json::Bool(false) => {}
        Json::Bool(true) => {
            return fail(
                "APPX_TARGET_ELEVATED",
                "Activated Desktop is elevated; normal-user launch could not be verified.",
            );
        }
        _ => {
            return fail(
                "APPX_TARGET_ELEVATION_QUERY_FAILED",
                "Activated Desktop elevation could not be verified; do not retry automatically.",
            );
        }
    }
    Ok(Json::object_from([
        ("pid", worker.at("pid").clone()),
        ("launch_method", Json::from("appmodel_activation")),
        ("activation_state", Json::from("returned")),
        ("instance", worker.at("instance").clone()),
        ("package_identity", Json::from("matched")),
        ("aumid", Json::from("matched")),
        ("target_elevation", elevated.clone()),
    ]))
}

pub fn stop_codex_daemon(cli: &CodexCli, cancel: &Cancellation) -> Result<&'static str> {
    cancel.check()?;
    if cli.executable.as_os_str().is_empty() || canonical_exe(&cli.executable).is_none() {
        return fail(
            "CODEX_DAEMON_STOP_FAILED",
            "Resolved Codex CLI or operation budget is invalid.",
        );
    }
    let mut environment = UnicodeEnvironment::current()?;
    if !cli.home.as_os_str().is_empty() {
        environment.set("CODEX_HOME", &cli.home.to_string_lossy());
    }
    let result = process::run_helper(
        &cli.executable,
        &[
            "app-server".to_owned(),
            "daemon".to_owned(),
            "stop".to_owned(),
        ],
        b"",
        cancel,
        DAEMON_BUDGET,
        DAEMON_OUTPUT_LIMIT,
        &mut environment,
        false,
    );
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            if cancel.cancelled() {
                return fail(
                    "LAUNCH_CANCELLED",
                    "Repair cancelled; the shared background server state is unconfirmed.",
                );
            }
            if error.code.contains("TIMEOUT") {
                return fail(
                    "CODEX_DAEMON_STOP_TIMEOUT",
                    "Repair exceeded its budget; Desktop was not started and shared service state is unconfirmed.",
                );
            }
            return fail(
                "CODEX_DAEMON_STOP_FAILED",
                "The public daemon-stop helper failed; shared service state is unconfirmed.",
            );
        }
    };
    let out = String::from_utf8_lossy(&result.out);
    let err = String::from_utf8_lossy(&result.err);
    if result.out.len() > DAEMON_OUTPUT_LIMIT
        || result.err.len() > DAEMON_OUTPUT_LIMIT
        || !cpg_domain::text::strict_utf8(&result.out)
        || !cpg_domain::text::strict_utf8(&result.err)
    {
        return fail(
            "CODEX_DAEMON_STOP_FAILED",
            "Codex CLI output exceeded its limit or was not valid UTF-8.",
        );
    }
    if result.exit_code != 0 {
        let combined = format!("{}\n{}", out, err).to_lowercase();
        if combined.contains("unrecognized subcommand") || combined.contains("unknown command") {
            return fail(
                "CODEX_DAEMON_UNSUPPORTED",
                "This Codex CLI does not support the public daemon lifecycle command; use normal launch.",
            );
        }
        return fail(
            "CODEX_DAEMON_STOP_FAILED",
            "The public Codex daemon-stop command failed; Desktop was not started.",
        );
    }
    daemon_stop_status(&out).ok_or_else(|| {
        Error::new(
            "CODEX_DAEMON_STOP_FAILED",
            "Unexpected daemon lifecycle JSON; shared service state is unconfirmed.",
        )
    })
}

/// Spawns the same-EXE activation worker over bounded stdio. Port of
/// `activateDesktop`.
pub fn activate_desktop(desktop: &Desktop, arguments: &str, cancel: &Cancellation) -> Result<Json> {
    if !desktop.registered {
        return fail(
            "APPX_ACTIVATION_PROTOCOL_INVALID",
            "Application activation requires a registered target",
        );
    }
    elevation::require_not_elevated()?;
    let request = Json::object_from([
        ("version", Json::Int(1)),
        ("aumid", Json::from(desktop.aumid.as_str())),
        (
            "expected_package_full_name",
            Json::from(desktop.package_full_name.as_str()),
        ),
        (
            "expected_executable",
            Json::from(desktop.executable.to_string_lossy().as_ref()),
        ),
        ("arguments", Json::from(arguments)),
    ]);
    let bytes = json_dump(&request);
    cpg_domain::worker::validate_activation_request(bytes.as_bytes())?;
    let mut environment = UnicodeEnvironment::current()?;
    let result = process::run_helper(
        &cpg_windows::paths::current_executable_path()?,
        &["internal-activate-package".to_owned()],
        bytes.as_bytes(),
        cancel,
        Duration::from_secs(30),
        cpg_domain::worker::RECEIPT_LIMIT,
        &mut environment,
        true,
    );
    let receipt = match result {
        Ok(result) if result.exit_code == 0 => {
            cpg_domain::worker::validate_activation_receipt(&result.out).map_err(|_| {
                Error::new(
                    "APPX_ACTIVATION_OUTCOME_UNKNOWN",
                    "Activation request was submitted but the receipt is unavailable or invalid; refresh Desktop state",
                )
            })
        }
        Ok(_) | Err(_) => {
            return fail(
                "APPX_ACTIVATION_OUTCOME_UNKNOWN",
                "Activation request was submitted but the receipt is unavailable or invalid; refresh Desktop state",
            )
        }
    }?;
    if !receipt.at("failure").is_null() {
        let message = receipt.string_at("failure");
        return Err(Error::new(
            "APPX_ACTIVATION_WORKER_FAILED",
            public_text(&message, 4096),
        ));
    }
    Ok(receipt)
}

fn native_launch(
    desktop: &Desktop,
    environment: &mut UnicodeEnvironment,
    cancel: &Cancellation,
) -> Result<Json> {
    if desktop.registered {
        return fail(
            "APPX_METADATA_INCOMPLETE",
            "Registered applications require application-model activation.",
        );
    }
    let executable = desktop.executable.to_string_lossy().to_string();
    if !absolute_path(&desktop.executable)
        || executable.contains('"')
        || !desktop.executable.is_file()
    {
        return fail(
            "CODEX_EXECUTABLE_MISSING",
            "Desktop executable is missing or invalid.",
        );
    }
    cancel.check()?;
    let mut command_line: Vec<u16> = Vec::new();
    process::append_quoted(
        &mut command_line,
        &executable.encode_utf16().collect::<Vec<u16>>(),
    );
    command_line.push(0);
    let program_wide: Vec<u16> = executable.encode_utf16().chain([0]).collect();
    let mut information = windows::Win32::System::Threading::PROCESS_INFORMATION::default();
    // SAFETY: detached desktop launch through the Unicode environment block.
    // The returned handles are observations only; cancellation and Guard
    // shutdown must never terminate the newly created Desktop.
    let created = unsafe {
        let startup = windows::Win32::System::Threading::STARTUPINFOW {
            cb: std::mem::size_of::<windows::Win32::System::Threading::STARTUPINFOW>() as u32,
            ..windows::Win32::System::Threading::STARTUPINFOW::default()
        };
        let flags = windows::Win32::System::Threading::CREATE_UNICODE_ENVIRONMENT.0
            | windows::Win32::System::Threading::DETACHED_PROCESS.0
            | windows::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP.0;
        windows::Win32::System::Threading::CreateProcessW(
            PCWSTR(program_wide.as_ptr()),
            Some(windows::core::PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            windows::Win32::System::Threading::PROCESS_CREATION_FLAGS(flags),
            Some(environment.pcwstr_block().as_ptr().cast()),
            PCWSTR::null(),
            &startup,
            &mut information,
        )
        .is_ok()
    };
    if !created {
        return fail(
            "CODEX_LAUNCH_FAILED",
            "Windows could not create the Desktop process.",
        );
    }
    let _observation_thread = OwnedHandle::new(information.hThread);
    let observation_process = OwnedHandle::new(information.hProcess);
    let target_elevation = observation_process
        .as_ref()
        .and_then(|handle| cpg_windows::elevation::process_elevated(handle.raw()))
        .map(Json::from)
        .unwrap_or(Json::Null);
    drop(observation_process);
    Ok(Json::object_from([
        ("pid", Json::from(u64::from(information.dwProcessId))),
        ("launch_method", Json::from("native_process")),
        ("activation_state", Json::from("not_submitted")),
        ("instance", Json::from("created")),
        ("package_identity", Json::from("not_applicable")),
        ("aumid", Json::from("not_applicable")),
        ("target_elevation", target_elevation),
    ]))
}

fn discover_desktop(config: &Config, cancel: &Cancellation) -> Result<Desktop> {
    cancel.check()?;
    // Native AppModel discovery only: no PowerShell child, no fallback to it.
    let envelope = appmodel::discover_packages_native(cancel)?;
    let json = json_dump(&envelope);
    cpg_domain::discovery::parse_discovery(
        &json,
        &config.executable_override,
        &cpg_windows::paths::NativeFsProbe,
    )
}

/// The checks that need no file, handle or process. Running them before the
/// configuration lease and the cross-process startup guard means a
/// deterministic refusal leaves no trace at all; the bypass-expressibility
/// refusal cannot move here, because only a discovered target says whether
/// activation arguments are required at all.
fn preflight(config: &Config, options: &LaunchOptions) -> Result<()> {
    if options.repair && options.activation_only {
        return fail(
            "INVALID_LAUNCH_OPTIONS",
            "Activation-only cannot be combined with daemon repair.",
        );
    }
    config.validate()
}

/// The single authoritative pipeline used by CLI, plain console and TUI.
pub fn launch_pipeline(
    config: &Config,
    config_path: &Path,
    options: LaunchOptions,
    cancel: &Cancellation,
    facts: Option<&mut LaunchFacts>,
) -> Result<Json> {
    cancel.check()?;
    preflight(config, &options)?;
    let _lease = ConfigLease::new(config_path, config)?;
    elevation::require_not_elevated()?;
    let startup = StartupLock::open()?;
    let mut local_facts = LaunchFacts::fresh();
    let facts = facts.unwrap_or(&mut local_facts);
    launch_with(
        config,
        options,
        cancel,
        &LaunchServices {
            config_path,
            discover: &|config, cancel| discover_desktop(config, cancel),
            prepare: &|config| config_store::prepare_proxy_env(config),
            resolve: &resolve_codex_cli,
            stop: &|cli, cancel| stop_codex_daemon(cli, cancel),
            activate: &|desktop, arguments, cancel| activate_desktop(desktop, arguments, cancel),
            mark_submitted: &|| startup.mark_submitted(),
        },
        facts,
    )
}

/// Injectable service bundle so integration tests use the same orchestration
/// without real Desktop/Home/daemon side effects.
pub struct LaunchServices<'a> {
    pub config_path: &'a Path,
    pub discover: &'a dyn Fn(&Config, &Cancellation) -> Result<Desktop>,
    pub prepare: &'a dyn Fn(&Config) -> Result<()>,
    pub resolve: &'a dyn Fn(&Config) -> Result<CodexCli>,
    pub stop: &'a dyn Fn(&CodexCli, &Cancellation) -> Result<&'static str>,
    pub activate: &'a dyn Fn(&Desktop, &str, &Cancellation) -> Result<Json>,
    /// Submission boundary (the C++ `markSubmitted` seam); the production
    /// pipeline records the cross-process startup guard here.
    pub mark_submitted: &'a dyn Fn() -> Result<()>,
}

pub fn launch_with(
    config: &Config,
    options: LaunchOptions,
    cancel: &Cancellation,
    services: &LaunchServices<'_>,
    facts: &mut LaunchFacts,
) -> Result<Json> {
    *facts = LaunchFacts::fresh();
    preflight(config, &options)?;
    cancel.check()?;
    elevation::require_not_elevated()?;
    let desktop = (services.discover)(config, cancel)?;
    cancel.check()?;
    require_stopped(&desktop)?;
    if options.activation_only && !desktop.registered {
        return fail(
            "ACTIVATION_ONLY_UNSUPPORTED",
            "Activation-only requires a registered Desktop application.",
        );
    }
    if desktop.registered
        && (desktop.package_full_name.is_empty()
            || desktop.package_family_name.is_empty()
            || desktop.application_id.is_empty()
            || desktop.manifest_executable.is_empty()
            || desktop.aumid
                != format!("{}!{}", desktop.package_family_name, desktop.application_id)
            || desktop.runtime_kind != "full_trust_desktop")
    {
        return fail(
            "APPX_METADATA_INCOMPLETE",
            "Desktop has no verified FullTrust application identity.",
        );
    }

    let mut plan = ProxyPlan::default();
    let mut backend = "not_applicable";
    if !options.activation_only {
        plan = if desktop.registered {
            proxy_plan(config)?
        } else {
            ProxyPlan {
                endpoint: config.proxy_url(),
                arguments: String::new(),
            }
        };
        if desktop.registered {
            if !config.manage_backend {
                if options.repair {
                    return fail(
                        "BACKEND_PROXY_REQUIRED_FOR_REPAIR",
                        "Authorize the Codex backend proxy block before a registered repair launch; the daemon was not touched.",
                    );
                }
                backend = "not_authorized";
            } else {
                cancel.check()?;
                (services.prepare)(config)?;
                backend = "prepared";
            }
        }
    }
    facts.backend_proxy_config = backend;
    let mut daemon_stopped = false;
    let mut cli = CodexCli::default();
    let outcome = (|| -> Result<Json> {
        if options.repair {
            cancel.check()?;
            if !same_target(&desktop, &(services.discover)(config, cancel)?) {
                return fail(
                    "APPX_PACKAGE_CHANGED",
                    "Desktop changed before repair; refresh and retry.",
                );
            }
            require_stopped(&desktop)?;
            cli = (services.resolve)(config)?;
            cancel.check()?;
            facts.daemon_preparation = "unconfirmed";
            let daemon = (services.stop)(&cli, cancel)?; // exactly once, after preparation
            facts.daemon_preparation = daemon;
            if daemon != "stopped" && daemon != "not_needed" {
                return fail(
                    "CODEX_DAEMON_STOP_FAILED",
                    "Daemon stop did not confirm its final state.",
                );
            }
            daemon_stopped = daemon == "stopped";
            cancel.check()?;
        }
        // Fresh registration plus a final process check cover a package update
        // or another client starting Desktop during the cancellable stop wait.
        if !same_target(&desktop, &(services.discover)(config, cancel)?) {
            return fail(
                "APPX_PACKAGE_CHANGED",
                "Desktop changed during preparation; refresh and retry.",
            );
        }
        require_stopped(&desktop)?;
        elevation::require_not_elevated()?;
        cancel.check()?;
        (services.mark_submitted)()?;
        let mut receipt;
        if desktop.registered {
            facts.activation_state = "unknown";
            let worker = (services.activate)(&desktop, &plan.arguments, cancel)?;
            if worker.at("activation_returned").as_bool().unwrap_or(false) {
                facts.activation_state = "returned";
            }
            receipt = activation_receipt(&worker)?;
            receipt.insert(
                "proxy_delivery",
                Json::from(if options.activation_only {
                    "not_established"
                } else if backend == "prepared" {
                    "activation_arguments_and_home_config"
                } else {
                    "activation_arguments"
                }),
            );
        } else {
            let mut environment = proxy_environment(config)?;
            if !cli.home.as_os_str().is_empty() {
                environment.set("CODEX_HOME", &cli.home.to_string_lossy());
            }
            facts.activation_state = "unknown";
            receipt = native_launch(&desktop, &mut environment, cancel)?;
            facts.activation_state = "returned";
            receipt.insert("proxy_delivery", Json::from("process_environment"));
        }
        receipt.insert(
            "proxy_endpoint",
            if options.activation_only {
                Json::Null
            } else {
                Json::from(plan.endpoint.as_str())
            },
        );
        receipt.insert("backend_proxy_config", Json::from(backend));
        receipt.insert("daemon_preparation", Json::from(facts.daemon_preparation));
        receipt.insert("desktop", desktop.public_json());
        Ok(receipt)
    })();
    match outcome {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            if error.code == "LAUNCH_CANCELLED" && facts.activation_state == "unknown" {
                facts.activation_state = "not_submitted";
            }
            if daemon_stopped {
                return Err(Error::new(
                    error.code,
                    format!(
                        "{}; the shared Codex background server was stopped before this failure.",
                        error.message
                    ),
                ));
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deterministic_refusal_needs_no_file_handle_or_process() {
        // Preflight is the only gate that can refuse before the configuration
        // lease and the cross-process startup lock are taken, so both refusals
        // are pure functions of the options and the stored configuration.
        let conflict = preflight(
            &Config::default(),
            &LaunchOptions {
                repair: true,
                activation_only: true,
            },
        )
        .expect_err("repair cannot be combined with activation-only");
        assert_eq!(conflict.code, "INVALID_LAUNCH_OPTIONS");

        let unusable = Config {
            port: 0,
            ..Config::default()
        };
        let refused = preflight(&unusable, &LaunchOptions::default())
            .expect_err("a port outside 1..=65535 is not launchable");
        assert_eq!(refused.code, "CONFIG_INVALID");
    }
}
