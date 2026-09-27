use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "codex-proxy-guard",
    version,
    about = "Launch ChatGPT Desktop (Chat, Work, and Codex) with a loopback HTTP proxy: registered apps are activated through Windows application activation, plain executables get a process-scoped environment"
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
        /// Before launching, stop the shared Codex background server through
        /// the official `codex app-server daemon stop` lifecycle command. This
        /// is a one-shot authorization for this invocation only and may
        /// interrupt tasks of other CLI / IDE / remote clients sharing the
        /// same Codex Home.
        #[arg(long, conflicts_with = "activation_only")]
        refresh_codex_daemon: bool,
        /// Diagnostic identity check: activate a registered Desktop without
        /// proxy arguments and without touching the Codex Home `.env`. The
        /// receipt reports proxy_delivery = not_established; this is never a
        /// silent downgrade of a normal proxy launch.
        #[arg(long)]
        activation_only: bool,
    },
    /// Print embedded build provenance (version, commit, dirty, exe path).
    BuildInfo,
    #[command(hide = true)]
    /// Internal one-shot native activation worker. Reads one bounded JSON
    /// request on stdin, performs the activation, and writes one receipt line
    /// to stdout. Not a public entry point.
    InternalActivatePackage,
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
            Some(Command::Launch {
                json: true,
                refresh_codex_daemon: false,
                activation_only: false,
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["cpg", "launch", "--refresh-codex-daemon", "--json"])
                .unwrap()
                .command,
            Some(Command::Launch {
                json: true,
                refresh_codex_daemon: true,
                activation_only: false,
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["cpg", "launch", "--activation-only"])
                .unwrap()
                .command,
            Some(Command::Launch {
                activation_only: true,
                ..
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["cpg", "build-info"]).unwrap().command,
            Some(Command::BuildInfo)
        ));
        assert!(matches!(
            Cli::try_parse_from(["cpg", "internal-activate-package"])
                .unwrap()
                .command,
            Some(Command::InternalActivatePackage)
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
        // The removed experimental surfaces must stay gone.
        assert!(Cli::try_parse_from(["cpg", "usage"]).is_err());
        assert!(Cli::try_parse_from(["cpg", "node-test"]).is_err());
        assert!(Cli::try_parse_from(["cpg", "daemon-stop"]).is_err());
        assert!(Cli::try_parse_from(["cpg", "launch", "--auto-stop-daemon"]).is_err());
        assert!(Cli::try_parse_from(["cpg", "launch", "--package-context-compat"]).is_err());
        assert!(Cli::try_parse_from(["cpg", "package-helper", "--pipe", "x"]).is_err());
        // Stopping the shared daemon for a deliberately unproxied diagnostic
        // is contradictory; the CLI refuses the combination outright.
        assert!(
            Cli::try_parse_from([
                "cpg",
                "launch",
                "--activation-only",
                "--refresh-codex-daemon"
            ])
            .is_err()
        );
    }
}
