//! Strict command-line parsing (`command_line.cpp` port) and the CLI
//! dispatcher (`cli.cpp` port): exact usage text, single-value options,
//! mutual-exclusion rules, JSON/human error shapes.

use crate::build_info;
use crate::licenses;
use crate::worker;
use cpg_domain::config::Config;
use cpg_domain::{Error, Json, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    Interactive,
    Launch,
    InitConfig,
    ConfigPath,
    BuildInfo,
    Licenses,
    InternalActivatePackage,
    Help,
    Version,
}

#[derive(Clone, Debug, Default)]
pub struct ParsedCommand {
    pub kind: Option<CommandKind>,
    pub plain: bool,
    pub json: bool,
    pub repair: bool,
    pub activation_only: bool,
    pub force: bool,
    pub config_override: Option<String>,
    pub proxy_host: Option<String>,
    pub proxy_port: Option<i64>,
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(Error::new("CLI_INVALID", message))
}

pub fn parse_command_line(arguments: &[String]) -> Result<ParsedCommand> {
    let mut parsed = ParsedCommand::default();
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut flags: BTreeSet<String> = BTreeSet::new();
    let mut command: Option<String> = None;
    let mut options = true;
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = &arguments[index];
        index += 1;
        if options && argument == "--" {
            options = false;
            continue;
        }
        if options && argument.starts_with('-') && !argument.is_empty() {
            let mut name = if argument == "-h" || argument == "-?" {
                "help".to_owned()
            } else if argument == "-v" {
                "version".to_owned()
            } else if let Some(stripped) = argument.strip_prefix("--") {
                stripped.to_owned()
            } else {
                return invalid("Unknown option.");
            };
            let mut value: Option<String> = None;
            if let Some(equal) = name.find('=') {
                value = Some(name[equal + 1..].to_owned());
                name.truncate(equal);
            }
            match name.as_str() {
                "config" | "proxy-host" | "proxy-port" => {
                    if values.contains_key(&name) {
                        return invalid("A value option may be specified only once.");
                    }
                    let value = match value {
                        Some(value) => value,
                        None => {
                            let Some(next) = arguments.get(index) else {
                                return invalid("The option requires a value.");
                            };
                            if next.starts_with('-') && !next.is_empty() {
                                return invalid(
                                    "The option requires a value; use --name=value for a value beginning with '-'.",
                                );
                            }
                            index += 1; // consume the positional value
                            next.clone()
                        }
                    };
                    if value.is_empty() {
                        return invalid("Option values must not be empty.");
                    }
                    values.insert(name, value);
                }
                "plain"
                | "json"
                | "refresh-codex-daemon"
                | "activation-only"
                | "force"
                | "help"
                | "version" => {
                    if value.is_some() {
                        return invalid("The flag does not accept a value.");
                    }
                    if !flags.insert(name.clone()) {
                        return invalid("A flag may be specified only once.");
                    }
                }
                _ => return invalid("Unknown option."),
            }
            continue;
        }
        if command.is_some() {
            return invalid("Expected one command.");
        }
        command = Some(argument.clone());
    }
    if let Some(command) = command.as_deref() {
        parsed.kind = Some(match command {
            "console" => CommandKind::Interactive,
            "launch" => CommandKind::Launch,
            "init-config" => CommandKind::InitConfig,
            "config-path" => CommandKind::ConfigPath,
            "build-info" => CommandKind::BuildInfo,
            "licenses" => CommandKind::Licenses,
            "internal-activate-package" => CommandKind::InternalActivatePackage,
            _ => return invalid("Unknown command."),
        });
    }
    if flags.contains("help") || flags.contains("version") {
        if command.is_some() || flags.len() != 1 || !values.is_empty() {
            return invalid("Information flags cannot be combined with other operations.");
        }
        parsed.kind = Some(if flags.contains("help") {
            CommandKind::Help
        } else {
            CommandKind::Version
        });
    }
    parsed.plain = flags.contains("plain");
    parsed.json = flags.contains("json");
    parsed.repair = flags.contains("refresh-codex-daemon");
    parsed.activation_only = flags.contains("activation-only");
    parsed.force = flags.contains("force");
    if parsed.plain && parsed.kind != Some(CommandKind::Interactive) {
        return invalid("The plain flag requires interactive mode.");
    }
    if (parsed.json || parsed.repair || parsed.activation_only)
        && parsed.kind != Some(CommandKind::Launch)
    {
        return invalid("Launch options require launch.");
    }
    if parsed.repair && parsed.activation_only {
        return invalid("Repair and activation-only cannot be combined.");
    }
    if (parsed.force || values.contains_key("proxy-host") || values.contains_key("proxy-port"))
        && parsed.kind != Some(CommandKind::InitConfig)
    {
        return invalid("Configuration options require init-config.");
    }
    if let Some(config) = values.get("config") {
        if parsed.kind == Some(CommandKind::InternalActivatePackage) {
            return invalid("The worker does not accept configuration.");
        }
        parsed.config_override = Some(config.clone());
    }
    if let Some(host) = values.get("proxy-host") {
        parsed.proxy_host = Some(host.clone());
    }
    if let Some(port) = values.get("proxy-port") {
        parsed.proxy_port = Some(cpg_domain::parse_port_strict(port).map_err(|_| {
            Error::new(
                "CLI_INVALID",
                "Proxy port must be ASCII digits in the range 1-65535.",
            )
        })? as i64);
    }
    Ok(parsed)
}

