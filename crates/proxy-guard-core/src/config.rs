use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

use crate::GuardError;

pub const CONFIG_VERSION: u32 = 2;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GuardConfig {
    pub version: u32,
    pub proxy: ProxyConfig,
    pub codex: CodexConfig,
    pub tui: TuiConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyConfig {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub no_proxy: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CodexConfig {
    /// ChatGPT Desktop executable override.
    pub executable_override: PathBuf,
    /// Official Codex CLI executable override used only for the public
    /// `app-server daemon stop` lifecycle command. Empty means resolve from
    /// CODEX_HOME packages and PATH.
    pub cli_executable_override: PathBuf,
    pub refuse_if_running: bool,
    /// Explicit user consent for Guard to manage the HTTP_PROXY / HTTPS_PROXY /
    /// NO_PROXY block inside one authorized Codex Home `.env`. Default off; the
    /// flag is meaningless without the bound `proxy_env_home` below. Changing
    /// the home requires a new confirmation.
    pub manage_codex_proxy_env: bool,
    /// The absolute Codex Home whose `.env` the proxy-block consent is bound
    /// to. Empty means no home is authorized yet. This is the default
    /// user home's `.codex` unless the user explicitly authorized another
    /// location; Guard never infers it from its own CODEX_HOME variable.
    pub proxy_env_home: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    pub alternate_screen: AlternateScreen,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AlternateScreen {
    Always,
    Never,
    #[default]
    Auto,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            proxy: ProxyConfig::default(),
            codex: CodexConfig::default(),
            tui: TuiConfig::default(),
        }
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            scheme: "http".into(),
            host: "127.0.0.1".into(),
            port: 10808,
            no_proxy: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
        }
    }
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            executable_override: PathBuf::new(),
            cli_executable_override: PathBuf::new(),
            refuse_if_running: true,
            manage_codex_proxy_env: false,
            proxy_env_home: PathBuf::new(),
        }
    }
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            alternate_screen: AlternateScreen::Auto,
        }
    }
}

impl GuardConfig {
    pub fn config_path() -> Result<PathBuf, GuardError> {
        BaseDirs::new()
            .map(|dirs| {
                dirs.config_dir()
                    .join("codex-proxy-guard")
                    .join("config.toml")
            })
            .ok_or_else(|| {
                GuardError::Config("cannot resolve the user configuration directory".into())
            })
    }

    /// Candidate Codex Home for the `.env` proxy-block consent prompt: the
    /// default `.codex` under the OS user-profile home. This is a display
    /// candidate bound by an explicit authorization — never a claim about the
    /// home the activated Desktop actually uses, and never taken from Guard's
    /// own CODEX_HOME variable.
    pub fn default_codex_home() -> Option<PathBuf> {
        BaseDirs::new().map(|dirs| dirs.home_dir().join(".codex"))
    }

    pub fn load(path: &Path) -> Result<Self, GuardError> {
        let text = fs::read_to_string(path)
            .map_err(|error| GuardError::Io(format!("cannot read {}: {error}", path.display())))?;
        Self::parse(&text).map_err(|error| {
            GuardError::Config(format!("cannot parse {}: {error}", path.display()))
        })
    }

    pub fn load_or_create(path: &Path) -> Result<(Self, bool), GuardError> {
        if path.exists() {
            return Self::load(path).map(|config| (config, false));
        }
        let config = Self::default();
        config.save(path)?;
        Ok((config, true))
    }

