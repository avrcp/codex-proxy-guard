//! `config.toml` schema v2: strict parsing (unknown keys, wrong types,
//! non-UTF-8 and >64 KiB rejected) and the byte-exact toml++ canonical
//! serialization the C++ product writes (alphabetical tables/keys, literal
//! `'…'` strings unless the value contains `'` or control characters, arrays
//! as `[ 'a', 'b' ]`).

use crate::error::{Error, Result};
use crate::proxy::is_loopback_host;
use crate::text::strict_utf8;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CONFIG_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub version: i64,
    pub scheme: String,
    pub host: String,
    pub port: i64,
    pub no_proxy: Vec<String>,
    pub executable_override: PathBuf,
    pub cli_override: PathBuf,
    pub refuse_if_running: bool,
    pub manage_backend: bool,
    pub home: PathBuf,
    pub alternate_screen: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: 2,
            scheme: "http".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 10808,
            no_proxy: vec![
                "localhost".to_owned(),
                "127.0.0.1".to_owned(),
                "::1".to_owned(),
            ],
            executable_override: PathBuf::new(),
            cli_override: PathBuf::new(),
            refuse_if_running: true,
            manage_backend: false,
            home: PathBuf::new(),
            alternate_screen: "auto".to_owned(),
        }
    }
}

fn invalid(message: &str) -> Error {
    Error::new("CONFIG_INVALID", message)
}

fn ascii_trimmed(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_ascii_whitespace())
}

impl Config {
    pub fn proxy_url(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("http://{}:{}", host, self.port)
    }

    pub fn no_proxy_value(&self) -> String {
        self.no_proxy.join(",")
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 2 {
            return Err(invalid(
                "Unsupported configuration version; expected version 2.",
            ));
        }
        if !self.scheme.eq_ignore_ascii_case("http") {
            return Err(invalid("Proxy scheme must be http."));
        }
        if !(1..=65535).contains(&self.port) {
            return Err(Error::with_field(
                "CONFIG_INVALID",
                "Proxy port must be between 1 and 65535.",
                Some("proxy.port"),
            ));
        }
        if self.host.is_empty()
            || ascii_trimmed(&self.host) != self.host
            || self.host.contains('\0')
        {
            return Err(Error::with_field(
                "CONFIG_INVALID",
                "Invalid proxy host.",
                Some("proxy.host"),
            ));
        }
        if !is_loopback_host(&self.host) {
            return Err(Error::with_field(
                "CONFIG_INVALID",
                "Proxy host must be localhost or a loopback IP address.",
                Some("proxy.host"),
            ));
        }
        if self.no_proxy.is_empty()
            || self.no_proxy.len() > 32
            || self.no_proxy_value().len() > 4096
        {
            return Err(invalid(
                "Proxy exclusions must contain 1 to 32 bounded entries.",
            ));
        }
        for entry in &self.no_proxy {
            if entry.is_empty()
                || ascii_trimmed(entry) != entry.as_str()
                || entry.len() > 255
                || entry.contains('\r')
                || entry.contains('\n')
                || entry.contains('\0')
            {
                return Err(invalid("Proxy exclusions contain an invalid entry."));
            }
        }
        if !matches!(self.alternate_screen.as_str(), "auto" | "always" | "never") {
            return Err(invalid("Invalid alternate-screen setting."));
        }
        Ok(())
    }

    pub fn parse(bytes: &[u8]) -> Result<Config> {
        if bytes.len() > CONFIG_LIMIT || !strict_utf8(bytes) {
            return Err(invalid(
                "Configuration exceeds 64 KiB or is not valid UTF-8.",
            ));
        }
        let text = std::str::from_utf8(bytes).expect("checked above");
        let table: toml::Table =
            toml::from_str(text).map_err(|_| invalid("Configuration is not valid TOML."))?;
        Self::from_table(&table)
    }

