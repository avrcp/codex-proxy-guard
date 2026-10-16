//! Pure discovery-record selection: envelope validation, candidate trust
//! rules (`OpenAI.Codex` requires App/`app/ChatGPT.exe`; Classic requires a
//! FullTrust entry point with `ChatGPT.exe` basename), manifest-executable
//! containment rules and override resolution decisions. Filesystem probes are
//! injected so tests stay hermetic.

use crate::error::{Error, Result};
use crate::json::{Json, parse_strict_json_object};
use crate::text::public_text;
use std::path::{Path, PathBuf};

pub const DISCOVERY_ERROR: &str = "APPX_DISCOVERY_PROTOCOL_INVALID";
const DISCOVERY_LIMIT: usize = 64 * 1024;

/// Filesystem probes the selection logic needs; production wires the Win32
/// canonicalization, tests provide fakes.
pub trait FsProbe {
    fn is_directory(&self, path: &Path) -> bool;
    fn is_regular_file(&self, path: &Path) -> bool;
    /// Canonicalized absolute path of a file (no reparse points), or None.
    fn canonical_file(&self, path: &Path) -> Option<PathBuf>;
    /// Canonicalized absolute path of a directory, or None.
    fn canonical_directory(&self, path: &Path) -> Option<PathBuf>;
    /// Case-folded normalized comparison of two Windows paths.
    fn same_normalized_path(&self, left: &Path, right: &Path) -> bool;
    /// True when `child` is inside (or equal to) `root` after normalization.
    fn path_within(&self, root: &Path, child: &Path) -> bool;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Desktop {
    pub product: &'static str,
    pub display_name: &'static str,
    pub package_name: String,
    pub package_version: String,
    pub architecture: String,
    pub discovery_source: &'static str,
    pub registered: bool,
    pub executable: PathBuf,
    pub install_location: PathBuf,
    pub package_full_name: String,
    pub package_family_name: String,
    pub application_id: String,
    pub aumid: String,
    pub manifest_executable: String,
    pub runtime_kind: &'static str,
}

impl Default for Desktop {
    fn default() -> Self {
        Desktop {
            product: "chat_gpt",
            display_name: "ChatGPT Desktop",
            package_name: String::new(),
            package_version: String::new(),
            architecture: String::new(),
            discovery_source: "",
            registered: false,
            executable: PathBuf::new(),
            install_location: PathBuf::new(),
            package_full_name: String::new(),
            package_family_name: String::new(),
            application_id: String::new(),
            aumid: String::new(),
            manifest_executable: String::new(),
            runtime_kind: "",
        }
    }
}

impl Desktop {
    pub fn public_json(&self) -> Json {
        Json::object_from([
            ("state", Json::from("found")),
            ("product", Json::from(self.product)),
            ("display_name", Json::from(self.display_name)),
            (
                "package_version",
                Json::from(public_text(&self.package_version, 160)),
            ),
            (
                "architecture",
                Json::from(public_text(&self.architecture, 160)),
            ),
            (
                "application_id",
                if self.registered {
                    Json::from(public_text(&self.application_id, 160))
                } else {
                    Json::Null
                },
            ),
            (
                "manifest_executable",
                if self.registered {
                    Json::from(public_text(&self.manifest_executable, 160))
                } else {
                    Json::Null
                },
            ),
            (
                "runtime_kind",
                if self.registered {
                    Json::from(self.runtime_kind)
                } else {
                    Json::Null
                },
            ),
        ])
    }
}

fn fail_discovery<T>(message: &str) -> Result<T> {
    Err(Error::new(DISCOVERY_ERROR, message))
}

fn string_field(record: &Json, key: &str, optional: bool, nullable: bool) -> Result<String> {
    match record.get(key) {
        None if optional => Ok(String::new()),
        Some(value) if nullable && value.is_null() => Ok(String::new()),
        Some(Json::String(text)) if !text.contains('\0') => Ok(text.clone()),
        Some(Json::String(text)) => {
            let _ = text;
            fail_discovery(&format!("Embedded NUL in field: {key}"))
        }
        Some(_) => fail_discovery(&format!("Invalid string field: {key}")),
        None => fail_discovery(&format!("Invalid string field: {key}")),
    }
}

fn ascii_equals(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn ascii_trimmed(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_ascii_whitespace())
}

fn supported(name: &str) -> bool {
    name == "OpenAI.Codex" || name == "OpenAI.ChatGPT-Desktop"
}

fn runtime_of(app: &Json) -> Result<&'static str> {
    let entry_point = string_field(app, "entry_point", true, true)?;
    if ascii_equals(&entry_point, "Windows.FullTrustApplication") {
        return Ok("full_trust_desktop");
    }
    if ascii_trimmed(&entry_point).is_empty()
        && ascii_trimmed(&string_field(app, "runtime_behavior", true, true)?).is_empty()
        && ascii_trimmed(&string_field(app, "trust_level", true, true)?).is_empty()
    {
        return Ok("unknown");
    }
    Ok("app_container")
}

