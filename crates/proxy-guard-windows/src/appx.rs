use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[cfg(windows)]
use std::{process::Stdio, time::Duration};

use proxy_guard_core::{
    DesktopAppInfo, DesktopDiscoverySource, DesktopProduct, DesktopTargetKind, GuardConfig,
    PackageApplication, PackageRuntimeKind,
};
use serde::Deserialize;
#[cfg(windows)]
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

/// The production discovery script lives in `resources/appx-discovery.ps1`
/// and is shared verbatim by the production launch path, the Windows
/// integration tests, and manual diagnosis (`powershell -File
/// resources/appx-discovery.ps1`), so hand-written probes can never drift
/// from what Guard actually executes.
#[cfg(windows)]
const APPX_DISCOVERY_SCRIPT: &str = include_str!("../../../resources/appx-discovery.ps1");
#[cfg(windows)]
const APPX_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(windows)]
const MAX_APPX_STDOUT_BYTES: u64 = 64 * 1024;
#[cfg(windows)]
const MAX_APPX_STDERR_BYTES: u64 = 128 * 1024;

/// Schema version of the discovery envelope the PowerShell script emits.
const APPX_DISCOVERY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct AppxRecord {
    package_name: String,
    #[serde(default)]
    package_full_name: String,
    #[serde(default)]
    package_family_name: String,
    package_version: String,
    #[serde(default)]
    architecture: String,
    install_location: PathBuf,
    #[serde(default)]
    applications: Vec<AppxApplicationRecord>,
}

#[derive(Debug, Deserialize)]
struct AppxApplicationRecord {
    #[serde(default)]
    application_id: String,
    #[serde(default)]
    manifest_executable: String,
    #[serde(default)]
    entry_point: String,
    /// Optional manifest attributes. The PowerShell contract emits JSON null
    /// when the manifest omits them: absence is legal protocol state, and a
    /// null must deserialize cleanly instead of failing the whole record.
    #[serde(default)]
    runtime_behavior: Option<String>,
    #[serde(default)]
    trust_level: Option<String>,
}

/// Fixed-shape discovery envelope. Whatever the package count (0, 1, many),
/// the top level is always the same object — never a bare record and never a
/// bare array, which is what the old untagged enum had to guess between.
#[derive(Debug, Deserialize)]
struct AppxDiscoveryEnvelope {
    schema_version: u32,
    #[serde(default)]
    records: Vec<AppxRecord>,
}

/// Optional manifest attribute text: `None` and empty strings mean the same
/// thing — the attribute is absent.
fn optional_manifest_text(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("").trim()
}

