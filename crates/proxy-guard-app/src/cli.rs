use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
    },
    /// Print the resolved configuration path.
    ConfigPath,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_surface_is_intentionally_small() {
        assert!(Cli::try_parse_from(["cpg"]).unwrap().command.is_none());
        assert!(matches!(
            Cli::try_parse_from(["cpg", "launch", "--json"])
                .unwrap()
                .command,
            Some(Command::Launch { json: true })
        ));
        assert!(matches!(
            Cli::try_parse_from(["cpg", "init-config", "--proxy-port", "7890"])
                .unwrap()
                .command,
            Some(Command::InitConfig {
                proxy_port: Some(7890),
                ..
            })
        ));
        assert!(Cli::try_parse_from(["cpg", "usage"]).is_err());
        assert!(Cli::try_parse_from(["cpg", "node-test"]).is_err());
    }
}