fn to_forward_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

fn base_name(forward_path: &str) -> &str {
    forward_path.rsplit('/').next().unwrap_or(forward_path)
}

fn validate_record(record: &Json) -> Result<()> {
    for key in ["package_name", "package_version", "install_location"] {
        string_field(record, key, false, false)?;
    }
    for key in ["package_full_name", "package_family_name", "architecture"] {
        string_field(record, key, true, false)?;
    }
    if let Some(applications) = record.get("applications") {
        let items = applications
            .as_array()
            .ok_or_else(|| Error::new(DISCOVERY_ERROR, "Applications must be an array"))?;
        if items.len() > 16 {
            return fail_discovery("Too many application records");
        }
        for value in items {
            if !value.is_object() {
                return fail_discovery("Application must be an object");
            }
            for key in ["application_id", "manifest_executable", "entry_point"] {
                string_field(value, key, true, true)?;
            }
            for key in ["runtime_behavior", "trust_level"] {
                string_field(value, key, true, true)?;
            }
        }
    }
    Ok(())
}

/// Trust rule for one manifest application record of the given package.
fn trusted_candidate(package_name: &str, app: &Json) -> Result<bool> {
    let manifest = string_field(app, "manifest_executable", true, true)?;
    let relative = to_forward_slashes(&manifest);
    Ok(if package_name == "OpenAI.Codex" {
        string_field(app, "application_id", true, true)? == "App"
            && ascii_equals(&relative, "app/ChatGPT.exe")
    } else {
        runtime_of(app)? == "full_trust_desktop"
            && ascii_equals(base_name(&relative), "ChatGPT.exe")
    })
}

fn registered_desktop(record: &Json, fs: &dyn FsProbe) -> Result<Desktop> {
    let package_name = string_field(record, "package_name", false, false)?;
    let is_codex = package_name == "OpenAI.Codex";
    let mut desktop = Desktop {
        product: if is_codex {
            "chat_gpt"
        } else {
            "chat_gpt_classic"
        },
        display_name: if is_codex {
            "ChatGPT Desktop"
        } else {
            "ChatGPT Desktop (Classic)"
        },
        package_name: package_name.clone(),
        package_version: string_field(record, "package_version", false, false)?,
        architecture: ascii_trimmed(&string_field(record, "architecture", true, false)?).to_owned(),
        package_full_name: ascii_trimmed(&string_field(record, "package_full_name", true, false)?)
            .to_owned(),
        package_family_name: ascii_trimmed(&string_field(
            record,
            "package_family_name",
            true,
            false,
        )?)
        .to_owned(),
        ..Desktop::default()
    };
    if desktop.architecture.is_empty() {
        desktop.architecture = "unknown".to_owned();
    }
    if desktop.package_full_name.is_empty() || desktop.package_family_name.is_empty() {
        return Err(Error::new(
            "APPX_METADATA_INCOMPLETE",
            "Package identity metadata is missing",
        ));
    }
    let root = PathBuf::from(string_field(record, "install_location", false, false)?);
    if !root.is_absolute() || !fs.is_directory(&root) {
        return Err(Error::new(
            "APPX_INSTALL_LOCATION_INVALID",
            "Package installation directory is unavailable",
        ));
    }
    let Some(canonical_root) = fs.canonical_directory(&root) else {
        return Err(Error::new(
            "APPX_INSTALL_LOCATION_INVALID",
            "Package installation directory is unavailable",
        ));
    };
    desktop.install_location = canonical_root;
    let mut candidates: Vec<&Json> = Vec::new();
    if let Some(Json::Array(applications)) = record.get("applications") {
        for value in applications {
            if trusted_candidate(&package_name, value)? {
                candidates.push(value);
            }
        }
    }
    if candidates.is_empty() {
        return Err(Error::new(
            "APPX_APPLICATION_MISSING",
            "No trusted Desktop application was found",
        ));
    }
    if candidates.len() != 1 {
        return Err(Error::new(
            "APPX_APPLICATION_AMBIGUOUS",
            "Multiple trusted Desktop applications were found",
        ));
    }
    let app = candidates[0];
    desktop.application_id =
        ascii_trimmed(&string_field(app, "application_id", true, false)?).to_owned();
    desktop.manifest_executable =
        ascii_trimmed(&string_field(app, "manifest_executable", true, false)?).to_owned();
    if desktop.application_id.is_empty() || desktop.manifest_executable.is_empty() {
        return Err(Error::new(
            "APPX_METADATA_INCOMPLETE",
            "Application identity metadata is missing",
        ));
    }
    let relative = to_forward_slashes(&desktop.manifest_executable);
    if relative.starts_with('/') {
        return Err(Error::new(
            "APPX_EXECUTABLE_INVALID",
            "Manifest executable must remain within the package directory",
        ));
    }
    if relative.contains(':') {
        return Err(Error::new(
            "APPX_EXECUTABLE_INVALID",
            "Manifest executable must remain within the package directory",
        ));
    }
    let bytes = relative.as_bytes();
    for position in 0..bytes.len().saturating_sub(1) {
        if bytes[position] == b'.'
            && bytes[position + 1] == b'.'
            && (position + 2 == bytes.len() || bytes[position + 2] == b'/')
        {
            return Err(Error::new(
                "APPX_EXECUTABLE_INVALID",
                "Manifest executable must remain within the package directory",
            ));
        }
    }
    let executable_path = desktop.install_location.join(&relative);
    if !executable_path.is_absolute() || !fs.is_regular_file(&executable_path) {
        return Err(Error::new(
            "CODEX_EXECUTABLE_MISSING",
            "Desktop executable is not an existing absolute file",
        ));
    }
    let Some(executable) = fs.canonical_file(&executable_path) else {
        return Err(Error::new(
            "CODEX_EXECUTABLE_INVALID",
            "Cannot resolve Desktop executable",
        ));
    };
    if !fs.path_within(&desktop.install_location, &executable) {
        return Err(Error::new(
            "APPX_EXECUTABLE_INVALID",
            "Manifest executable resolves outside the package directory",
        ));
    }
    desktop.executable = executable;
    desktop.aumid = format!("{}!{}", desktop.package_family_name, desktop.application_id);
    desktop.runtime_kind = runtime_of(app)?;
    if desktop.runtime_kind != "full_trust_desktop" {
        return Err(Error::new(
            "APPX_METADATA_INCOMPLETE",
            "Selected Desktop has no explicit FullTrust entry point",
        ));
    }
    desktop.registered = true;
    desktop.discovery_source = "appx_manifest";
    Ok(desktop)
}