pub async fn discover_desktop_app(
    config: &GuardConfig,
    cached: Option<&DesktopAppInfo>,
    cancellation: &CancellationToken,
) -> Result<DesktopAppInfo, String> {
    #[cfg(not(windows))]
    if !config.codex.executable_override.as_os_str().is_empty() {
        return info_from_unpacked_override(&config.codex.executable_override);
    }
    if let Some(cached) = cached
        && config.codex.executable_override.as_os_str().is_empty()
        && cached.executable.is_file()
    {
        return Ok(cached.clone());
    }

    #[cfg(not(windows))]
    {
        let _ = (config, cancellation);
        Err("APPX_UNSUPPORTED: automatic ChatGPT/Codex Desktop discovery requires Windows".into())
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let mut command = Command::new(system_powershell()?);
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                APPX_DISCOVERY_SCRIPT,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command.as_std_mut().creation_flags(0x0800_0000);

        let mut child = command
            .spawn()
            .map_err(|error| format!("APPX_DISCOVERY_FAILED: cannot run PowerShell: {error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "APPX_DISCOVERY_IO: stdout was not captured".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "APPX_DISCOVERY_IO: stderr was not captured".to_string())?;
        let operation = async move {
            let stdout_task = tokio::spawn(async move {
                let mut bytes = Vec::new();
                stdout
                    .take(MAX_APPX_STDOUT_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map(|_| bytes)
            });
            let stderr_task = tokio::spawn(async move {
                let mut bytes = Vec::new();
                stderr
                    .take(MAX_APPX_STDERR_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map(|_| bytes)
            });
            let status = child.wait().await;
            let stdout = stdout_task
                .await
                .map_err(|error| format!("APPX_DISCOVERY_IO: stdout reader failed: {error}"))?
                .map_err(|error| format!("APPX_DISCOVERY_IO: stdout read failed: {error}"))?;
            let stderr = stderr_task
                .await
                .map_err(|error| format!("APPX_DISCOVERY_IO: stderr reader failed: {error}"))?
                .map_err(|error| format!("APPX_DISCOVERY_IO: stderr read failed: {error}"))?;
            Ok::<_, String>((status, stdout, stderr))
        };
        let (status, stdout, stderr) = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err("APPX_DISCOVERY_CANCELLED: Guard is shutting down".into());
            }
            result = tokio::time::timeout(APPX_TIMEOUT, operation) => {
                result
                    .map_err(|_| "APPX_DISCOVERY_TIMEOUT: PowerShell exceeded 15 seconds".to_string())??
            }
        };
        let status = status.map_err(|error| format!("APPX_DISCOVERY_WAIT_FAILED: {error}"))?;
        validate_appx_output_lengths(stdout.len(), stderr.len())?;
        if !status.success() {
            let stderr = proxy_guard_core::redact_text(&String::from_utf8_lossy(&stderr));
            return Err(format!(
                "APPX_DISCOVERY_FAILED: PowerShell exited with {}; {}",
                status,
                stderr.trim()
            ));
        }
        // Identity metadata must not survive silent character replacement:
        // corrupted bytes are a protocol error, not mojibake package names.
        let stdout_text = String::from_utf8(stdout).map_err(|_| {
            "APPX_DISCOVERY_PROTOCOL_INVALID: PowerShell output was not valid UTF-8".to_string()
        })?;
        desktop_info_from_discovery_output(&stdout_text, &config.codex.executable_override)
    }
}

#[cfg(windows)]
fn validate_appx_output_lengths(stdout_len: usize, stderr_len: usize) -> Result<(), String> {
    if stdout_len > 64 * 1024 {
        return Err("APPX_DISCOVERY_OUTPUT_LIMIT: stdout exceeded 64 KiB".into());
    }
    if stderr_len > 128 * 1024 {
        return Err("APPX_DISCOVERY_OUTPUT_LIMIT: stderr exceeded 128 KiB".into());
    }
    Ok(())
}

/// Resolve the system PowerShell from OS metadata, never inherited environment.
#[cfg(windows)]
fn system_powershell() -> Result<PathBuf, String> {
    crate::system_tools::system_powershell()
        .map_err(|error| format!("APPX_DISCOVERY_FAILED: {error}"))
}

pub fn parse_appx_json(input: &str) -> Result<DesktopAppInfo, String> {
    desktop_info_from_records(parse_appx_records(input)?)
}

/// The exact discovery script the production path executes. Public so the
/// Windows integration tests (and external diagnostics) exercise the same
/// source instead of a hand-rewritten copy.
#[cfg(windows)]
pub fn discovery_script() -> &'static str {
    APPX_DISCOVERY_SCRIPT
}

/// A successful helper always emits one envelope document (even with zero
/// records). Empty stdout after a zero exit status is therefore a broken
/// protocol, never "no package installed"; the caller has already checked
/// the exit status before reaching here. A native executable override
/// remains usable in the specific zero-record case only.
#[cfg(windows)]
fn desktop_info_from_discovery_output(
    input: &str,
    executable_override: &Path,
) -> Result<DesktopAppInfo, String> {
    if input.trim().is_empty() {
        return Err(
            "APPX_DISCOVERY_PROTOCOL_INVALID: the discovery script produced no envelope".into(),
        );
    }
    let records = parse_appx_records(input)?;
    if records.is_empty() {
        return if executable_override.as_os_str().is_empty() {
            Err(no_supported_package_error())
        } else {
            info_from_unpacked_override_canonical(canonicalize_existing(executable_override)?)
        };
    }
    if executable_override.as_os_str().is_empty() {
        desktop_info_from_records(records)
    } else {
        info_from_override(executable_override, records)
    }
}

/// Parses the fixed discovery envelope and checks its schema version. Zero
/// records is a valid, parseable result; the business layer decides what it
/// means. A parse failure never falls back to a package-less or bare-EXE
/// launch.
fn parse_appx_records(input: &str) -> Result<Vec<AppxRecord>, String> {
    let envelope: AppxDiscoveryEnvelope = serde_json::from_str(input).map_err(|error| {
        format!("APPX_DISCOVERY_PROTOCOL_INVALID: malformed discovery envelope: {error}")
    })?;
    if envelope.schema_version != APPX_DISCOVERY_SCHEMA_VERSION {
        return Err(format!(
            "APPX_DISCOVERY_PROTOCOL_UNSUPPORTED: schema version {}; expected {}",
            envelope.schema_version, APPX_DISCOVERY_SCHEMA_VERSION
        ));
    }
    Ok(envelope.records)
}

fn no_supported_package_error() -> String {
    "CODEX_NOT_INSTALLED: no supported ChatGPT Desktop package was returned by Windows".into()
}

fn desktop_info_from_records(records: Vec<AppxRecord>) -> Result<DesktopAppInfo, String> {
    let record = select_preferred_record(records)?;
    if record.install_location.as_os_str().is_empty() {
        return Err("APPX_DISCOVERY_INVALID: package has no install location".into());
    }
    let product = desktop_product(&record.package_name)?;
    let (install_location, executable, application) = resolve_appx_executable(&record, product)?;
    Ok(DesktopAppInfo {
        product,
        package_name: record.package_name,
        package_version: record.package_version,
        architecture: metadata_or_unknown(record.architecture),
        discovery_source: DesktopDiscoverySource::AppxManifest,
        target_kind: DesktopTargetKind::RegisteredPackage(application),
        install_location,
        executable,
    })
}

fn select_preferred_record(records: Vec<AppxRecord>) -> Result<AppxRecord, String> {
    let mut classic = None;
    for record in records {
        match record.package_name.as_str() {
            "OpenAI.Codex" => return Ok(record),
            "OpenAI.ChatGPT-Desktop" if classic.is_none() => classic = Some(record),
            _ => {}
        }
    }
    classic.ok_or_else(|| {
        "CODEX_NOT_INSTALLED: no supported ChatGPT Desktop package was returned by Windows".into()
    })
}

fn desktop_product(package_name: &str) -> Result<DesktopProduct, String> {
    match package_name {
        "OpenAI.Codex" => Ok(DesktopProduct::ChatGpt),
        "OpenAI.ChatGPT-Desktop" => Ok(DesktopProduct::ChatGptClassic),
        _ => Err(format!(
            "APPX_DISCOVERY_INVALID: unsupported ChatGPT package name: {package_name}"
        )),
    }
}

fn resolve_appx_executable(
    record: &AppxRecord,
    product: DesktopProduct,
) -> Result<(PathBuf, PathBuf, PackageApplication), String> {
    let install_location = fs::canonicalize(&record.install_location).map_err(|error| {
        format!(
            "APPX_INSTALL_LOCATION_INVALID: cannot canonicalize {}: {error}",
            record.install_location.display()
        )
    })?;

    let application = select_package_application(record, product)?;
    let candidate = manifest_executable_path(&install_location, &application.manifest_executable)?;
    if !candidate.is_file() {
        return Err(format!(
            "CODEX_EXECUTABLE_MISSING: selected APPX application executable does not exist: {}",
            candidate.display()
        ));
    }
    let executable = candidate;
    let executable = canonicalize_existing(&executable)?;
    ensure_within_install_location(&install_location, &executable)?;
    Ok((install_location, executable, application))
}

fn select_package_application(
    record: &AppxRecord,
    product: DesktopProduct,
) -> Result<PackageApplication, String> {
    let package_full_name = required_metadata(&record.package_full_name, "PackageFullName")?;
    let package_family_name = required_metadata(&record.package_family_name, "PackageFamilyName")?;
    let candidates: Vec<_> = record
        .applications
        .iter()
        .filter(|application| match product {
            DesktopProduct::ChatGpt => {
                application.application_id == "App"
                    && manifest_path_matches(&application.manifest_executable, "app/ChatGPT.exe")
            }
            DesktopProduct::ChatGptClassic => {
                package_runtime_kind(application) == PackageRuntimeKind::FullTrustDesktop
                    && manifest_file_name_is(&application.manifest_executable, "ChatGPT.exe")
            }
            DesktopProduct::ExecutableOverride => false,
        })
        .collect();
    let [application] = candidates.as_slice() else {
        return if candidates.is_empty() {
            Err(format!(
                "APPX_APPLICATION_MISSING: no trusted Desktop application was found in {}",
                record.package_name
            ))
        } else {
            Err(format!(
                "APPX_APPLICATION_AMBIGUOUS: multiple trusted Desktop applications were found in {}",
                record.package_name
            ))
        };
    };
    let application_id = required_metadata(&application.application_id, "Application.Id")?;
    let manifest_executable =
        required_metadata(&application.manifest_executable, "Application.Executable")?;
    Ok(PackageApplication {
        app_user_model_id: format!("{package_family_name}!{application_id}"),
        package_full_name,
        package_family_name,
        application_id,
        manifest_executable,
        runtime_kind: package_runtime_kind(application),
    })
}

fn required_metadata(value: &str, field: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("APPX_METADATA_INCOMPLETE: {field} is missing"))
    } else {
        Ok(value.into())
    }
}

