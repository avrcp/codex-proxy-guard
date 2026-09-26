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
    let config_path = cli.config.unwrap_or(GuardConfig::config_path()?);

    match cli.command {
        Some(Command::ConfigPath) => {
            println!("{}", config_path.display());
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
        }) => {
            let (config, _) = GuardConfig::load_or_create(&config_path)
                .with_context(|| format!("load configuration {}", config_path.display()))?;
            let options = LaunchOptions {
                refresh_codex_daemon,
            };
            let (_, receipt) = launch_command(&config, options).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                println!(
                    "{} process created through {} (PID {})",
                    receipt.desktop.product.display_name(),
                    receipt.proxy_endpoint,
                    receipt.pid
                );
                if !receipt.daemon_preparation.status_detail().is_empty() {
                    println!("{}", receipt.daemon_preparation.status_detail());
                }
            }
            Ok(())
        }
        None => tui::run(with_elevation_hint(tui_state(&config_path))).await,
    }
}

/// CLI launch with the same cancellation protocol as the TUI: Ctrl-C cancels
/// the pipeline, Guard waits for its bounded cleanup, and the exit status
/// reflects the outcome.
async fn launch_command(
    config: &GuardConfig,
    options: LaunchOptions,
) -> anyhow::Result<(DesktopAppInfo, LaunchReceipt)> {
    let cancellation = CancellationToken::new();
    let ctrl_c_token = cancellation.clone();
    let ctrl_c = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            ctrl_c_token.cancel();
        }
    });
    let result = launch_pipeline(config, options, &cancellation).await;
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
}