    fn from_table(table: &toml::Table) -> Result<Config> {
        let mut config = Config::default();
        check_keys(table, &["version", "proxy", "codex", "tui"])?;
        if let Some(value) = table.get("version") {
            config.version = int_field(value, "version")?;
        }
        if let Some(proxy) = subtable(table, "proxy")? {
            check_keys(proxy, &["scheme", "host", "port", "no_proxy"])?;
            string_into(proxy, "scheme", &mut config.scheme)?;
            string_into(proxy, "host", &mut config.host)?;
            if let Some(value) = proxy.get("port") {
                config.port = int_field(value, "proxy.port")?;
            }
            if let Some(value) = proxy.get("no_proxy") {
                let items = value
                    .as_array()
                    .ok_or_else(|| invalid("Proxy exclusions must be an array."))?;
                let mut entries = Vec::with_capacity(items.len());
                for item in items {
                    entries.push(
                        item.as_str()
                            .ok_or_else(|| invalid("Proxy exclusions must be strings."))?
                            .to_owned(),
                    );
                }
                config.no_proxy = entries;
            }
        }
        if let Some(codex) = subtable(table, "codex")? {
            check_keys(
                codex,
                &[
                    "executable_override",
                    "cli_executable_override",
                    "refuse_if_running",
                    "manage_codex_proxy_env",
                    "proxy_env_home",
                ],
            )?;
            path_into(
                codex,
                "executable_override",
                &mut config.executable_override,
            )?;
            path_into(codex, "cli_executable_override", &mut config.cli_override)?;
            path_into(codex, "proxy_env_home", &mut config.home)?;
            if let Some(value) = codex.get("refuse_if_running") {
                config.refuse_if_running = bool_field(value)?;
            }
            if let Some(value) = codex.get("manage_codex_proxy_env") {
                config.manage_backend = bool_field(value)?;
            }
        }
        if let Some(tui) = subtable(table, "tui")? {
            check_keys(tui, &["alternate_screen"])?;
            string_into(tui, "alternate_screen", &mut config.alternate_screen)?;
        }
        config.validate()?;
        Ok(config)
    }

    /// Canonical serialization, byte-identical to the C++ `Config::toml()`.
    pub fn to_toml(&self) -> Result<String> {
        self.validate()?;
        let mut out = String::new();
        out.push_str(&format!("version = {}\n", self.version));
        let codex: Vec<(&str, TomlValue)> = vec![
            (
                "cli_executable_override",
                TomlValue::Str(path_text(&self.cli_override)),
            ),
            (
                "executable_override",
                TomlValue::Str(path_text(&self.executable_override)),
            ),
            (
                "manage_codex_proxy_env",
                TomlValue::Bool(self.manage_backend),
            ),
            ("proxy_env_home", TomlValue::Str(path_text(&self.home))),
            ("refuse_if_running", TomlValue::Bool(self.refuse_if_running)),
        ];
        write_table(&mut out, "codex", &codex);
        let proxy: Vec<(&str, TomlValue)> = vec![
            ("host", TomlValue::Str(self.host.clone())),
            ("no_proxy", TomlValue::Array(self.no_proxy.clone())),
            ("port", TomlValue::Int(self.port)),
            ("scheme", TomlValue::Str(self.scheme.clone())),
        ];
        write_table(&mut out, "proxy", &proxy);
        let tui: Vec<(&str, TomlValue)> = vec![(
            "alternate_screen",
            TomlValue::Str(self.alternate_screen.clone()),
        )];
        write_table(&mut out, "tui", &tui);
        Ok(out)
    }
}

fn path_text(path: &Path) -> String {
    // Matches wideToUtf8(path.wstring()): the raw component text. Config
    // paths originate from TOML strings, so they are always valid Unicode.
    path.to_str().unwrap_or_default().to_owned()
}

enum TomlValue {
    Str(String),
    Int(i64),
    Bool(bool),
    Array(Vec<String>),
}

fn write_table(out: &mut String, name: &str, entries: &[(&str, TomlValue)]) {
    out.push_str(&format!("\n[{name}]\n"));
    for (key, value) in entries {
        match value {
            TomlValue::Str(text) => out.push_str(&format!("{key} = {}\n", toml_string(text))),
            TomlValue::Int(number) => out.push_str(&format!("{key} = {number}\n")),
            TomlValue::Bool(flag) => out.push_str(&format!("{key} = {flag}\n")),
            TomlValue::Array(items) => {
                let joined: Vec<String> = items.iter().map(|item| toml_string(item)).collect();
                out.push_str(&format!("{key} = [ {} ]\n", joined.join(", ")));
            }
        }
    }
}