/// Port of `parseDiscovery` — pure selection over an envelope plus injected
/// filesystem probes. `override_executable` is empty in the normal path.
pub fn parse_discovery(
    json: &str,
    override_executable: &Path,
    fs: &dyn FsProbe,
) -> Result<Desktop> {
    let envelope = parse_strict_json_object(json.as_bytes(), DISCOVERY_LIMIT, DISCOVERY_ERROR)?;
    let schema = envelope.at("schema_version");
    let schema_ok = match schema {
        Json::Int(value) => *value == 1,
        Json::UInt(value) => *value == 1,
        Json::Double(value) => *value == 1.0,
        _ => false,
    };
    if !schema_ok {
        return Err(Error::new(
            "APPX_DISCOVERY_PROTOCOL_UNSUPPORTED",
            "Unsupported APPX discovery schema",
        ));
    }
    let Json::Array(records) = envelope.at("records") else {
        return fail_discovery("Discovery records must be an array");
    };
    if records.len() > 16 {
        return fail_discovery("Too many package records");
    }
    for record in records {
        if !record.is_object() {
            return fail_discovery("Package record must be an object");
        }
        validate_record(record)?;
    }
    if !override_executable.as_os_str().is_empty() {
        let executable =
            if override_executable.is_absolute() && fs.is_regular_file(override_executable) {
                fs.canonical_file(override_executable)
            } else {
                None
            };
        let Some(executable) = executable else {
            return Err(Error::new(
                "CODEX_EXECUTABLE_MISSING",
                "Desktop executable is not an existing absolute file",
            ));
        };
        for record in records {
            let name = record.string_at("package_name");
            if !supported(&name) {
                continue;
            }
            let root = PathBuf::from(record.string_at("install_location"));
            let canonical_root = if root.as_os_str().is_empty() {
                None
            } else {
                fs.canonical_directory(&root)
            };
            let Some(canonical_root) = canonical_root else {
                continue;
            };
            if !fs.path_within(&canonical_root, &executable) {
                continue;
            }
            let mut result = registered_desktop(record, fs)?;
            if !fs.same_normalized_path(&executable, &result.executable) {
                return Err(Error::new(
                    "APPX_OVERRIDE_INVALID",
                    "Override is inside a package but is not its registered Desktop entry",
                ));
            }
            result.discovery_source = "executable_override";
            return Ok(result);
        }
        let normalized = executable
            .to_string_lossy()
            .to_ascii_lowercase()
            .replace('\\', "/");
        if normalized.contains("/windowsapps/") {
            return Err(Error::new(
                "APPX_OVERRIDE_INVALID",
                "Package executable requires registered discovery metadata",
            ));
        }
        let mut result = Desktop {
            product: "executable_override",
            display_name: "Desktop executable override",
            package_name: "executable_override".to_owned(),
            package_version: "manual".to_owned(),
            architecture: "manual".to_owned(),
            discovery_source: "executable_override",
            executable: executable.clone(),
            install_location: executable
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            ..Desktop::default()
        };
        result.registered = false;
        return Ok(result);
    }
    for name in ["OpenAI.Codex", "OpenAI.ChatGPT-Desktop"] {
        for record in records {
            if record.string_at("package_name") == name {
                return registered_desktop(record, fs);
            }
        }
    }
    Err(Error::new(
        "CODEX_NOT_INSTALLED",
        "No supported Desktop package is installed",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FakeFs {
        directories: Vec<PathBuf>,
        files: Vec<PathBuf>,
    }

    impl FakeFs {
        fn new() -> Self {
            FakeFs {
                directories: vec![PathBuf::from(r"C:\Install")],
                files: vec![PathBuf::from(r"C:\Install\app\ChatGPT.exe")],
            }
        }
        fn with(mut self, directory: &str, file: &str) -> Self {
            self.directories.push(PathBuf::from(directory));
            self.files.push(PathBuf::from(file));
            self
        }
    }

    impl FsProbe for FakeFs {
        fn is_directory(&self, path: &Path) -> bool {
            self.directories.iter().any(|d| d == path)
        }
        fn is_regular_file(&self, path: &Path) -> bool {
            self.files.iter().any(|f| f == path)
        }
        fn canonical_file(&self, path: &Path) -> Option<PathBuf> {
            self.files.iter().find(|f| f == &path).cloned()
        }
        fn canonical_directory(&self, path: &Path) -> Option<PathBuf> {
            self.directories.iter().find(|d| d == &path).cloned()
        }
        fn same_normalized_path(&self, left: &Path, right: &Path) -> bool {
            left.to_string_lossy().to_ascii_lowercase()
                == right.to_string_lossy().to_ascii_lowercase()
        }
        fn path_within(&self, root: &Path, child: &Path) -> bool {
            let root = root
                .to_string_lossy()
                .to_ascii_lowercase()
                .replace('\\', "/");
            let child = child
                .to_string_lossy()
                .to_ascii_lowercase()
                .replace('\\', "/");
            child == root || child.starts_with(&format!("{root}/"))
        }
    }

    fn envelope(applications: &str) -> String {
        format!(
            r#"{{"schema_version":1,"records":[{{"package_name":"OpenAI.Codex","package_version":"1.2.3.4","package_full_name":"OpenAI.Codex_2p2nqsd0c76g0","package_family_name":"OpenAI.Codex_2p2nqsd0c76g0","architecture":"X64","install_location":"C:\\Install","applications":[{applications}]}}]}}"#
        )
    }

    const TRUSTED_APP: &str = r#"{"application_id":"App","manifest_executable":"app\\ChatGPT.exe","entry_point":"Windows.FullTrustApplication","runtime_behavior":null,"trust_level":null}"#;

    #[test]
    fn selects_trusted_codex_application() {
        let json = envelope(TRUSTED_APP);
        let desktop = parse_discovery(&json, Path::new(""), &FakeFs::new()).unwrap();
        assert!(desktop.registered);
        assert_eq!(desktop.aumid, "OpenAI.Codex_2p2nqsd0c76g0!App");
        assert_eq!(desktop.runtime_kind, "full_trust_desktop");
        assert_eq!(
            desktop.executable,
            PathBuf::from(r"C:\Install\app\ChatGPT.exe")
        );
    }

    #[test]
    fn rejects_untrusted_applications() {
        // Wrong application id for OpenAI.Codex.
        let json = envelope(
            r#"{"application_id":"Other","manifest_executable":"app\\ChatGPT.exe","entry_point":"Windows.FullTrustApplication"}"#,
        );
        let error = parse_discovery(&json, Path::new(""), &FakeFs::new()).unwrap_err();
        assert_eq!(error.code, "APPX_APPLICATION_MISSING");
        // Command runner style app must never be selected.
        let json = envelope(
            r#"{"application_id":"CodexCoreCommandRunner","manifest_executable":"app\\ChatGPT.exe","entry_point":"Windows.FullTrustApplication"}"#,
        );
        assert_eq!(
            parse_discovery(&json, Path::new(""), &FakeFs::new())
                .unwrap_err()
                .code,
            "APPX_APPLICATION_MISSING"
        );
        // AppContainer entry point is not FullTrust.
        let json = envelope(
            r#"{"application_id":"App","manifest_executable":"app\\ChatGPT.exe","entry_point":"partial"}"#,
        );
        assert_eq!(
            parse_discovery(&json, Path::new(""), &FakeFs::new())
                .unwrap_err()
                .code,
            "APPX_METADATA_INCOMPLETE"
        );
    }

    #[test]
    fn rejects_escape_attempts_in_manifest_executable() {
        // OpenAI.Codex only trusts the exact "app/ChatGPT.exe" spelling, so a
        // traversal variant fails candidate selection first.
        let json = envelope(
            r#"{"application_id":"App","manifest_executable":"app/../app/ChatGPT.exe","entry_point":"Windows.FullTrustApplication"}"#,
        );
        assert_eq!(
            parse_discovery(&json, Path::new(""), &FakeFs::new())
                .unwrap_err()
                .code,
            "APPX_APPLICATION_MISSING"
        );
        // The Classic rule matches by basename, so containment must reject
        // traversal, absolute and drive-suffixed spellings.
        let classic = |bad: &str| {
            format!(
                r#"{{"schema_version":1,"records":[{{"package_name":"OpenAI.ChatGPT-Desktop","package_version":"1.0","package_full_name":"OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0","package_family_name":"OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0","architecture":"X64","install_location":"C:\\Install","applications":[{{"application_id":"App","manifest_executable":"{bad}","entry_point":"Windows.FullTrustApplication"}}]}}]}}"#
            )
        };
        for bad in ["app/../ChatGPT.exe", "/abs/ChatGPT.exe", "C:/x/ChatGPT.exe"] {
            let error = parse_discovery(&classic(bad), Path::new(""), &FakeFs::new()).unwrap_err();
            assert_eq!(error.code, "APPX_EXECUTABLE_INVALID", "{bad}");
        }
    }

    #[test]
    fn classic_package_uses_fulltrust_basename_rule() {
        let json = r#"{"schema_version":1,"records":[{"package_name":"OpenAI.ChatGPT-Desktop","package_version":"1.0","package_full_name":"OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0","package_family_name":"OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0","architecture":"X64","install_location":"C:\\Install","applications":[{"application_id":"App","manifest_executable":"ChatGPT.exe","entry_point":"Windows.FullTrustApplication"}]}]}"#.to_string();
        let desktop = parse_discovery(
            &json,
            Path::new(""),
            &FakeFs::new().with(r"C:\Install", r"C:\Install\ChatGPT.exe"),
        )
        .unwrap();
        assert_eq!(desktop.product, "chat_gpt_classic");
    }

    #[test]
    fn not_installed_when_no_supported_package() {
        let json = r#"{"schema_version":1,"records":[]}"#;
        let error = parse_discovery(json, Path::new(""), &FakeFs::new()).unwrap_err();
        assert_eq!(error.code, "CODEX_NOT_INSTALLED");
    }

    #[test]
    fn override_resolution_prefers_registered_entry() {
        let json = envelope(TRUSTED_APP);
        let desktop = parse_discovery(
            &json,
            Path::new(r"C:\Install\app\ChatGPT.exe"),
            &FakeFs::new(),
        )
        .unwrap();
        assert!(desktop.registered);
        assert_eq!(desktop.discovery_source, "executable_override");

        let outside = FakeFs::new().with(r"C:\Tools", r"C:\Tools\Custom.exe");
        let desktop = parse_discovery(&json, Path::new(r"C:\Tools\Custom.exe"), &outside).unwrap();
        assert!(!desktop.registered);
        assert_eq!(desktop.discovery_source, "executable_override");
    }

    #[test]
    fn protocol_envelope_is_strict() {
        let fs = FakeFs::new();
        assert!(parse_discovery("[]", Path::new(""), &fs).is_err());
        assert!(
            parse_discovery(r#"{"schema_version":2,"records":[]}"#, Path::new(""), &fs).is_err()
        );
        assert!(
            parse_discovery(r#"{"schema_version":1,"records":{}}"#, Path::new(""), &fs).is_err()
        );
        let duplicate = r#"{"schema_version":1,"schema_version":1,"records":[]}"#;
        assert!(parse_discovery(duplicate, Path::new(""), &fs).is_err());
    }
}
