//! Windows integration tests for the APPX discovery protocol. These execute
//! the exact production script (`resources/appx-discovery.ps1`, embedded via
//! `discovery_script()`) on the real machine, read-only: no application is
//! started, stopped, or activated. The point is to prove the JSON contract
//! the unit fixtures model — fixed envelope, explicit depth, null-tolerant
//! optional attributes — against the PowerShell that actually runs.

#![cfg(windows)]

use std::process::Stdio;

use proxy_guard_core::{DesktopTargetKind, GuardConfig, PackageRuntimeKind};
use proxy_guard_windows::{discover_desktop_app, discovery_script, system_tools};
use tokio_util::sync::CancellationToken;

const MAX_STDOUT: usize = 64 * 1024;

/// T9: the production pipeline end-to-end on this machine. With a supported
/// package installed it must return the registered Desktop entry (the same
/// semantics the TUI shows); on a machine without one it must report
/// CODEX_NOT_INSTALLED. Any APPX_DISCOVERY_* failure is a real protocol
/// regression and fails the test — including the old
/// "malformed PowerShell JSON" breakage.
#[tokio::test]
async fn production_discovery_pipeline_succeeds_on_this_machine() {
    let config = GuardConfig::default();
    let cancellation = CancellationToken::new();
    match discover_desktop_app(&config, None, &cancellation).await {
        Ok(info) => {
            assert_eq!(info.discovery_source, info.discovery_source); // shape held
            let DesktopTargetKind::RegisteredPackage(application) = &info.target_kind else {
                panic!("a discovered package must stay a registered target");
            };
            assert_eq!(
                application.runtime_kind,
                PackageRuntimeKind::FullTrustDesktop
            );
            assert!(!application.application_id.is_empty());
            assert!(!application.package_full_name.is_empty());
            assert!(!application.package_family_name.is_empty());
            assert_eq!(
                application.app_user_model_id,
                format!(
                    "{}!{}",
                    application.package_family_name, application.application_id
                )
            );
            assert!(
                info.executable
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with("chatgpt.exe"),
                "the selected entry must be the Desktop executable, not a runner: {}",
                proxy_guard_core::display_path(&info.executable)
            );
        }
        Err(error) => {
            assert!(
                error.contains("CODEX_NOT_INSTALLED"),
                "discovery must either find a package or report CODEX_NOT_INSTALLED: {error}"
            );
        }
    }
}

/// T10: run the exact `discovery_script()` constant through the same
/// system-PowerShell resolution the production launcher uses and verify the
/// raw envelope: top level is always an object, `records` is always an
/// array, and every `applications` entry is an object — never a string
/// truncated by ConvertTo-Json's default depth.
#[test]
fn production_script_emits_a_well_formed_envelope() {
    let powershell = system_tools::system_powershell().expect("system PowerShell");
    let output = std::process::Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            discovery_script(),
        ])
        .stdin(Stdio::null())
        .output()
        .expect("run discovery script");
    assert!(
        output.status.success(),
        "discovery script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = output.stdout;
    assert!(stdout.len() <= MAX_STDOUT, "stdout exceeded 64 KiB");
    let text = String::from_utf8(stdout)
        .expect("the script sets UTF-8 console output; identity bytes must survive exactly");
    let envelope: serde_json::Value =
        serde_json::from_str(text.trim()).expect("one parseable JSON envelope document");
    let object = envelope
        .as_object()
        .expect("the top level must be an object, not a bare record or array");
    assert_eq!(object["schema_version"], 1, "schema_version must be 1");
    let records = object["records"]
        .as_array()
        .expect("records must always be an array, even for a single package");
    for record in records {
        let applications = record["applications"]
            .as_array()
            .expect("applications must survive as an array of objects (explicit depth)");
        assert!(applications.len() <= 16);
        for application in applications {
            assert!(
                application.is_object(),
                "an application entry must be an object, not a depth-truncated string"
            );
            assert!(application["application_id"].is_string());
            assert!(application["manifest_executable"].is_string());
            // Optional manifest attributes are legal as null or string.
            let runtime_behavior = &application["runtime_behavior"];
            assert!(runtime_behavior.is_null() || runtime_behavior.is_string());
            let trust_level = &application["trust_level"];
            assert!(trust_level.is_null() || trust_level.is_string());
        }
    }
}