fn package_runtime_kind(application: &AppxApplicationRecord) -> PackageRuntimeKind {
    if application
        .entry_point
        .eq_ignore_ascii_case("Windows.FullTrustApplication")
    {
        PackageRuntimeKind::FullTrustDesktop
    } else if application.entry_point.trim().is_empty()
        && optional_manifest_text(&application.runtime_behavior).is_empty()
        && optional_manifest_text(&application.trust_level).is_empty()
    {
        PackageRuntimeKind::Unknown
    } else {
        PackageRuntimeKind::AppContainer
    }
}

fn manifest_path_matches(value: &str, expected: &str) -> bool {
    value.replace('\\', "/").eq_ignore_ascii_case(expected)
}

fn manifest_file_name_is(value: &str, expected: &str) -> bool {
    Path::new(value)
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(expected))
}

fn manifest_executable_path(install_location: &Path, executable: &str) -> Result<PathBuf, String> {
    let path = Path::new(executable);
    let invalid = executable.trim().is_empty()
        || path.is_absolute()
        || executable.starts_with(['/', '\\'])
        || executable.contains(':')
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
    if invalid {
        return Err(format!(
            "APPX_EXECUTABLE_INVALID: APPX manifest executable must be a relative path inside the install location: {executable}"
        ));
    }
    Ok(install_location.join(path))
}