fn toml_string(value: &str) -> String {
    let literal_ok = value.chars().all(|c| c != '\'' && !is_toml_control(c));
    if literal_ok {
        return format!("'{value}'");
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if is_toml_control(c) => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn is_toml_control(c: char) -> bool {
    let value = c as u32;
    value < 0x20 || (0x7f..=0x9f).contains(&value)
}

fn check_keys(table: &toml::Table, allowed: &[&str]) -> Result<()> {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(invalid("Configuration contains an unknown field."));
        }
    }
    Ok(())
}

fn subtable<'a>(table: &'a toml::Table, key: &str) -> Result<Option<&'a toml::Table>> {
    match table.get(key) {
        None => Ok(None),
        Some(toml::Value::Table(sub)) => Ok(Some(sub)),
        Some(_) => Err(invalid("Configuration section has the wrong type.")),
    }
}

fn string_into(table: &toml::Table, key: &str, slot: &mut String) -> Result<()> {
    if let Some(value) = table.get(key) {
        *slot = value
            .as_str()
            .ok_or_else(|| invalid("Configuration string has the wrong type."))?
            .to_owned();
    }
    Ok(())
}

fn path_into(table: &toml::Table, key: &str, slot: &mut PathBuf) -> Result<()> {
    if let Some(value) = table.get(key) {
        let text = value
            .as_str()
            .ok_or_else(|| invalid("Configuration string has the wrong type."))?;
        *slot = PathBuf::from(text);
    }
    Ok(())
}

fn int_field(value: &toml::Value, field: &str) -> Result<i64> {
    let number = value
        .as_integer()
        .ok_or_else(|| invalid("Configuration integer has the wrong type."))?;
    if number < i32::MIN as i64 || number > i32::MAX as i64 {
        return Err(invalid("Configuration integer is out of range."));
    }
    let _ = field;
    Ok(number)
}

fn bool_field(value: &toml::Value) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| invalid("Configuration boolean has the wrong type."))
}

/// Case-insensitive containment over escape-decoded TOML key spellings, used
/// by the consent-preservation guard for unreadable configurations.
pub fn may_contain_consent(bytes: &[u8]) -> bool {
    if !strict_utf8(bytes) {
        return true;
    }
    let text = decode_toml_escapes(std::str::from_utf8(bytes).unwrap_or(""));
    let folded = text.to_ascii_lowercase();
    folded.contains("manage_codex_proxy_env") || folded.contains("proxy_env_home")
}

fn decode_toml_escapes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'\\' || index + 1 >= bytes.len() {
            out.push(bytes[index] as char);
            index += 1;
            continue;
        }
        index += 1;
        let kind = bytes[index];
        let width = match kind {
            b'u' => 4,
            b'U' => 8,
            _ => {
                out.push('\\');
                out.push(kind as char);
                index += 1;
                continue;
            }
        };
        if index + width >= bytes.len() {
            out.push('\\');
            out.push(kind as char);
            index += 1;
            continue;
        }
        let mut point: u32 = 0;
        let mut valid = true;
        for step in 1..=width {
            let digit = bytes[index + step];
            let nibble = match digit {
                b'0'..=b'9' => u32::from(digit - b'0'),
                b'a'..=b'f' => u32::from(digit - b'a' + 10),
                b'A'..=b'F' => u32::from(digit - b'A' + 10),
                _ => {
                    valid = false;
                    break;
                }
            };
            point = (point << 4) | nibble;
        }
        if !valid || point > 0x10ffff || (0xd800..=0xdfff).contains(&point) {
            out.push('\\');
            out.push(kind as char);
            index += 1;
            continue;
        }
        if let Some(character) = char::from_u32(point) {
            out.push(character);
        }
        index += width + 1;
    }
    out
}