pub fn resolve_config_path(command: &ParsedCommand) -> Result<PathBuf> {
    let path = match &command.config_override {
        Some(path) => PathBuf::from(path),
        None => cpg_engine::default_config_path()?,
    };
    if path.as_os_str().is_empty() {
        return invalid("The configuration path must not be empty.");
    }
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&path)
    };
    if !absolute.is_absolute() {
        return invalid("Could not resolve the configuration path.");
    }
    Ok(lexically_normal(&absolute))
}

/// std::filesystem::path::lexically_normal equivalent (no filesystem access).
fn lexically_normal(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

pub fn usage_text() -> &'static str {
    "Usage: CodexProxyGuard [options] [command]\n\
     Launch Desktop through a loopback HTTP/Mixed proxy.\n\
     \n\
     Options:\n\
     \x20 -h, -?, --help      Displays help on commandline options.\n\
     \x20 -v, --version       Displays version information.\n\
     \x20 --config <path>     Guard configuration file.\n\
     \n\
     Commands:\n\
     \x20 launch                   Launch Desktop\n\
     \x20   --json                   Print launch receipt as JSON\n\
     \x20   --refresh-codex-daemon   Single-use authorization to stop the shared daemon\n\
     \x20   --activation-only        Registered activation diagnostic\n\
     \x20 init-config              Write the default configuration\n\
     \x20   --force                  Replace the Guard configuration\n\
     \x20   --proxy-host <host>      Loopback proxy host\n\
     \x20   --proxy-port <port>      Proxy port\n\
     \x20 config-path              Print the effective configuration path\n\
     \x20 build-info               Print embedded build provenance\n\
     \x20 licenses                 Stream the embedded offline notices\n\
     \x20 console                  Interactive console (--plain for line mode)\n\
     \x20 internal-activate-package  Activation worker (internal)"
}

fn print(text: &str) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

fn print_json(value: &Json) {
    print(&value.dump());
}

fn report_error(error: &Error, json_output: bool) -> i32 {
    if json_output {
        print_json(&Json::object_from([("error", error.json())]));
    } else {
        eprintln!("{}: {}", error.code, error.message);
    }
    1
}

pub fn run(arguments: &[String]) -> i32 {
    let mut json_output = false;
    let result = dispatch(arguments, &mut json_output);
    match result {
        Ok(code) => code,
        Err(error) => report_error(&error, json_output),
    }
}

fn dispatch(arguments: &[String], json_output: &mut bool) -> Result<i32> {
    let command = parse_command_line(arguments)?;
    *json_output = command.json;
    match command.kind {
        Some(CommandKind::Help) => {
            print(usage_text());
            Ok(0)
        }
        Some(CommandKind::Version) => {
            print(&format!(
                "codex-proxy-guard {}",
                build_info::product_version()
            ));
            Ok(0)
        }
        Some(CommandKind::InternalActivatePackage) => Ok(worker::activation_worker()),
        Some(CommandKind::BuildInfo) => {
            print_json(&Json::object_from([
                ("version", Json::from(build_info::product_version())),
                ("commit", Json::from(build_info::build_commit())),
                ("dirty", Json::from(build_info::build_dirty())),
                ("language", Json::from("Rust")),
            ]));
            Ok(0)
        }
        Some(CommandKind::Licenses) => Ok(licenses::stream()),
        _ => {
            let config_path = resolve_config_path(&command)?;
            match command.kind {
                Some(CommandKind::ConfigPath) => {
                    print(&config_path.to_string_lossy());
                    Ok(0)
                }
                Some(CommandKind::InitConfig) => {
                    let mut config = Config::default();
                    if let Some(host) = &command.proxy_host {
                        config.host = host.clone();
                    }
                    if let Some(port) = command.proxy_port {
                        config.port = port;
                    }
                    cpg_engine::initialize_config(&config_path, &config, command.force)?;
                    print("Configuration saved.");
                    Ok(0)
                }
                Some(CommandKind::Launch) => run_launch(&command, &config_path),
                _ => {
                    // Interactive console (explicit console subcommand or the
                    // default no-argument form).
                    let code = cpg_tui::run_console(&config_path, command.plain)?;
                    Ok(code)
                }
            }
        }
    }
}