fn ensure_within_install_location(
    install_location: &Path,
    executable: &Path,
) -> Result<(), String> {
    if executable.starts_with(install_location) {
        Ok(())
    } else {
        Err(format!(
            "APPX_EXECUTABLE_INVALID: {} resolves outside APPX install location {}",
            executable.display(),
            install_location.display()
        ))
    }
}

fn metadata_or_unknown(value: String) -> String {
    let value = value.trim();
    if value.is_empty() {
        "unknown".into()
    } else {
        value.into()
    }
}

#[cfg(windows)]
fn info_from_override(path: &Path, records: Vec<AppxRecord>) -> Result<DesktopAppInfo, String> {
    let executable = canonicalize_existing(path)?;
    for record in records {
        let product = match desktop_product(&record.package_name) {
            Ok(product) => product,
            Err(_) => continue,
        };
        let Ok(candidate_install_location) = fs::canonicalize(&record.install_location) else {
            continue;
        };
        if !executable.starts_with(&candidate_install_location) {
            continue;
        }
        let (install_location, registered_executable, application) =
            resolve_appx_executable(&record, product)?;
        if executable == registered_executable {
            return Ok(DesktopAppInfo {
                product,
                package_name: record.package_name,
                package_version: record.package_version,
                architecture: metadata_or_unknown(record.architecture),
                discovery_source: DesktopDiscoverySource::ExecutableOverride,
                target_kind: DesktopTargetKind::RegisteredPackage(application),
                install_location,
                executable,
            });
        }
        return Err(format!(
            "APPX_OVERRIDE_INVALID: configured executable is inside registered package {} but is not its selected Desktop application",
            record.package_name
        ));
    }
    info_from_unpacked_override_canonical(executable)
}