/// `BTreeMap` adapter used by callers that need ordered key views.
pub type OrderedTable = BTreeMap<String, toml::Value>;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(text: &str) -> Config {
        Config::parse(text.as_bytes()).expect("parse")
    }

    #[test]
    fn default_serialization_matches_tomlpp_bytes() {
        let text = Config::default().to_toml().unwrap();
        assert_eq!(
            text,
            "version = 2\n\n\
             [codex]\n\
             cli_executable_override = ''\n\
             executable_override = ''\n\
             manage_codex_proxy_env = false\n\
             proxy_env_home = ''\n\
             refuse_if_running = true\n\n\
             [proxy]\n\
             host = '127.0.0.1'\n\
             no_proxy = [ 'localhost', '127.0.0.1', '::1' ]\n\
             port = 10808\n\
             scheme = 'http'\n\n\
             [tui]\n\
             alternate_screen = 'auto'\n"
        );
    }

    #[test]
    fn round_trips_golden_default() {
        let golden = std::fs::read_to_string("../../tests/golden/default-config.toml").unwrap();
        let config = Config::parse(golden.as_bytes()).unwrap();
        assert_eq!(config.to_toml().unwrap(), golden);
    }

    #[test]
    fn round_trips_apostrophe_and_cjk_goldens() {
        for name in [
            "config-roundtrip-apostrophe.toml",
            "config-roundtrip-cjk.toml",
            "config-plain-defaults.toml",
        ] {
            let golden = std::fs::read_to_string(format!("../../tests/golden/{name}")).unwrap();
            let config =
                Config::parse(golden.as_bytes()).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(config.to_toml().unwrap(), golden, "{name} roundtrip");
        }
    }

    #[test]
    fn parses_minimal_and_defaults_absent_fields() {
        let config = parse_ok("version = 2\n");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 10808);
        assert_eq!(config.no_proxy.len(), 3);
        assert!(!config.manage_backend);
    }

    #[test]
    fn rejects_unknown_keys_wrong_types_and_bad_values() {
        assert!(Config::parse(b"version = 2\nextra = 1\n").is_err());
        assert!(Config::parse(b"version = 2\n[proxy]\nextra = 1\n").is_err());
        assert!(Config::parse(b"version = 3\n").is_err());
        assert!(Config::parse(b"version = \"2\"\n").is_err());
        assert!(Config::parse(b"version = 2\n[proxy]\nport = \"x\"\n").is_err());
        assert!(Config::parse(b"version = 2\n[proxy]\nport = 99999\n").is_err());
        assert!(Config::parse(b"version = 2\n[proxy]\nhost = \"example.com\"\n").is_err());
        assert!(Config::parse(b"version = 2\n[proxy]\nhost = \" 127.0.0.1\"\n").is_err());
        assert!(Config::parse(b"version = 2\n[proxy]\nno_proxy = []\n").is_err());
        assert!(Config::parse(b"version = 2\n[proxy]\nno_proxy = [1]\n").is_err());
        assert!(Config::parse(b"version = 2\n[tui]\nalternate_screen = \"sometimes\"\n").is_err());
        assert!(Config::parse(b"version = 2\n[codex]\nrefuse_if_running = 1\n").is_err());
        assert!(Config::parse(b"\xff\xfe").is_err());
        assert!(Config::parse("version = 2\n[proxy]\nhost = \"x\u{2028}\"\n".as_bytes()).is_err());
    }

    #[test]
    fn accepts_ipv6_loopback_and_brackets_url() {
        let config = parse_ok("version = 2\n[proxy]\nhost = \"::1\"\nport = 8080\n");
        assert_eq!(config.proxy_url(), "http://[::1]:8080");
    }

    #[test]
    fn consent_detection_survives_key_escapes_and_bad_utf8() {
        assert!(may_contain_consent(b"manage_codex_proxy_env = true"));
        assert!(may_contain_consent(
            b"\"\\u006danage_codex_proxy_env\" = true"
        ));
        assert!(may_contain_consent(b"PROXY_ENV_HOME = 'C:/x'"));
        assert!(may_contain_consent(&[0xff]));
        assert!(!may_contain_consent(b"version = 2\n"));
    }

    #[test]
    fn oversize_input_rejected() {
        let mut bytes = vec![b'#'; 65 * 1024];
        bytes.extend_from_slice(b"\nversion = 2\n");
        assert!(Config::parse(&bytes).is_err());
    }
}
