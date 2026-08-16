use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[cfg(windows)]
use std::{process::Stdio, time::Duration};

use proxy_guard_core::{DesktopAppInfo, DesktopDiscoverySource, DesktopProduct, GuardConfig};
use serde::Deserialize;
#[cfg(windows)]
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
const APPX_DISCOVERY_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$records = @(
  foreach ($name in @('OpenAI.Codex', 'OpenAI.ChatGPT-Desktop')) {
    $package = Get-AppxPackage -Name $name | Sort-Object Version -Descending | Select-Object -First 1
    if ($null -ne $package) {
      $manifestExecutable = $null
      try {
        $manifest = Get-AppxPackageManifest -Package $package
        $application = @(
          $manifest.Package.Applications.Application |
            Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_.Executable) } |
            Select-Object -First 1
        )
        if ($application.Count -gt 0) {
          $manifestExecutable = [string]$application[0].Executable
        }
      } catch {
        $manifestExecutable = $null
      }
      [PSCustomObject]@{
        package_name = [string]$package.Name
        package_version = [string]$package.Version
        architecture = [string]$package.Architecture
        install_location = [string]$package.InstallLocation
        manifest_executable = $manifestExecutable
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
    package_version: String,
    #[serde(default)]
    architecture: String,
    install_location: PathBuf,
    #[serde(default)]
    manifest_executable: Option<String>,
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
    if !config.codex.executable_override.as_os_str().is_empty() {
        return info_from_override(&config.codex.executable_override);
    }
    if let Some(cached) = cached
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

        let mut command = Command::new("powershell.exe");
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
        let stdout = String::from_utf8_lossy(&stdout);
        parse_appx_json(stdout.trim())
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn validate_appx_output_lengths(stdout_len: usize, stderr_len: usize) -> Result<(), String> {
    if stdout_len > 64 * 1024 {
        return Err("APPX_DISCOVERY_OUTPUT_LIMIT: stdout exceeded 64 KiB".into());
    }
    if stderr_len > 128 * 1024 {
        return Err("APPX_DISCOVERY_OUTPUT_LIMIT: stderr exceeded 128 KiB".into());
    }
    Ok(())
}

pub fn parse_appx_json(input: &str) -> Result<DesktopAppInfo, String> {
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
    let record = select_preferred_record(records)?;
    if record.install_location.as_os_str().is_empty() {
        return Err("APPX_DISCOVERY_INVALID: package has no install location".into());
    }
    let product = desktop_product(&record.package_name)?;
    let (install_location, executable, discovery_source) = resolve_appx_executable(&record)?;
    Ok(DesktopAppInfo {
        product,
        package_name: record.package_name,
        package_version: record.package_version,
        architecture: metadata_or_unknown(record.architecture),
        discovery_source,
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
) -> Result<(PathBuf, PathBuf, DesktopDiscoverySource), String> {
    let install_location = fs::canonicalize(&record.install_location).map_err(|error| {
        format!(
            "APPX_INSTALL_LOCATION_INVALID: cannot canonicalize {}: {error}",
            record.install_location.display()
        )
    })?;

    if let Some(manifest_executable) = record.manifest_executable.as_deref() {
        let candidate = manifest_executable_path(&install_location, manifest_executable)?;
        if candidate.is_file() {
            let executable = canonicalize_existing(&candidate)?;
            ensure_within_install_location(&install_location, &executable)?;
            return Ok((
                install_location,
                executable,
                DesktopDiscoverySource::AppxManifest,
            ));
        }
    }

    let executable = [
        install_location.join("app").join("ChatGPT.exe"),
        install_location.join("app").join("Codex.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| {
        format!(
            "CODEX_EXECUTABLE_MISSING: neither the APPX manifest executable nor app\\ChatGPT.exe/app\\Codex.exe exists under {}",
            install_location.display()
        )
    })?;
    let executable = canonicalize_existing(&executable)?;
    ensure_within_install_location(&install_location, &executable)?;
    Ok((
        install_location,
        executable,
        DesktopDiscoverySource::KnownExecutableFallback,
    ))
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

fn info_from_override(path: &Path) -> Result<DesktopAppInfo, String> {
    let executable = canonicalize_existing(path)?;
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
            "package_version": "1.2.3.4",
            "architecture": "X64",
            "install_location": root,
        })
        .to_string();
        let result = parse_appx_json(&input).unwrap();
        assert_eq!(result.product, DesktopProduct::ChatGptClassic);
        assert_eq!(result.package_name, "OpenAI.ChatGPT-Desktop");
        assert_eq!(result.architecture, "X64");
        assert_eq!(
            result.discovery_source,
            DesktopDiscoverySource::KnownExecutableFallback
        );
        fs::remove_dir_all(result.install_location).unwrap();
    }

    #[test]
    fn prefers_current_chatgpt_when_classic_has_a_higher_version() {
        let current = temp_install();
        let classic = temp_install();
        let input = serde_json::json!([
            {
                "package_name": "OpenAI.ChatGPT-Desktop",
                "package_version": "99.0.0.0",
                "architecture": "X64",
                "install_location": classic.clone(),
            },
            {
                "package_name": "OpenAI.Codex",
                "package_version": "1.0.0.0",
                "architecture": "Arm64",
                "install_location": current,
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
            "package_version": "26.727.6591.0",
            "architecture": "X64",
            "install_location": root.clone(),
            "manifest_executable": "app/Codex.exe",
        })
        .to_string();
        let result = parse_appx_json(&input).unwrap();
        assert_eq!(result.product, DesktopProduct::ChatGpt);
        assert!(result.executable.ends_with("app/Codex.exe"));
        assert_eq!(
            result.discovery_source,
            DesktopDiscoverySource::AppxManifest
        );
        fs::remove_dir_all(result.install_location).unwrap();
    }

    #[test]
    fn rejects_manifest_paths_outside_the_install_location() {
        let root = temp_install();
        let input = serde_json::json!({
            "package_name": "OpenAI.Codex",
            "package_version": "26.727.6591.0",
            "architecture": "X64",
            "install_location": root,
            "manifest_executable": "../outside.exe",
        })
        .to_string();
        let error = parse_appx_json(&input).unwrap_err();
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
