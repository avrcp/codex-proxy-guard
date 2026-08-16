use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "codex-proxy-guard",
    version,
    about = "Launch ChatGPT Desktop (Chat, Work, and Codex) with a process-scoped loopback HTTP proxy"
)]
pub struct Cli {
    /// Use a specific Guard configuration file.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch ChatGPT Desktop immediately without opening the TUI.
    Launch {
        /// Print the launch receipt as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Create the minimal configuration file.
    InitConfig {
        /// Replace an existing configuration file.
        #[arg(long)]
        force: bool,
        /// Local HTTP/Mixed proxy host to write to the configuration file.
        #[arg(long, value_name = "HOST")]
        proxy_host: Option<String>,
        /// Local HTTP/Mixed proxy port to write to the configuration file.
        #[arg(long, value_name = "PORT")]
        proxy_port: Option<u16>,
        /// Enable Managed Mode (subscription-driven JP/SG/US nodes).
        #[arg(long)]
        managed: bool,
    },
    /// Print the resolved configuration path.
    ConfigPath,
    /// Manage airport subscriptions.
    Subscription {
        #[command(subcommand)]
        action: SubscriptionCommand,
    },
    /// List the imported JP/SG/US nodes.
    NodeList {
        /// Only show nodes whose region hint matches.
        #[arg(long, value_name = "REGION")]
        region: Option<RegionArg>,
    },
    /// Benchmark JP/SG/US nodes and cache the winner.
    Benchmark {
        /// Force a full rescan (an explicit benchmark always rescans).
        #[arg(long)]
        force: bool,
        /// Print the summary as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the currently selected JP > SG > US winner from fresh cache.
    BestNode,
}

#[derive(Debug, Subcommand)]
pub enum SubscriptionCommand {
    /// Add a subscription (stores the URL in the OS credential store).
    Add {
        /// Display name for the subscription.
        #[arg(long, value_name = "NAME")]
        name: String,
        /// HTTPS subscription URL (stored as a credential, never logged).
        #[arg(long, value_name = "URL")]
        url: String,
    },
    /// List saved subscriptions and their sync status.
    List,
    /// Fetch and reconcile one subscription, importing only JP/SG/US nodes.
    Sync {
        /// Subscription name or ID.
        reference: String,
    },
    /// Delete one subscription and its credential (nodes are retained).
    Delete {
        /// Subscription name or ID.
        reference: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RegionArg {
    #[value(name = "JP")]
    JP,
    #[value(name = "SG")]
    SG,
    #[value(name = "US")]
    US,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_surface_parses_new_managed_commands() {
        assert!(matches!(
            Cli::try_parse_from([
                "cpg",
                "subscription",
                "add",
                "--name",
                "A",
                "--url",
                "https://x"
            ])
            .unwrap()
            .command,
            Some(Command::Subscription {
                action: SubscriptionCommand::Add { .. }
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["cpg", "node-list", "--region", "JP"])
                .unwrap()
                .command,
            Some(Command::NodeList { .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["cpg", "benchmark", "--json"])
                .unwrap()
                .command,
            Some(Command::Benchmark { .. })
        ));
        assert!(
            Cli::try_parse_from(["cpg", "best-node"])
                .unwrap()
                .command
                .is_some()
        );
    }

    #[test]
    fn command_surface_rejects_unknown_commands() {
        assert!(Cli::try_parse_from(["cpg", "usage"]).is_err());
        assert!(Cli::try_parse_from(["cpg", "node-test"]).is_err());
    }
}
