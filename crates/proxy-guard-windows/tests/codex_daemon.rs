//! Regression tests for the launch pipeline and the explicit daemon repair
//! path, driven by the `fake-codex-cli` fixture binary. These tests never touch
//! a real Codex daemon or Desktop; every child is a fixture under test control.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use proxy_guard_core::{
    DaemonPreparation, DesktopAppInfo, DesktopDiscoverySource, DesktopProduct, DesktopTargetKind,
    GuardConfig, LaunchOptions, PackageApplication, PackageRuntimeKind,
};
use proxy_guard_windows::{
    CodexCli, DaemonStopBudget, LaunchHooks, launch_codex_with, stop_codex_daemon,
};
use tokio_util::sync::CancellationToken;

/// Serializes tests that read or write process-wide environment variables
/// (which the fixture children inherit) and tests that acquire the
/// cross-process startup lock. Everything here is a launch/stop test, so one
/// lock keeps the whole file deterministic.
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const STOP_JSON: &str = r#"{"status":"stopped"}"#;
const NOT_RUNNING_JSON: &str = r#"{"status":"notRunning"}"#;

fn fake_cli_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-codex-cli"))
}

fn temp_dir(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("cpg it {label} {unique} {}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    root
}

fn desktop_info(executable: &Path) -> DesktopAppInfo {
    DesktopAppInfo {
        product: DesktopProduct::ExecutableOverride,
        package_name: "fixture".into(),
        package_version: "test".into(),
        architecture: "test".into(),
        discovery_source: DesktopDiscoverySource::ExecutableOverride,
        target_kind: DesktopTargetKind::UnpackagedExecutable,
        install_location: executable.parent().unwrap().to_path_buf(),
        executable: executable.to_path_buf(),
    }
}

fn config_with_cli_override(override_path: &Path) -> GuardConfig {
    let mut config = GuardConfig::default();
    config.codex.cli_executable_override = override_path.to_path_buf();
    config
}

fn set_env(name: &str, value: &str) {
    unsafe { std::env::set_var(name, value) };
}

fn remove_env(name: &str) {
    unsafe { std::env::remove_var(name) };
}

/// Sets the fixture variables for one scenario and clears them at the end.
async fn with_fixture_env(
    desktop_touch: &Path,
    vars: &[(&str, &str)],
    test: impl std::future::Future<Output = ()>,
) {
    let _guard = TEST_LOCK.lock().await;
    set_env("FAKE_DESKTOP_TOUCH", desktop_touch.to_str().unwrap());
    for (name, value) in vars {
        set_env(name, value);
    }
    test.await;
    for (name, _) in vars {
        remove_env(name);
    }
    remove_env("FAKE_DESKTOP_TOUCH");
}

async fn wait_for_file(path: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !path.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        path.exists(),
        "fixture marker never appeared: {}",
        path.display()
    );
}

fn launch_hooks<'a>(
    resolver: &'a (dyn Fn(&GuardConfig) -> Result<Option<CodexCli>, String> + Sync),
) -> LaunchHooks<'a> {
    LaunchHooks {
        resolve_cli: resolver,
        stop_budget: DaemonStopBudget {
            total: Duration::from_secs(30),
            cleanup: Duration::from_secs(5),
        },
        // Serialized launches in tests must not trip the production anti-race
        // window left by the previous test's spawn.
        post_spawn_busy_window: Duration::from_millis(0),
    }
}

fn real_resolver(config: &GuardConfig) -> Result<Option<CodexCli>, String> {
    proxy_guard_windows::resolve_codex_cli(config)
}

