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

#[cfg(windows)]
const APPX_DISCOVERY_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
function Read-ManifestAttribute($element, [string]$name) {
  $attribute = @($element.Attributes | Where-Object { $_.LocalName -eq $name })
  if ($attribute.Count -gt 1) { throw "APPX_DISCOVERY_INVALID: ambiguous manifest attribute $name" }
  if ($attribute.Count -eq 1) { return [string]$attribute[0].Value }
  return $null
}
$records = @(
  foreach ($name in @('OpenAI.Codex', 'OpenAI.ChatGPT-Desktop')) {
    $package = Get-AppxPackage -Name $name | Sort-Object Version -Descending | Select-Object -First 1
    if ($null -ne $package) {
      $manifest = Get-AppxPackageManifest -Package $package.PackageFullName
      $applications = @(
        $manifest.Package.Applications.Application | ForEach-Object {
          [PSCustomObject]@{
            application_id = [string]$_.Id
            manifest_executable = [string]$_.Executable
            entry_point = [string]$_.EntryPoint
            runtime_behavior = Read-ManifestAttribute $_ 'RuntimeBehavior'
            trust_level = Read-ManifestAttribute $_ 'TrustLevel'
          }
        }
      )
      if ($applications.Count -gt 16) { throw 'APPX_DISCOVERY_INVALID: package has too many applications' }
      [PSCustomObject]@{
        package_name = [string]$package.Name
        package_full_name = [string]$package.PackageFullName
        package_family_name = [string]$package.PackageFamilyName
        package_version = [string]$package.Version
        architecture = [string]$package.Architecture
        install_location = [string]$package.InstallLocation
        applications = $applications
      }
    }
  }
)
if ($records.Count -gt 0) {
  $records | ConvertTo-Json -Compress
}
"#;
#[cfg(windows)]
const APPX_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(windows)]
const MAX_APPX_STDOUT_BYTES: u64 = 64 * 1024;
#[cfg(windows)]
const MAX_APPX_STDERR_BYTES: u64 = 128 * 1024;

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
    #[serde(default)]
    runtime_behavior: String,
    #[serde(default)]
    trust_level: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AppxRecords {
    One(AppxRecord),
    Many(Vec<AppxRecord>),
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
        desktop_info_from_discovery_output(
            &String::from_utf8_lossy(&stdout),
            &config.codex.executable_override,
        )
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

/// A successful discovery helper emits no JSON when no supported package is
/// installed. That is distinct from a failed helper: the caller has already
/// checked its exit status before reaching here. A native executable override
/// remains usable in that specific no-package case only.
#[cfg(windows)]
fn desktop_info_from_discovery_output(
    input: &str,
    executable_override: &Path,
) -> Result<DesktopAppInfo, String> {
    let records = if input.trim().is_empty() {
        Vec::new()
    } else {
        parse_appx_records(input)?
    };
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

fn parse_appx_records(input: &str) -> Result<Vec<AppxRecord>, String> {
    if input.trim().is_empty() {
        return Err(
            "CODEX_NOT_INSTALLED: ChatGPT Desktop was not found. Install the current app from https://chatgpt.com/download/ (Microsoft Store ID 9PLM9XGG6VKS), or set codex.executable_override"
                .into(),
        );
    }
    let records: AppxRecords = serde_json::from_str(input)
        .map_err(|error| format!("APPX_DISCOVERY_INVALID: malformed PowerShell JSON: {error}"))?;
    let records = match records {
        AppxRecords::One(record) => vec![record],
        AppxRecords::Many(records) => records,
    };
    if records.is_empty() {
        return Err(no_supported_package_error());
    }
    Ok(records)
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
        && application.runtime_behavior.trim().is_empty()
        && application.trust_level.trim().is_empty()
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

    #[test]
    fn parses_path_with_spaces() {
        let root = temp_install();
        let input = serde_json::json!({
            "package_name": "OpenAI.ChatGPT-Desktop",
            "package_full_name": "OpenAI.ChatGPT-Desktop_1.2.3.4_x64__fixture",
            "package_family_name": "OpenAI.ChatGPT-Desktop_fixture",
            "package_version": "1.2.3.4",
            "architecture": "X64",
            "install_location": root,
            "applications": [{
                "application_id": "Desktop",
                "manifest_executable": "app/ChatGPT.exe",
                "entry_point": "Windows.FullTrustApplication"
            }]
        })
        .to_string();
        let result = parse_appx_json(&input).unwrap();
        assert_eq!(result.product, DesktopProduct::ChatGptClassic);
        assert_eq!(result.package_name, "OpenAI.ChatGPT-Desktop");
        assert_eq!(result.architecture, "X64");
        assert_eq!(
            result.discovery_source,
            DesktopDiscoverySource::AppxManifest
        );
        fs::remove_dir_all(result.install_location).unwrap();
    }

    #[test]
    fn rejects_ambiguous_classic_desktop_entries() {
        let root = temp_install();
        fs::write(root.join("app").join("ChatGPT-copy.exe"), b"test").unwrap();
        let input = serde_json::json!({
            "package_name": "OpenAI.ChatGPT-Desktop",
            "package_full_name": "OpenAI.ChatGPT-Desktop_1.0.0.0_x64__fixture",
            "package_family_name": "OpenAI.ChatGPT-Desktop_fixture",
            "package_version": "1.0.0.0",
            "architecture": "X64",
            "install_location": root.clone(),
            "applications": [
                {"application_id": "One", "manifest_executable": "app/ChatGPT.exe", "entry_point": "Windows.FullTrustApplication"},
                {"application_id": "Two", "manifest_executable": "other/ChatGPT.exe", "entry_point": "Windows.FullTrustApplication"}
            ]
        })
        .to_string();
        let error = parse_appx_json(&input).unwrap_err();
        assert!(error.contains("APPX_APPLICATION_AMBIGUOUS"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_missing_registered_identity_metadata() {
        let root = temp_install();
        let input = serde_json::json!({
            "package_name": "OpenAI.Codex",
            "package_version": "1.0.0.0",
            "architecture": "X64",
            "install_location": root.clone(),
            "applications": [{
                "application_id": "App",
                "manifest_executable": "app/ChatGPT.exe",
                "entry_point": "Windows.FullTrustApplication"
            }]
        })
        .to_string();
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
                    runtime_behavior: String::new(),
                    trust_level: String::new(),
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
    fn empty_successful_discovery_allows_a_native_override() {
        let root = temp_install();
        let executable = root.join("app").join("ChatGPT.exe");
        let info = desktop_info_from_discovery_output("", &executable).unwrap();
        assert_eq!(info.target_kind, DesktopTargetKind::UnpackagedExecutable);
        assert_eq!(
            info.discovery_source,
            DesktopDiscoverySource::ExecutableOverride
        );
        assert_eq!(info.executable, fs::canonicalize(&executable).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prefers_current_chatgpt_when_classic_has_a_higher_version() {
        let current = temp_install();
        let classic = temp_install();
        let input = serde_json::json!([
            {
                "package_name": "OpenAI.ChatGPT-Desktop",
                "package_full_name": "OpenAI.ChatGPT-Desktop_99.0.0.0_x64__fixture",
                "package_family_name": "OpenAI.ChatGPT-Desktop_fixture",
                "package_version": "99.0.0.0",
                "architecture": "X64",
                "install_location": classic.clone(),
                "applications": [{
                    "application_id": "Classic",
                    "manifest_executable": "app/ChatGPT.exe",
                    "entry_point": "Windows.FullTrustApplication"
                }]
            },
            {
                "package_name": "OpenAI.Codex",
                "package_full_name": "OpenAI.Codex_1.0.0.0_arm64__fixture",
                "package_family_name": "OpenAI.Codex_fixture",
                "package_version": "1.0.0.0",
                "architecture": "Arm64",
                "install_location": current,
                "applications": [{
                    "application_id": "App",
                    "manifest_executable": "app/ChatGPT.exe",
                    "entry_point": "Windows.FullTrustApplication"
                }]
            }
        ])
        .to_string();
        let result = parse_appx_json(&input).unwrap();
        assert_eq!(result.product, DesktopProduct::ChatGpt);
        assert_eq!(result.package_name, "OpenAI.Codex");
        assert_eq!(result.architecture, "Arm64");
        fs::remove_dir_all(result.install_location).unwrap();
        fs::remove_dir_all(classic).unwrap();
    }

    #[test]
    fn manifest_executable_is_preferred_and_recorded() {
        let root = temp_install();
        fs::write(root.join("app").join("Codex.exe"), b"test").unwrap();
        let input = serde_json::json!({
            "package_name": "OpenAI.Codex",
            "package_full_name": "OpenAI.Codex_26.727.6591.0_x64__fixture",
            "package_family_name": "OpenAI.Codex_fixture",
            "package_version": "26.727.6591.0",
            "architecture": "X64",
            "install_location": root.clone(),
            "applications": [
                {
                    "application_id": "App",
                    "manifest_executable": "app/ChatGPT.exe",
                    "entry_point": "Windows.FullTrustApplication"
                },
                {
                    "application_id": "CodexCoreCommandRunner",
                    "manifest_executable": "app/resources/codex-command-runner.exe",
                    "entry_point": "Windows.FullTrustApplication"
                }
            ]
        })
        .to_string();
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
    fn empty_output_is_actionable() {
        let error = parse_appx_json("").unwrap_err();
        assert!(error.contains("CODEX_NOT_INSTALLED"));
        assert!(error.contains("9PLM9XGG6VKS"));
    }

    #[test]
    fn appx_output_limits_are_strict() {
        assert!(validate_appx_output_lengths(64 * 1024, 128 * 1024).is_ok());
        assert!(validate_appx_output_lengths(64 * 1024 + 1, 0).is_err());
        assert!(validate_appx_output_lengths(0, 128 * 1024 + 1).is_err());
    }
}
