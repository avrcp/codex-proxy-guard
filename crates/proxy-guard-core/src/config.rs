use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

use crate::GuardError;

pub const CONFIG_VERSION: u32 = 3;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GuardConfig {
    pub version: u32,
    pub proxy: ProxyConfig,
    pub managed: ManagedConfig,
    pub codex: CodexConfig,
    pub tui: TuiConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub no_proxy: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    #[default]
    External,
    Managed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ManagedConfig {
    pub subscription_id: String,
    pub sing_box_path: PathBuf,
    pub benchmark_cache_hours: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CodexConfig {
    pub executable_override: PathBuf,
    pub refuse_if_running: bool,
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
            managed: ManagedConfig::default(),
            codex: CodexConfig::default(),
            tui: TuiConfig::default(),
        }
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            mode: ProxyMode::External,
            scheme: "http".into(),
            host: "127.0.0.1".into(),
            port: 10808,
            no_proxy: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
        }
    }
}

impl Default for ManagedConfig {
    fn default() -> Self {
        Self {
            subscription_id: String::new(),
            sing_box_path: PathBuf::new(),
            benchmark_cache_hours: 6,
        }
    }
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            executable_override: PathBuf::new(),
            refuse_if_running: true,
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

    pub fn data_dir() -> Result<PathBuf, GuardError> {
        BaseDirs::new()
            .map(|dirs| dirs.config_dir().join("codex-proxy-guard"))
            .ok_or_else(|| {
                GuardError::Config("cannot resolve the user configuration directory".into())
            })
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
        if self
            .proxy
            .no_proxy
            .iter()
            .any(|value| value.len() > 255 || value.contains(['\r', '\n']))
        {
            return Err(GuardError::Config(
                "proxy.no_proxy contains an invalid entry".into(),
            ));
        }
        if self.managed.benchmark_cache_hours == 0 || self.managed.benchmark_cache_hours > 720 {
            return Err(GuardError::Config(
                "managed.benchmark_cache_hours must be between 1 and 720".into(),
            ));
        }
        if self.proxy.mode == ProxyMode::Managed && self.managed.subscription_id.trim().is_empty() {
            return Err(GuardError::Config(
                "proxy.mode = managed requires a non-empty managed.subscription_id".into(),
            ));
        }
        Ok(())
    }

    pub fn is_managed(&self) -> bool {
        self.proxy.mode == ProxyMode::Managed
    }

    pub fn proxy_url(&self) -> String {
        let host = if self.proxy.host.contains(':') {
            format!("[{}]", self.proxy.host)
        } else {
            self.proxy.host.clone()
        };
        format!("http://{host}:{}", self.proxy.port)
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
        assert_eq!(table.len(), 5);
        assert!(table.contains_key("proxy"));
        assert!(table.contains_key("managed"));
        assert!(table.contains_key("codex"));
        assert!(table.contains_key("tui"));
    }

    #[test]
    fn version_one_is_rejected_without_migration() {
        let error = GuardConfig::parse("version = 1").unwrap_err();
        assert!(error.contains("unsupported configuration version"));
    }

    #[test]
    fn version_two_is_rejected_without_migration() {
        let error = GuardConfig::parse("version = 2").unwrap_err();
        assert!(error.contains("unsupported configuration version"));
    }

    #[test]
    fn current_version_rejects_legacy_fields() {
        let error = GuardConfig::parse(
            r#"
                version = 3
                [proxy]
                host = "127.0.0.1"
                remove_all_proxy = true
            "#,
        )
        .unwrap_err();
        assert!(error.contains("unknown field"));
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
    fn managed_mode_requires_a_subscription() {
        let mut config = GuardConfig::default();
        config.proxy.mode = ProxyMode::Managed;
        assert!(config.validate().is_err());
        config.managed.subscription_id = "id".into();
        assert!(config.validate().is_ok());
        assert!(config.is_managed());
    }

    #[test]
    fn formats_ipv6_proxy_url() {
        let mut config = GuardConfig::default();
        config.proxy.host = "::1".into();
        assert_eq!(config.proxy_url(), "http://[::1]:10808");
    }
}
