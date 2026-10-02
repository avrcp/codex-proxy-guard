mod bridge;
mod bridge_protocol;
mod build_info;
mod cli;
mod dispatcher;
mod tui;

use std::path::Path;

use anyhow::{Context, bail};
use clap::Parser;
use cli::{Cli, Command};
use dispatcher::launch_pipeline;
use proxy_guard_core::{
    AppState, ConfigReadiness, DesktopAppInfo, GuardConfig, LaunchOptions, LaunchReceipt,
    redact_text,
};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if matches!(&cli.command, Some(Command::InternalActivatePackage)) {
        // This hidden, one-shot worker path runs before user configuration,
        // the TUI, and all daemon code. It only performs the bounded native
        // activation protocol on stdin/stdout.
        return run_internal_activation_worker().await;
    }
    let config_path = cli.config.unwrap_or(GuardConfig::config_path()?);

    match cli.command {
        Some(Command::Bridge) => bridge::run(config_path).await,
        Some(Command::ConfigPath) => {
            println!("{}", config_path.display());
            Ok(())
        }
        Some(Command::BuildInfo) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&build_info::build_info())?
            );
            Ok(())
        }
        Some(Command::InitConfig {
            force,
            proxy_host,
            proxy_port,
        }) => init_config(&config_path, force, proxy_host, proxy_port),
        Some(Command::Launch {
            json,
            refresh_codex_daemon,
            activation_only,
        }) => {
            let (config, _) = GuardConfig::load_or_create(&config_path)
                .with_context(|| format!("load configuration {}", config_path.display()))?;
            let options = LaunchOptions {
                refresh_codex_daemon,
                activation_only,
            };
            let (_, receipt) = launch_command(&config, &config_path, options).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                print_receipt_summary(&receipt);
            }
            Ok(())
        }
        Some(Command::InternalActivatePackage) => unreachable!("handled before configuration"),
        None => tui::run(with_elevation_hint(tui_state(&config_path))).await,
    }
}

/// The hidden activation worker: plain stdio, no configuration, no TUI, no
/// daemon code. Exit status carries the protocol outcome: zero when a receipt
/// was produced (including a failed activation — that is a result, not an
/// error of the worker), non-zero only for pre-activation rejections, which
/// never activated anything.
async fn run_internal_activation_worker() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    proxy_guard_windows::run_activation_worker(&mut input, &mut output)
        .map_err(|error| anyhow::anyhow!(redact_text(&error)))
}

fn print_receipt_summary(receipt: &LaunchReceipt) {
    match receipt.launch_method {
        proxy_guard_core::LaunchMethod::AppmodelActivation => println!(
            "{} activated through Windows registered application activation (PID {})",
            receipt.desktop.product.display_name(),
            receipt.pid
        ),
        proxy_guard_core::LaunchMethod::NativeProcess => println!(
            "{} process created through {} (PID {})",
            receipt.desktop.product.display_name(),
            receipt
                .proxy_endpoint
                .as_deref()
                .unwrap_or("<no proxy plan>"),
            receipt.pid
        ),
    }
    match receipt.proxy_delivery {
        proxy_guard_core::ProxyDelivery::ProcessEnvironment => {}
        proxy_guard_core::ProxyDelivery::ActivationArguments => {
            println!("Chromium proxy arguments submitted; backend Codex Home config not authorized")
        }
        proxy_guard_core::ProxyDelivery::ActivationArgumentsAndHomeConfig => {
            println!("Chromium proxy arguments submitted; backend Codex Home config prepared")
        }
        proxy_guard_core::ProxyDelivery::NotEstablished => {
            println!("no proxy delivered (activation-only)")
        }
    }
    if !receipt.daemon_preparation.status_detail().is_empty() {
        println!("{}", receipt.daemon_preparation.status_detail());
    }
}

/// CLI launch with the same cancellation protocol as the TUI: Ctrl-C cancels
/// the pipeline, Guard waits for its bounded cleanup, and the exit status
/// reflects the outcome.
async fn launch_command(
    config: &GuardConfig,
    config_path: &Path,
    options: LaunchOptions,
) -> anyhow::Result<(DesktopAppInfo, LaunchReceipt)> {
    let cancellation = CancellationToken::new();
    let ctrl_c_token = cancellation.clone();
    let ctrl_c = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            ctrl_c_token.cancel();
        }
    });
    let result = launch_pipeline(config, config_path, options, &cancellation).await;
    ctrl_c.abort();
    result.map_err(|error| anyhow::anyhow!(redact_text(&error)))
}

/// Shows the elevation block up front when Guard itself runs elevated (or its
/// elevation cannot be determined). The launch pipeline enforces the same rule
/// regardless of this hint; Guard never automates UAC or privilege tricks.
fn with_elevation_hint(mut state: AppState) -> AppState {
    match proxy_guard_windows::query_elevation() {
        Ok(proxy_guard_windows::ElevationState::NotElevated) => {}
        Ok(proxy_guard_windows::ElevationState::Elevated) => {
            state.status_message = "Launch unavailable".into();
            state.error_message =
                Some(proxy_guard_windows::elevation::ELEVATED_LAUNCH_UNSUPPORTED.into());
        }
        Err(detail) => {
            state.status_message = "Launch unavailable".into();
            state.error_message = Some(format!("ELEVATION_QUERY_FAILED: {detail}"));
        }
    }
    state
}