#[cfg(not(windows))]
fn info_from_unpacked_override(path: &Path) -> Result<DesktopAppInfo, String> {
    info_from_unpacked_override_canonical(canonicalize_existing(path)?)
}

fn info_from_unpacked_override_canonical(executable: PathBuf) -> Result<DesktopAppInfo, String> {
    let install_location = executable
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf();
    Ok(DesktopAppInfo {
        product: DesktopProduct::ExecutableOverride,
        package_name: "executable_override".into(),
        package_version: "manual".into(),
        architecture: "manual".into(),
        discovery_source: DesktopDiscoverySource::ExecutableOverride,
        target_kind: DesktopTargetKind::UnpackagedExecutable,
        install_location,
        executable,
    })
}

fn canonicalize_existing(path: &Path) -> Result<PathBuf, String> {
    if !path.is_file() {
        return Err(format!(
            "CODEX_EXECUTABLE_MISSING: configured executable does not exist: {}",
            path.display()
        ));
    }
    fs::canonicalize(path).map_err(|error| {
        format!(
            "CODEX_EXECUTABLE_INVALID: cannot canonicalize {}: {error}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_install() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cpg appx test {unique}"));
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(root.join("app").join("ChatGPT.exe"), b"test").unwrap();
        root
    }

    /// Wraps record JSON in the fixed envelope the production script emits.
    fn envelope(records: serde_json::Value) -> String {
        serde_json::json!({
            "schema_version": APPX_DISCOVERY_SCHEMA_VERSION,
            "records": records,
        })
        .to_string()
    }

    /// The application object shape the real manifest produces, including the
    /// JSON nulls PowerShell emits for omitted optional attributes.
    fn application_record(application_id: &str, manifest_executable: &str) -> serde_json::Value {
        serde_json::json!({
            "application_id": application_id,
            "manifest_executable": manifest_executable,
            "entry_point": "Windows.FullTrustApplication",
            "runtime_behavior": null,
            "trust_level": null,
        })
    }

    /// Fixture A: the typical current OpenAI.Codex shape (two applications,
    /// null optional attributes, backslash executable separators) parses and
    /// selects the registered Desktop entry.
    #[test]
    fn fixture_a_current_codex_shape_parses_with_nulls_and_two_entries() {
        let root = temp_install();
        let input = envelope(serde_json::json!([{
            "package_name": "OpenAI.Codex",
            "package_full_name": "OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0",
            "package_family_name": "OpenAI.Codex_2p2nqsd0c76g0",
            "package_version": "26.924.2738.0",
            "architecture": "X64",
            "install_location": root.clone(),
            "applications": [
                application_record("App", r"app\ChatGPT.exe"),
                application_record(
                    "CodexCoreCommandRunner",
                    r"app\resources\codex-command-runner.exe",
                ),
            ]
        }]));
        let result = parse_appx_json(&input).unwrap();
        assert_eq!(result.product, DesktopProduct::ChatGpt);
        assert_eq!(result.package_name, "OpenAI.Codex");
        assert_eq!(result.package_version, "26.924.2738.0");
        let DesktopTargetKind::RegisteredPackage(application) = result.target_kind else {
            panic!("manifest discovery must retain registered package identity");
        };
        assert_eq!(application.application_id, "App");
        assert_eq!(application.manifest_executable, r"app\ChatGPT.exe");
        assert_eq!(
            application.app_user_model_id,
            "OpenAI.Codex_2p2nqsd0c76g0!App"
        );
        assert_eq!(
            application.runtime_kind,
            PackageRuntimeKind::FullTrustDesktop
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Fixture B: zero records is a valid, parseable envelope. The parse
    /// layer succeeds; only the business layer turns it into
    /// CODEX_NOT_INSTALLED.
    #[test]
    fn fixture_b_empty_records_parse_but_mean_not_installed() {
        let records = parse_appx_records(&envelope(serde_json::json!([]))).unwrap();
        assert!(records.is_empty());
        let error = desktop_info_from_records(records).unwrap_err();
        assert!(error.contains("CODEX_NOT_INSTALLED"), "{error}");
    }

    /// Fixture C: null optional attributes deserialize; absent and empty
    /// strings behave identically in the runtime classification.
    #[test]
    fn fixture_c_null_optional_attributes_deserialize() {
        for (runtime_behavior, trust_level) in [
            (serde_json::Value::Null, serde_json::Value::Null),
            (serde_json::json!(""), serde_json::json!("")),
        ] {
            let root = temp_install();
            let input = envelope(serde_json::json!([{
                "package_name": "OpenAI.ChatGPT-Desktop",
                "package_full_name": "OpenAI.ChatGPT-Desktop_1.0.0.0_x64__fixture",
                "package_family_name": "OpenAI.ChatGPT-Desktop_fixture",
                "package_version": "1.0.0.0",
                "architecture": "X64",
                "install_location": root.clone(),
                "applications": [{
                    "application_id": "Desktop",
                    "manifest_executable": "app/ChatGPT.exe",
                    "entry_point": "Windows.FullTrustApplication",
                    "runtime_behavior": runtime_behavior,
                    "trust_level": trust_level,
                }]
            }]));
            let result = parse_appx_json(&input)
                .unwrap_or_else(|error| panic!("{runtime_behavior}/{trust_level}: {error}"));
            let DesktopTargetKind::RegisteredPackage(application) = result.target_kind else {
                panic!("registered package expected");
            };
            assert_eq!(
                application.runtime_kind,
                PackageRuntimeKind::FullTrustDesktop
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// Fixture D: two packages in one envelope still prefer OpenAI.Codex over
    /// Classic regardless of versions, and the top-level shape is identical
    /// for one or many records.
    #[test]
    fn fixture_d_multiple_packages_keep_one_shape_and_prefer_codex() {
        let current = temp_install();
        let classic = temp_install();
        let input = envelope(serde_json::json!([
            {
                "package_name": "OpenAI.ChatGPT-Desktop",
                "package_full_name": "OpenAI.ChatGPT-Desktop_99.0.0.0_x64__fixture",
                "package_family_name": "OpenAI.ChatGPT-Desktop_fixture",
                "package_version": "99.0.0.0",
                "architecture": "X64",
                "install_location": classic.clone(),
                "applications": [application_record("Classic", "app/ChatGPT.exe")]
            },
            {
                "package_name": "OpenAI.Codex",
                "package_full_name": "OpenAI.Codex_1.0.0.0_arm64__fixture",
                "package_family_name": "OpenAI.Codex_fixture",
                "package_version": "1.0.0.0",
                "architecture": "Arm64",
                "install_location": current.clone(),
                "applications": [application_record("App", "app/ChatGPT.exe")]
            }
        ]));
        let result = parse_appx_json(&input).unwrap();
        assert_eq!(result.product, DesktopProduct::ChatGpt);
        assert_eq!(result.package_name, "OpenAI.Codex");
        assert_eq!(result.architecture, "Arm64");
        // Single record: same envelope shape, same code path.
        let single = envelope(serde_json::json!([{
            "package_name": "OpenAI.Codex",
            "package_full_name": "OpenAI.Codex_1.0.0.0_arm64__fixture",
            "package_family_name": "OpenAI.Codex_fixture",
            "package_version": "1.0.0.0",
            "architecture": "Arm64",
            "install_location": current.clone(),
            "applications": [application_record("App", "app/ChatGPT.exe")]
        }]));
        let single_result = parse_appx_json(&single).unwrap();
        assert_eq!(single_result.package_name, result.package_name);
        fs::remove_dir_all(current).unwrap();
        fs::remove_dir_all(classic).unwrap();
    }

    /// Fixture E: an unknown schema version is an unsupported protocol, not a
    /// guess, and never falls back to a bare-EXE launch.
    #[test]
    fn fixture_e_unknown_schema_version_is_refused() {
        let input = r#"{"schema_version":999,"records":[]}"#;
        let error = parse_appx_records(input).unwrap_err();
        assert!(
            error.starts_with("APPX_DISCOVERY_PROTOCOL_UNSUPPORTED:"),
            "{error}"
        );
        assert!(error.contains("999"));
        // A bare record or bare array is a malformed envelope now — the old
        // untagged-enum guessing is gone.
        for legacy in [
            r#"{"package_name":"OpenAI.Codex"}"#,
            r#"[{"package_name":"OpenAI.Codex"}]"#,
        ] {
            assert!(
                parse_appx_records(legacy)
                    .unwrap_err()
                    .starts_with("APPX_DISCOVERY_PROTOCOL_INVALID:"),
                "{legacy}"
            );
        }
    }

    #[test]
    fn rejects_ambiguous_classic_desktop_entries() {
        let root = temp_install();
        fs::write(root.join("app").join("ChatGPT-copy.exe"), b"test").unwrap();
        let input = envelope(serde_json::json!([{
            "package_name": "OpenAI.ChatGPT-Desktop",
            "package_full_name": "OpenAI.ChatGPT-Desktop_1.0.0.0_x64__fixture",
            "package_family_name": "OpenAI.ChatGPT-Desktop_fixture",
            "package_version": "1.0.0.0",
            "architecture": "X64",
            "install_location": root.clone(),
            "applications": [
                application_record("One", "app/ChatGPT.exe"),
                application_record("Two", "other/ChatGPT.exe"),
            ]
        }]));
        let error = parse_appx_json(&input).unwrap_err();
        assert!(error.contains("APPX_APPLICATION_AMBIGUOUS"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_missing_registered_identity_metadata() {
        let root = temp_install();
        let input = envelope(serde_json::json!([{
            "package_name": "OpenAI.Codex",
            "package_version": "1.0.0.0",
            "architecture": "X64",
            "install_location": root.clone(),
            "applications": [application_record("App", "app/ChatGPT.exe")]
        }]));
        let error = parse_appx_json(&input).unwrap_err();
        assert!(error.contains("APPX_METADATA_INCOMPLETE"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn override_matching_the_registered_primary_entry_remains_packaged() {
        let root = temp_install();
        let executable = root.join("app").join("ChatGPT.exe");
        let info = info_from_override(
            &executable,
            vec![AppxRecord {
                package_name: "OpenAI.Codex".into(),
                package_full_name: "OpenAI.Codex_1.0.0.0_x64__fixture".into(),
                package_family_name: "OpenAI.Codex_fixture".into(),
                package_version: "1.0.0.0".into(),
                architecture: "X64".into(),
                install_location: root.clone(),
                applications: vec![AppxApplicationRecord {
                    application_id: "App".into(),
                    manifest_executable: "app/ChatGPT.exe".into(),
                    entry_point: "Windows.FullTrustApplication".into(),
                    runtime_behavior: None,
                    trust_level: None,
                }],
            }],
        )
        .unwrap();
        assert!(matches!(
            info.target_kind,
            DesktopTargetKind::RegisteredPackage(_)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn zero_records_with_override_allows_a_native_override_but_empty_stdout_does_not() {
        let root = temp_install();
        let executable = root.join("app").join("ChatGPT.exe");
        // A well-formed zero-record envelope plus an override stays usable.
        let info =
            desktop_info_from_discovery_output(&envelope(serde_json::json!([])), &executable)
                .unwrap();
        assert_eq!(info.target_kind, DesktopTargetKind::UnpackagedExecutable);
        assert_eq!(
            info.discovery_source,
            DesktopDiscoverySource::ExecutableOverride
        );
        assert_eq!(info.executable, fs::canonicalize(&executable).unwrap());
        // A silent success is a protocol break, not "no package".
        let error = desktop_info_from_discovery_output("", &executable).unwrap_err();
        assert!(
            error.starts_with("APPX_DISCOVERY_PROTOCOL_INVALID:"),
            "{error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_executable_is_preferred_and_recorded() {
        let root = temp_install();
        fs::write(root.join("app").join("Codex.exe"), b"test").unwrap();
        let input = envelope(serde_json::json!([{
            "package_name": "OpenAI.Codex",
            "package_full_name": "OpenAI.Codex_26.727.6591.0_x64__fixture",
            "package_family_name": "OpenAI.Codex_fixture",
            "package_version": "26.727.6591.0",
            "architecture": "X64",
            "install_location": root.clone(),
            "applications": [
                application_record("App", "app/ChatGPT.exe"),
                application_record(
                    "CodexCoreCommandRunner",
                    "app/resources/codex-command-runner.exe",
                ),
            ]
        }]));
        let result = parse_appx_json(&input).unwrap();
        assert_eq!(result.product, DesktopProduct::ChatGpt);
        assert!(result.executable.ends_with("app/ChatGPT.exe"));
        assert_eq!(
            result.discovery_source,
            DesktopDiscoverySource::AppxManifest
        );
        let DesktopTargetKind::RegisteredPackage(application) = result.target_kind else {
            panic!("manifest discovery must retain registered package identity");
        };
        assert_eq!(application.application_id, "App");
        assert_eq!(application.app_user_model_id, "OpenAI.Codex_fixture!App");
        assert_eq!(
            application.runtime_kind,
            PackageRuntimeKind::FullTrustDesktop
        );
        fs::remove_dir_all(result.install_location).unwrap();
    }

    #[test]
    fn rejects_manifest_paths_outside_the_install_location() {
        let root = temp_install();
        let error = manifest_executable_path(&root, "../outside.exe").unwrap_err();
        assert!(error.contains("APPX_EXECUTABLE_INVALID"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_output_is_a_protocol_error() {
        let error = parse_appx_json("").unwrap_err();
        assert!(
            error.starts_with("APPX_DISCOVERY_PROTOCOL_INVALID:"),
            "empty output must not be reported as CODEX_NOT_INSTALLED: {error}"
        );
    }

    #[test]
    fn appx_output_limits_are_strict() {
        assert!(validate_appx_output_lengths(64 * 1024, 128 * 1024).is_ok());
        assert!(validate_appx_output_lengths(64 * 1024 + 1, 0).is_err());
        assert!(validate_appx_output_lengths(0, 128 * 1024 + 1).is_err());
    }

    /// The production constant, the integration tests, and manual diagnosis
    /// must all execute this same source file.
    #[cfg(windows)]
    #[test]
    fn production_script_is_the_shared_resource_file() {
        assert!(APPX_DISCOVERY_SCRIPT.contains("schema_version = 1"));
        assert!(APPX_DISCOVERY_SCRIPT.contains("ConvertTo-Json -Depth 8 -Compress"));
        assert!(APPX_DISCOVERY_SCRIPT.contains("OpenAI.Codex"));
        assert!(!APPX_DISCOVERY_SCRIPT.contains("if ($records.Count -gt 0)"));
    }
}
