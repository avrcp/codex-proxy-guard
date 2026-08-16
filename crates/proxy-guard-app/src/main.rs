mod cli;
mod commands;
mod dispatcher;
mod tui;

use std::path::Path;

use anyhow::{Context, bail};
use clap::Parser;
use cli::{Cli, Command, RegionArg, SubscriptionCommand};
use commands::{
    cmd_benchmark, cmd_best_node, cmd_node_list, cmd_subscription_add, cmd_subscription_delete,
    cmd_subscription_list, cmd_subscription_sync,
};
use dispatcher::launch_pipeline;
use proxy_guard_core::{AppState, GuardConfig, ProxyMode, redact_text};
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
            managed,
        }) => init_config(&config_path, force, proxy_host, proxy_port, managed),
        Some(Command::Launch { json }) => {
            let (config, _) = GuardConfig::load_or_create(&config_path)
                .with_context(|| format!("load configuration {}", config_path.display()))?;
            let (_, receipt) = launch_pipeline(&config, None, &CancellationToken::new())
                .await
                .map_err(|error| anyhow::anyhow!(redact_text(&error)))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                println!(
                    "{} launched through {} (PID {})",
                    receipt.desktop.product.display_name(),
                    receipt.proxy_endpoint,
                    receipt.pid
                );
            }
            Ok(())
        }
        Some(Command::Subscription { action }) => match action {
            SubscriptionCommand::Add { name, url } => cmd_subscription_add(&name, &url),
            SubscriptionCommand::List => cmd_subscription_list(),
            SubscriptionCommand::Sync { reference } => cmd_subscription_sync(&reference),
            SubscriptionCommand::Delete { reference, yes } => {
                cmd_subscription_delete(&reference, yes)
            }
        },
        Some(Command::NodeList { region }) => cmd_node_list(region.map(map_region)),
        Some(Command::Benchmark { force: _, json }) => {
            let config = load_config(&config_path)?;
            cmd_benchmark(&config, json).await
        }
        Some(Command::BestNode) => {
            let config = load_config(&config_path)?;
            cmd_best_node(&config)
        }
        None => tui::run(tui_state(&config_path)).await,
    }
}

fn map_region(region: RegionArg) -> proxy_guard_core::CodexRegion {
    match region {
        RegionArg::JP => proxy_guard_core::CodexRegion::JP,
        RegionArg::SG => proxy_guard_core::CodexRegion::SG,
        RegionArg::US => proxy_guard_core::CodexRegion::US,
    }
}

fn load_config(path: &Path) -> anyhow::Result<GuardConfig> {
    let (config, _) = GuardConfig::load_or_create(path)
        .with_context(|| format!("load configuration {}", path.display()))?;
    Ok(config)
}

fn tui_state(config_path: &Path) -> AppState {
    match GuardConfig::load_or_create(config_path) {
        Ok((config, created)) => {
            let mut state = AppState::new(config, config_path.into());
            if created {
                state.status_message =
                    format!("Created minimal config at {}", config_path.display());
            }
            state
        }
        Err(error) => {
            let mut state = AppState::new(GuardConfig::default(), config_path.into());
            state.status_message = "Configuration needs repair before launch".into();
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
    managed: bool,
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
    if managed {
        config.proxy.mode = ProxyMode::Managed;
    }
    config
        .save(path)
        .with_context(|| format!("write configuration {}", path.display()))?;
    println!("Created {}", path.display());
    if managed {
        println!("Managed Mode enabled. Add a subscription with:");
        println!("  codex-proxy-guard subscription add --name \"Airport\" --url <https-url>");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            state
                .error_message
                .as_deref()
                .is_some_and(|error| error.contains("Press C"))
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn region_mapping_is_deterministic() {
        assert_eq!(map_region(RegionArg::JP), proxy_guard_core::CodexRegion::JP);
        assert_eq!(map_region(RegionArg::US), proxy_guard_core::CodexRegion::US);
    }
}