fn tui_state(config_path: &Path) -> AppState {
    match GuardConfig::load_or_create(config_path) {
        Ok((config, created)) => {
            let mut state = AppState::new(config, config_path.into());
            state.config_readiness = ConfigReadiness::Ready;
            if created {
                state.status_message =
                    format!("Created minimal config at {}", config_path.display());
            }
            state
        }
        Err(error) => {
            // The in-memory default is only an editor seed here; it can never
            // launch until a valid configuration has been saved.
            let mut state = AppState::new(GuardConfig::default(), config_path.into());
            state.config_readiness = ConfigReadiness::RepairRequired;
            if GuardConfig::read_version(config_path) == Some(3) {
                state.status_message =
                    "Managed (v3) configuration is not supported; press C to rebuild the local \
                     proxy configuration"
                        .into();
            } else {
                state.status_message = "Configuration needs repair before launch".into();
            }
            state.error_message = Some(redact_text(&format!(
                "CONFIG_INVALID: {error}. Press C to configure and replace it."
            )));
            state
        }
    }
}

fn init_config(
    path: &Path,
    force: bool,
    proxy_host: Option<String>,
    proxy_port: Option<u16>,
) -> anyhow::Result<()> {
    if path.exists() && !force {
        bail!(
            "configuration already exists at {}; use --force to replace it",
            path.display()
        );
    }
    let mut config = GuardConfig::default();
    if let Some(host) = proxy_host {
        config.proxy.host = host;
    }
    if let Some(port) = proxy_port {
        config.proxy.port = port;
    }
    config
        .save(path)
        .with_context(|| format!("write configuration {}", path.display()))?;
    println!("Created {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_elevated_startup_keeps_the_default_state() {
        let state =
            with_elevation_hint(AppState::new(GuardConfig::default(), "config.toml".into()));
        assert!(state.error_message.is_none());
        assert_eq!(state.config_readiness, ConfigReadiness::Ready);
    }

    #[test]
    fn invalid_configuration_opens_the_tui_in_repair_mode() {
        let path = std::env::temp_dir().join(format!(
            "codex-proxy-guard-invalid-config-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "version = 1\n").unwrap();
        let state = tui_state(&path);
        assert_eq!(state.config, GuardConfig::default());
        assert_eq!(state.config_readiness, ConfigReadiness::RepairRequired);
        assert!(
            state
                .error_message
                .as_deref()
                .is_some_and(|error| error.contains("Press C"))
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn managed_v3_configuration_gets_targeted_guidance() {
        let path = std::env::temp_dir().join(format!(
            "codex-proxy-guard-v3-config-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "version = 3\n").unwrap();
        let state = tui_state(&path);
        assert_eq!(state.config_readiness, ConfigReadiness::RepairRequired);
        assert!(state.status_message.contains("Managed (v3)"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn receipt_summary_separates_the_proxy_layers() {
        let receipt = |delivery| LaunchReceipt {
            pid: 42,
            proxy_endpoint: if matches!(delivery, proxy_guard_core::ProxyDelivery::NotEstablished) {
                None
            } else {
                Some("http://127.0.0.1:10808".into())
            },
            daemon_preparation: proxy_guard_core::DaemonPreparation::Skipped,
            launch_method: proxy_guard_core::LaunchMethod::AppmodelActivation,
            activation_state: proxy_guard_core::ActivationState::Returned,
            instance: proxy_guard_core::InstanceObservation::Created,
            package_identity: proxy_guard_core::PackageIdentityObservation::Matched,
            aumid: proxy_guard_core::AumidObservation::Matched,
            proxy_delivery: delivery,
            backend_proxy_config: match delivery {
                proxy_guard_core::ProxyDelivery::ActivationArgumentsAndHomeConfig => {
                    proxy_guard_core::BackendProxyConfig::Prepared
                }
                proxy_guard_core::ProxyDelivery::ActivationArguments => {
                    proxy_guard_core::BackendProxyConfig::NotAuthorized
                }
                _ => proxy_guard_core::BackendProxyConfig::NotApplicable,
            },
            target_elevation: Some(false),
            desktop: proxy_guard_core::DesktopLaunchInfo {
                product: proxy_guard_core::DesktopProduct::ChatGpt,
                package_name: "OpenAI.Codex".into(),
                package_version: "1".into(),
                architecture: "X64".into(),
                discovery_source: proxy_guard_core::DesktopDiscoverySource::AppxManifest,
            },
        };
        let activation_only = receipt(proxy_guard_core::ProxyDelivery::NotEstablished);
        assert!(
            activation_only.proxy_endpoint.is_none(),
            "activation-only must never imply a delivered proxy plan"
        );
        let layered = receipt(proxy_guard_core::ProxyDelivery::ActivationArgumentsAndHomeConfig);
        assert_eq!(
            layered.backend_proxy_config,
            proxy_guard_core::BackendProxyConfig::Prepared
        );
    }
}