    pub fn save(&self, path: &Path) -> Result<(), GuardError> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                GuardError::Io(format!("cannot create {}: {error}", parent.display()))
            })?;
        }
        let text = toml::to_string_pretty(self).map_err(|error| {
            GuardError::Config(format!("cannot serialize configuration: {error}"))
        })?;
        fs::write(path, text)
            .map_err(|error| GuardError::Io(format!("cannot write {}: {error}", path.display())))
    }

    pub fn validate(&self) -> Result<(), GuardError> {
        if self.version != CONFIG_VERSION {
            return Err(GuardError::Config(format!(
                "unsupported configuration version {}; expected {CONFIG_VERSION}",
                self.version
            )));
        }
        if !self.proxy.scheme.eq_ignore_ascii_case("http") {
            return Err(GuardError::Config("proxy.scheme must be http".into()));
        }
        if self.proxy.port == 0 {
            return Err(GuardError::Config(
                "proxy.port must be between 1 and 65535".into(),
            ));
        }
        // Validation and URL construction must agree on the host, so reject
        // leading/trailing whitespace instead of trimming in one place only.
        if self.proxy.host != self.proxy.host.trim() || self.proxy.host.is_empty() {
            return Err(GuardError::Config(
                "proxy.host must not be empty or contain leading/trailing whitespace".into(),
            ));
        }
        let host = self.proxy.host.trim();
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback());
        if !loopback {
            return Err(GuardError::Config(
                "proxy.host must be localhost or a loopback IP address".into(),
            ));
        }
        if self.proxy.no_proxy.is_empty() || self.proxy.no_proxy.len() > 32 {
            return Err(GuardError::Config(
                "proxy.no_proxy must contain 1 to 32 entries".into(),
            ));
        }
        if self.proxy.no_proxy.iter().any(|value| {
            value.is_empty()
                || value != value.trim()
                || value.len() > 255
                || value.contains(['\r', '\n', '\0'])
        }) || self.no_proxy_value().len() > 4096
        {
            return Err(GuardError::Config(
                "proxy.no_proxy contains an invalid entry".into(),
            ));
        }
        Ok(())
    }

    pub fn proxy_url(&self) -> String {
        let host = self.normalized_host();
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        format!("http://{host}:{}", self.proxy.port)
    }

    fn normalized_host(&self) -> &str {
        self.proxy.host.trim()
    }

    /// Best-effort read of the `version` key for targeted repair messaging.
    /// Returns `None` when the file is missing or not a TOML table.
    pub fn read_version(path: &Path) -> Option<u32> {
        let text = fs::read_to_string(path).ok()?;
        let value = toml::from_str::<toml::Value>(&text).ok()?;
        value.get("version")?.as_integer()?.try_into().ok()
    }

    pub fn no_proxy_value(&self) -> String {
        self.proxy.no_proxy.join(",")
    }

    fn parse(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        config.validate().map_err(|error| error.to_string())?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_minimal_and_valid() {
        let config = GuardConfig::default();
        config.validate().unwrap();
        let value = toml::Value::try_from(&config).unwrap();
        let table = value.as_table().unwrap();
        assert_eq!(table.len(), 4);
        assert!(table.contains_key("proxy"));
        assert!(table.contains_key("codex"));
        assert!(table.contains_key("tui"));
    }

    #[test]
    fn version_one_is_rejected_without_migration() {
        let error = GuardConfig::parse("version = 1").unwrap_err();
        assert!(error.contains("unsupported configuration version"));
    }

    #[test]
    fn current_version_rejects_legacy_fields() {
        let error = GuardConfig::parse(
            r#"
                version = 2
                [proxy]
                host = "127.0.0.1"
                remove_all_proxy = true
            "#,
        )
        .unwrap_err();
        assert!(error.contains("unknown field"));
    }

    #[test]
    fn cli_override_is_an_optional_v2_extension() {
        let config = GuardConfig::parse(
            r#"
                version = 2
                [codex]
                executable_override = ""
                refuse_if_running = true
            "#,
        )
        .unwrap();
        assert_eq!(config.codex.cli_executable_override, PathBuf::new());

        let config = GuardConfig::parse(
            r#"
                version = 2
                [codex]
                cli_executable_override = "D:\\Tools\\codex.exe"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.codex.cli_executable_override,
            PathBuf::from(r"D:\Tools\codex.exe")
        );
    }

    #[test]
    fn backend_proxy_consent_defaults_off_and_stays_a_launch_time_gate() {
        // An enabled flag without a bound home is not a global configuration
        // error: it must only block the registered-application launch that
        // would need the block (reported as BACKEND_PROXY_SCOPE_UNCONFIRMED
        // by the launcher), so unrelated launches keep working.
        let config = GuardConfig::default();
        assert!(!config.codex.manage_codex_proxy_env);
        assert!(config.codex.proxy_env_home.as_os_str().is_empty());
        config.validate().unwrap();

        let mut enabled = GuardConfig::default();
        enabled.codex.manage_codex_proxy_env = true;
        enabled.validate().unwrap();

        enabled.codex.proxy_env_home = PathBuf::from(r"C:\Users\example\.codex");
        enabled.validate().unwrap();
    }

    #[test]
    fn rejects_remote_or_non_http_proxy() {
        let mut config = GuardConfig::default();
        config.proxy.host = "192.0.2.1".into();
        assert!(config.validate().is_err());
        config.proxy.host = "127.0.0.1".into();
        config.proxy.scheme = "socks5".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn host_validation_and_url_construction_agree() {
        let mut config = GuardConfig::default();
        config.proxy.host = " 127.0.0.1 ".into();
        assert!(config.validate().is_err(), "whitespace hosts are rejected");
        config.proxy.host = " 127.0.0.1".into();
        assert!(config.validate().is_err());
        config.proxy.host = "::1".into();
        assert!(config.validate().is_ok());
        assert_eq!(config.proxy_url(), "http://[::1]:10808");
        config.proxy.host = "LOCALHOST".into();
        assert!(config.validate().is_ok());
        assert_eq!(config.proxy_url(), "http://LOCALHOST:10808");
    }

    #[test]
    fn no_proxy_validation_matches_package_helper_limit() {
        let mut config = GuardConfig::default();
        for value in ["", " ", " example.com", "example.com ", "a\0b"] {
            config.proxy.no_proxy = vec![value.into()];
            assert!(
                config.validate().is_err(),
                "entry {value:?} must be rejected"
            );
        }
        config.proxy.no_proxy = vec!["a".repeat(255); 32];
        assert!(
            config.validate().is_err(),
            "joined value exceeds 4096 bytes"
        );
        config.proxy.no_proxy = vec!["localhost".into()];
        assert!(config.validate().is_ok());
    }

    #[test]
    fn reads_version_for_targeted_repair_messaging() {
        let path = std::env::temp_dir().join(format!(
            "codex-proxy-guard-version-probe-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "version = 3\n[proxy]\nhost = \"127.0.0.1\"\n").unwrap();
        assert_eq!(GuardConfig::read_version(&path), Some(3));
        std::fs::write(&path, "not toml").unwrap();
        assert_eq!(GuardConfig::read_version(&path), None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn formats_ipv6_proxy_url() {
        let mut config = GuardConfig::default();
        config.proxy.host = "::1".into();
        assert_eq!(config.proxy_url(), "http://[::1]:10808");
    }
}