#[tokio::test]
async fn normal_launch_never_invokes_the_codex_cli() {
    let root = temp_dir("a1");
    let stop_marker = root.join("stop-invoked");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(
        &desktop_marker,
        &[("FAKE_CLI_TOUCH", stop_marker.to_str().unwrap())],
        async {
            // The CLI override points at the fixture, so a resolver call would
            // be visible via the stop marker.
            let config = config_with_cli_override(&fake_cli_exe());
            let info = desktop_info(&fake_cli_exe());
            let receipt = launch_codex_with(
                &info,
                &config,
                LaunchOptions::default(),
                &CancellationToken::new(),
                &launch_hooks(&real_resolver),
            )
            .await
            .unwrap();
            assert_eq!(receipt.daemon_preparation, DaemonPreparation::Skipped);
            assert_eq!(
                receipt.launch_method,
                proxy_guard_core::LaunchMethod::NativeProcess
            );
            assert_eq!(
                receipt.proxy_delivery,
                proxy_guard_core::ProxyDelivery::ProcessEnvironment
            );
            assert_eq!(
                receipt.backend_proxy_config,
                proxy_guard_core::BackendProxyConfig::NotApplicable
            );
            assert!(
                !stop_marker.exists(),
                "normal launch must not run any daemon command"
            );
            wait_for_file(&desktop_marker).await;
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn activation_only_is_refused_for_unpackaged_targets_without_spawning() {
    let root = temp_dir("activation-only");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(&desktop_marker, &[], async {
        let config = config_with_cli_override(&fake_cli_exe());
        let info = desktop_info(&fake_cli_exe());
        let error = launch_codex_with(
            &info,
            &config,
            LaunchOptions {
                refresh_codex_daemon: false,
                activation_only: true,
            },
            &CancellationToken::new(),
            &launch_hooks(&real_resolver),
        )
        .await
        .unwrap_err();
        assert!(error.starts_with("ACTIVATION_ONLY_UNSUPPORTED:"), "{error}");
        assert!(
            !desktop_marker.exists(),
            "an activation-only request must never spawn an unpackaged EXE"
        );
    })
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn registered_repair_without_consent_is_refused_before_the_daemon_stop() {
    let root = temp_dir("package-preflight");
    let stop_marker = root.join("stop-invoked");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(
        &desktop_marker,
        &[("FAKE_CLI_TOUCH", stop_marker.to_str().unwrap())],
        async {
            let config = config_with_cli_override(&fake_cli_exe());
            let mut info = desktop_info(&fake_cli_exe());
            info.target_kind = DesktopTargetKind::RegisteredPackage(PackageApplication {
                package_full_name: "Fixture_1.0.0.0_x64__test".into(),
                package_family_name: "Fixture_test".into(),
                application_id: "App".into(),
                app_user_model_id: "Fixture_test!App".into(),
                manifest_executable: "app/ChatGPT.exe".into(),
                runtime_kind: PackageRuntimeKind::FullTrustDesktop,
            });
            // A registered repair without the authorized backend proxy block
            // is refused deterministically before the daemon stop and before
            // any process creation: the stop would interrupt shared work and
            // establish nothing.
            let error = launch_codex_with(
                &info,
                &config,
                LaunchOptions {
                    refresh_codex_daemon: true,
                    activation_only: false,
                },
                &CancellationToken::new(),
                &launch_hooks(&real_resolver),
            )
            .await
            .unwrap_err();
            assert!(
                error.starts_with("BACKEND_PROXY_REQUIRED_FOR_REPAIR:"),
                "{error}"
            );
            assert!(
                !stop_marker.exists(),
                "an unauthorized registered repair must never run daemon commands"
            );
            assert!(
                !desktop_marker.exists(),
                "a registered-package launch must never spawn a bare EXE"
            );
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn registered_repair_prepares_the_home_block_before_stopping_the_daemon() {
    let root = temp_dir("repair-order");
    let stop_marker = root.join("stop-invoked");
    let desktop_marker = root.join("desktop-spawned");
    let home = root.join("authorized-home");
    fs::create_dir_all(&home).unwrap();
    // A conflicting proxy key outside Guard's block: the prepare step must
    // fail closed, and — the point of this test — the shared daemon must
    // still not have been stopped.
    fs::write(home.join(".env"), "HTTPS_PROXY=http://elsewhere:1\n").unwrap();
    with_fixture_env(
        &desktop_marker,
        &[("FAKE_CLI_TOUCH", stop_marker.to_str().unwrap())],
        async {
            let mut config = config_with_cli_override(&fake_cli_exe());
            config.codex.manage_codex_proxy_env = true;
            config.codex.proxy_env_home = home.clone();
            let mut info = desktop_info(&fake_cli_exe());
            info.target_kind = DesktopTargetKind::RegisteredPackage(PackageApplication {
                package_full_name: "Fixture_1.0.0.0_x64__test".into(),
                package_family_name: "Fixture_test".into(),
                application_id: "App".into(),
                app_user_model_id: "Fixture_test!App".into(),
                manifest_executable: "app/ChatGPT.exe".into(),
                runtime_kind: PackageRuntimeKind::FullTrustDesktop,
            });
            let error = launch_codex_with(
                &info,
                &config,
                LaunchOptions {
                    refresh_codex_daemon: true,
                    activation_only: false,
                },
                &CancellationToken::new(),
                &launch_hooks(&real_resolver),
            )
            .await
            .unwrap_err();
            assert!(
                error.starts_with("BACKEND_PROXY_CONFIG_CONFLICT:"),
                "{error}"
            );
            assert!(
                !stop_marker.exists(),
                "a failed .env preparation must never leave the shared daemon stopped"
            );
            assert!(!desktop_marker.exists(), "nothing may have been launched");
            assert_eq!(
                fs::read_to_string(home.join(".env")).unwrap(),
                "HTTPS_PROXY=http://elsewhere:1\n",
                "the conflicting file must be untouched"
            );
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn activation_only_with_daemon_repair_is_refused_as_invalid_options() {
    let root = temp_dir("invalid-options");
    let stop_marker = root.join("stop-invoked");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(
        &desktop_marker,
        &[("FAKE_CLI_TOUCH", stop_marker.to_str().unwrap())],
        async {
            let config = config_with_cli_override(&fake_cli_exe());
            let info = desktop_info(&fake_cli_exe());
            let error = launch_codex_with(
                &info,
                &config,
                LaunchOptions {
                    refresh_codex_daemon: true,
                    activation_only: true,
                },
                &CancellationToken::new(),
                &launch_hooks(&real_resolver),
            )
            .await
            .unwrap_err();
            assert!(error.starts_with("INVALID_LAUNCH_OPTIONS:"), "{error}");
            assert!(!stop_marker.exists(), "no daemon command may run");
            assert!(!desktop_marker.exists(), "nothing may be launched");
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn repair_launch_stops_the_daemon_once_then_launches() {
    let root = temp_dir("a2-stopped");
    let stop_marker = root.join("stop-invoked");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(
        &desktop_marker,
        &[
            ("FAKE_CLI_TOUCH", stop_marker.to_str().unwrap()),
            ("FAKE_CLI_STDOUT", STOP_JSON),
        ],
        async {
            let config = config_with_cli_override(&fake_cli_exe());
            let info = desktop_info(&fake_cli_exe());
            let receipt = launch_codex_with(
                &info,
                &config,
                LaunchOptions {
                    refresh_codex_daemon: true,
                    activation_only: false,
                },
                &CancellationToken::new(),
                &launch_hooks(&real_resolver),
            )
            .await
            .unwrap();
            assert_eq!(receipt.daemon_preparation, DaemonPreparation::Stopped);
            assert!(stop_marker.exists(), "the lifecycle stop ran exactly once");
            wait_for_file(&desktop_marker).await;
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn repair_launch_maps_not_running_to_not_needed() {
    let root = temp_dir("a2-notrunning");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(
        &desktop_marker,
        &[("FAKE_CLI_STDOUT", NOT_RUNNING_JSON)],
        async {
            let config = config_with_cli_override(&fake_cli_exe());
            let info = desktop_info(&fake_cli_exe());
            let receipt = launch_codex_with(
                &info,
                &config,
                LaunchOptions {
                    refresh_codex_daemon: true,
                    activation_only: false,
                },
                &CancellationToken::new(),
                &launch_hooks(&real_resolver),
            )
            .await
            .unwrap();
            assert_eq!(receipt.daemon_preparation, DaemonPreparation::NotNeeded);
            wait_for_file(&desktop_marker).await;
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn repair_blocks_when_no_cli_is_available() {
    let root = temp_dir("a3-missing");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(&desktop_marker, &[], async {
        let config = GuardConfig::default();
        let info = desktop_info(&fake_cli_exe());
        let error = launch_codex_with(
            &info,
            &config,
            LaunchOptions {
                refresh_codex_daemon: true,
                activation_only: false,
            },
            &CancellationToken::new(),
            &launch_hooks(&|_config| Ok(None)),
        )
        .await
        .unwrap_err();
        assert!(error.contains("CODEX_CLI_UNAVAILABLE"), "{error}");
        assert!(!desktop_marker.exists(), "no Desktop spawn on failure");
    })
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn repair_blocks_on_invalid_override_without_fallback() {
    let root = temp_dir("a3-override");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(&desktop_marker, &[], async {
        let config = config_with_cli_override(Path::new(r"Z:\missing\codex.exe"));
        let info = desktop_info(&fake_cli_exe());
        let error = launch_codex_with(
            &info,
            &config,
            LaunchOptions {
                refresh_codex_daemon: true,
                activation_only: false,
            },
            &CancellationToken::new(),
            &launch_hooks(&real_resolver),
        )
        .await
        .unwrap_err();
        assert!(error.contains("CODEX_CLI_OVERRIDE_INVALID"), "{error}");
        assert!(!desktop_marker.exists());
    })
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn repair_blocks_on_unsupported_daemon_command() {
    let root = temp_dir("a3-unsupported");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(
        &desktop_marker,
        &[
            ("FAKE_CLI_EXIT", "2"),
            ("FAKE_CLI_STDERR", "error: unrecognized subcommand 'daemon'"),
        ],
        async {
            let config = config_with_cli_override(&fake_cli_exe());
            let info = desktop_info(&fake_cli_exe());
            let error = launch_codex_with(
                &info,
                &config,
                LaunchOptions {
                    refresh_codex_daemon: true,
                    activation_only: false,
                },
                &CancellationToken::new(),
                &launch_hooks(&real_resolver),
            )
            .await
            .unwrap_err();
            assert!(error.contains("CODEX_DAEMON_UNSUPPORTED"), "{error}");
            assert!(!desktop_marker.exists());
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn repair_blocks_on_unknown_or_running_status() {
    let root = temp_dir("d2-status");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(
        &desktop_marker,
        &[("FAKE_CLI_STDOUT", r#"{"status":"running"}"#)],
        async {
            let config = config_with_cli_override(&fake_cli_exe());
            let info = desktop_info(&fake_cli_exe());
            let error = launch_codex_with(
                &info,
                &config,
                LaunchOptions {
                    refresh_codex_daemon: true,
                    activation_only: false,
                },
                &CancellationToken::new(),
                &launch_hooks(&real_resolver),
            )
            .await
            .unwrap_err();
            assert!(error.contains("CODEX_DAEMON_STOP_FAILED"), "{error}");
            assert!(!desktop_marker.exists());
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cancelled_token_blocks_before_any_spawn_or_stop() {
    let root = temp_dir("c1");
    let stop_marker = root.join("stop-invoked");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(
        &desktop_marker,
        &[("FAKE_CLI_TOUCH", stop_marker.to_str().unwrap())],
        async {
            let config = config_with_cli_override(&fake_cli_exe());
            let info = desktop_info(&fake_cli_exe());
            let cancellation = CancellationToken::new();
            cancellation.cancel();
            for options in [
                LaunchOptions::default(),
                LaunchOptions {
                    refresh_codex_daemon: true,
                    activation_only: false,
                },
            ] {
                let error = launch_codex_with(
                    &info,
                    &config,
                    options,
                    &cancellation,
                    &launch_hooks(&real_resolver),
                )
                .await
                .unwrap_err();
                assert!(error.contains("LAUNCH_CANCELLED"), "{error}");
            }
            assert!(!stop_marker.exists());
            assert!(!desktop_marker.exists());
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cancel_during_stop_reports_unconfirmed_state_and_never_spawns() {
    let root = temp_dir("c3");
    let stop_marker = root.join("stop-invoked");
    let desktop_marker = root.join("desktop-spawned");
    let release_file = root.join("release");
    with_fixture_env(
        &desktop_marker,
        &[
            ("FAKE_CLI_TOUCH", stop_marker.to_str().unwrap()),
            ("FAKE_CLI_WAIT_FILE", release_file.to_str().unwrap()),
        ],
        async {
            let config = config_with_cli_override(&fake_cli_exe());
            let info = desktop_info(&fake_cli_exe());
            let cancellation = CancellationToken::new();
            let token = cancellation.clone();
            let task = tokio::spawn(async move {
                launch_codex_with(
                    &info,
                    &config,
                    LaunchOptions {
                        refresh_codex_daemon: true,
                        activation_only: false,
                    },
                    &token,
                    &launch_hooks(&real_resolver),
                )
                .await
            });
            // File barrier: wait until the helper provably started, then cancel.
            wait_for_file(&stop_marker).await;
            cancellation.cancel();
            let error = task.await.unwrap().unwrap_err();
            assert!(error.contains("LAUNCH_CANCELLED"), "{error}");
            assert!(error.contains("unconfirmed"), "{error}");
            assert!(
                !desktop_marker.exists(),
                "cancellation must not start a Desktop spawn"
            );
            // Release the (already killed) helper's barrier for cleanliness.
            fs::write(&release_file, b"1").unwrap();
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

fn stop_cli() -> CodexCli {
    CodexCli {
        executable: fake_cli_exe(),
        pinned_home: None,
    }
}

fn fast_budget(total_ms: u64) -> DaemonStopBudget {
    DaemonStopBudget {
        total: Duration::from_millis(total_ms),
        cleanup: Duration::from_secs(5),
    }
}

#[tokio::test]
async fn stop_rejects_output_over_the_limit_before_parsing() {
    let root = temp_dir("d3-over");
    let desktop_marker = root.join("desktop-spawned");
    // Valid JSON followed by more than 64 KiB of padding: the reader fails on
    // the limit itself, before any JSON interpretation could rescue it.
    let padding = format!("x {}", 64 * 1024 + 16);
    with_fixture_env(
        &desktop_marker,
        &[
            ("FAKE_CLI_STDOUT", STOP_JSON),
            ("FAKE_CLI_STDOUT_BYTES", &padding),
        ],
        async {
            let error =
                stop_codex_daemon(&stop_cli(), &fast_budget(10_000), &CancellationToken::new())
                    .await
                    .unwrap_err();
            assert!(error.contains("exceeded its size limit"), "{error}");
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stop_rejects_oversized_stderr_even_with_unsupported_markers() {
    let root = temp_dir("d3-stderr");
    let desktop_marker = root.join("desktop-spawned");
    let padding = format!("x {}", 64 * 1024 + 16);
    with_fixture_env(
        &desktop_marker,
        &[
            ("FAKE_CLI_EXIT", "1"),
            ("FAKE_CLI_STDERR", "error: unrecognized subcommand 'daemon'"),
            ("FAKE_CLI_STDERR_BYTES", &padding),
        ],
        async {
            let error =
                stop_codex_daemon(&stop_cli(), &fast_budget(10_000), &CancellationToken::new())
                    .await
                    .unwrap_err();
            assert!(error.contains("exceeded its size limit"), "{error}");
            assert!(!error.contains("CODEX_DAEMON_UNSUPPORTED"), "{error}");
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stop_accepts_output_exactly_at_the_limit() {
    let root = temp_dir("d3-exact");
    let desktop_marker = root.join("desktop-spawned");
    let spaces = (64 * 1024 - STOP_JSON.len()).to_string();
    with_fixture_env(
        &desktop_marker,
        &[
            ("FAKE_CLI_STDOUT", STOP_JSON),
            ("FAKE_CLI_STDOUT_SPACES", &spaces),
        ],
        async {
            let preparation =
                stop_codex_daemon(&stop_cli(), &fast_budget(10_000), &CancellationToken::new())
                    .await
                    .unwrap();
            assert_eq!(preparation, DaemonPreparation::Stopped);
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stop_rejects_invalid_utf8_and_double_json() {
    let root = temp_dir("d2-format");
    let desktop_marker = root.join("desktop-spawned");
    with_fixture_env(&desktop_marker, &[("FAKE_CLI_STDOUT_HEX", "ff")], async {
        let error = stop_codex_daemon(&stop_cli(), &fast_budget(10_000), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.contains("not valid UTF-8"), "{error}");
    })
    .await;
    with_fixture_env(
        &desktop_marker,
        &[(
            "FAKE_CLI_STDOUT",
            r#"{"status":"stopped"}{"status":"running"}"#,
        )],
        async {
            let error =
                stop_codex_daemon(&stop_cli(), &fast_budget(10_000), &CancellationToken::new())
                    .await
                    .unwrap_err();
            assert!(error.contains("CODEX_DAEMON_STOP_FAILED"), "{error}");
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stop_times_out_reports_unconfirmed_and_kills_the_helper() {
    let root = temp_dir("d4");
    let desktop_marker = root.join("desktop-spawned");
    let stop_marker = root.join("stop-invoked");
    with_fixture_env(
        &desktop_marker,
        &[
            ("FAKE_CLI_TOUCH", stop_marker.to_str().unwrap()),
            ("FAKE_CLI_SLEEP_MS", "60000"),
        ],
        async {
            let error =
                stop_codex_daemon(&stop_cli(), &fast_budget(300), &CancellationToken::new())
                    .await
                    .unwrap_err();
            assert!(error.contains("CODEX_DAEMON_STOP_TIMEOUT"), "{error}");
            assert!(error.contains("unconfirmed"), "{error}");
            assert!(stop_marker.exists());
        },
    )
    .await;
    fs::remove_dir_all(root).unwrap();
}