fn run_launch(command: &ParsedCommand, config_path: &Path) -> Result<i32> {
    let config = cpg_engine::load_or_create_config(config_path)?;
    // Console signals translate to cancellation while the operation runs;
    // a settled receipt remains true even if Ctrl+C arrived late.
    let config_path = config_path.to_path_buf();
    let options = cpg_engine::LaunchOptions {
        repair: command.repair,
        activation_only: command.activation_only,
    };
    let receipt = cpg_windows::console::run_console_operation(move |cancel| {
        cpg_engine::launch_pipeline(&config, &config_path, options, cancel, None)
    })?;
    if command.json {
        print_json(&receipt);
    } else {
        let pid = receipt.at("pid").as_u64().unwrap_or(0);
        let endpoint = receipt.string_at("proxy_endpoint");
        print(&format!(
            "Desktop activation: PID {pid}; instance={}; proxy={} endpoint={}; backend config={}; daemon={}",
            receipt.string_at("instance"),
            receipt.string_at("proxy_delivery"),
            if endpoint.is_empty() { "-" } else { &endpoint },
            receipt.string_at("backend_proxy_config"),
            receipt.string_at("daemon_preparation"),
        ));
        print("Delivery facts only: Desktop adoption and network use are not verified.");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<ParsedCommand> {
        let owned: Vec<String> = arguments.iter().map(|s| s.to_string()).collect();
        parse_command_line(&owned)
    }

    #[test]
    fn parses_all_public_commands() {
        assert_eq!(parse(&["launch"]).unwrap().kind, Some(CommandKind::Launch));
        assert_eq!(
            parse(&["config-path"]).unwrap().kind,
            Some(CommandKind::ConfigPath)
        );
        assert_eq!(
            parse(&["build-info"]).unwrap().kind,
            Some(CommandKind::BuildInfo)
        );
        assert_eq!(
            parse(&["licenses"]).unwrap().kind,
            Some(CommandKind::Licenses)
        );
        assert_eq!(
            parse(&["console"]).unwrap().kind,
            Some(CommandKind::Interactive)
        );
        assert!(parse(&["console", "--plain"]).unwrap().plain);
        assert_eq!(parse(&["-h"]).unwrap().kind, Some(CommandKind::Help));
        assert_eq!(parse(&["-?"]).unwrap().kind, Some(CommandKind::Help));
        assert_eq!(
            parse(&["--version"]).unwrap().kind,
            Some(CommandKind::Version)
        );
        assert_eq!(parse(&["-v"]).unwrap().kind, Some(CommandKind::Version));
        assert_eq!(parse(&[]).unwrap().kind, None);
    }

    #[test]
    fn value_options_are_single_use_and_nonempty() {
        assert!(parse(&["--config", "a.toml", "--config", "b.toml"]).is_err());
        assert!(parse(&["--config", ""]).is_err());
        assert!(parse(&["--config=a.toml"]).is_ok());
        assert!(parse(&["--config"]).is_err());
        assert!(parse(&["--config", "-weird"]).is_err());
        assert!(parse(&["--config=-weird"]).is_ok());
    }

    #[test]
    fn mutual_exclusion_rules_match_the_contract() {
        assert_eq!(parse(&["--json"]).unwrap_err().code, "CLI_INVALID");
        assert!(parse(&["console", "--json"]).is_err());
        assert!(parse(&["launch", "--plain"]).is_err());
        assert!(parse(&["config-path", "--json"]).is_err());
        assert!(parse(&["launch", "--refresh-codex-daemon", "--activation-only"]).is_err());
        assert!(parse(&["init-config", "--force", "--proxy-host", "127.0.0.1"]).is_ok());
        assert!(parse(&["launch", "--force"]).is_err());
        assert!(parse(&["--version", "--json"]).is_err());
        assert!(parse(&["--version", "launch"]).is_err());
        assert!(parse(&["--help", "--json"]).is_err());
        assert!(parse(&["internal-activate-package", "--config", "x"]).is_err());
        assert!(parse(&["launch", "--proxy-port", "abc"]).is_err());
        assert!(parse(&["launch", "--proxy-port", "0"]).is_err());
        assert!(parse(&["bogus"]).is_err());
        assert!(parse(&["launch", "extra"]).is_err());
        assert!(parse(&["--unknown"]).is_err());
        assert!(parse(&["launch", "--json", "--json"]).is_err());
    }

    #[test]
    fn double_dash_stops_option_parsing() {
        let parsed = parse(&["--", "launch"]).unwrap();
        assert_eq!(parsed.kind, Some(CommandKind::Launch));
    }
}
