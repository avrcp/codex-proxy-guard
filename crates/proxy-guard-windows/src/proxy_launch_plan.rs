//! Deterministic proxy launch plans derived from Guard's configuration.
//!
//! Pure functions only — no I/O, no network. Two layers come out of here and
//! they stay separate facts:
//!
//! * Chromium arguments (`--proxy-server`, `--proxy-bypass-list`) submitted
//!   through the activation arguments; they cover Electron/Chromium's HTTP
//!   paths, not arbitrary Node/Rust backend traffic.
//! * The `.env` values for the authorized Codex Home block, which is applied
//!   by `proxy_env_file` only after explicit user consent.
//!
//! `NO_PROXY` and the Chromium bypass list are different grammars. Loopback
//! entries map one-to-one; anything that cannot be mapped equivalently is an
//! actionable error rather than a silently dropped rule.

use std::net::IpAddr;

use proxy_guard_core::GuardConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyLaunchPlan {
    /// Loopback HTTP proxy URL, e.g. `http://127.0.0.1:10808`.
    pub proxy_url: String,
    /// Semicolon-separated Chromium bypass list, e.g. `localhost;127.0.0.1;[::1]`.
    pub bypass_list: String,
    /// The full activation argument string; empty when there is no proxy plan.
    pub chromium_arguments: String,
}

impl ProxyLaunchPlan {
    pub fn no_proxy_arguments() -> Self {
        Self {
            proxy_url: String::new(),
            bypass_list: String::new(),
            chromium_arguments: String::new(),
        }
    }
}

/// Builds the deterministic launch plan from a validated configuration.
pub fn proxy_launch_plan(config: &GuardConfig) -> Result<ProxyLaunchPlan, String> {
    let bypass_entries: Vec<String> = config
        .proxy
        .no_proxy
        .iter()
        .map(|entry| map_bypass_entry(entry))
        .collect::<Result<_, _>>()?;
    let bypass_list = bypass_entries.join(";");
    let proxy_url = config.proxy_url();
    if !proxy_url.is_empty() {
        // Keep the composed string bounded like every other external text.
        if proxy_url.len() > 2048 || bypass_list.len() > 4096 {
            return Err(
                "PROXY_LAUNCH_PLAN_INVALID: the composed proxy arguments exceed their budget"
                    .into(),
            );
        }
    }
    Ok(ProxyLaunchPlan {
        proxy_url: proxy_url.clone(),
        bypass_list: bypass_list.clone(),
        chromium_arguments: format!(
            "--proxy-server=\"{proxy_url}\" --proxy-bypass-list=\"{bypass_list}\""
        ),
    })
}

/// Maps one `no_proxy` entry to its Chromium bypass-list equivalent. Only
/// entries with an exact equivalent are accepted: `localhost` and loopback IP
/// literals (`::1` becomes `[::1]`). Everything else — wildcards, domain
/// suffixes, CIDR blocks, `<local>` — is rejected because dropping or
/// approximating it would silently change what bypasses the proxy.
fn map_bypass_entry(entry: &str) -> Result<String, String> {
    let mapped = if entry.eq_ignore_ascii_case("localhost") {
        "localhost".to_string()
    } else if let Ok(address) = entry.parse::<IpAddr>() {
        if !address.is_loopback() {
            return Err(unsupported_bypass_entry(entry));
        }
        match address {
            IpAddr::V4(_) => entry.to_string(),
            IpAddr::V6(_) => format!("[{entry}]"),
        }
    } else {
        return Err(unsupported_bypass_entry(entry));
    };
    Ok(mapped)
}

fn unsupported_bypass_entry(entry: &str) -> String {
    format!(
        "PROXY_BYPASS_UNSUPPORTED: the no_proxy entry {entry:?} has no exact Chromium \
         bypass-list equivalent; remove it from proxy.no_proxy (loopback entries and \
         localhost are mapped automatically)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_configuration_builds_the_documented_arguments() {
        let plan = proxy_launch_plan(&GuardConfig::default()).unwrap();
        assert_eq!(plan.proxy_url, "http://127.0.0.1:10808");
        assert_eq!(plan.bypass_list, "localhost;127.0.0.1;[::1]");
        assert_eq!(
            plan.chromium_arguments,
            "--proxy-server=\"http://127.0.0.1:10808\" --proxy-bypass-list=\"localhost;127.0.0.1;[::1]\""
        );
    }

    #[test]
    fn ipv6_loopback_hosts_are_bracketed_once() {
        let mut config = GuardConfig::default();
        config.proxy.host = "::1".into();
        config.proxy.no_proxy = vec!["localhost".into(), "::1".into()];
        let plan = proxy_launch_plan(&config).unwrap();
        assert_eq!(plan.proxy_url, "http://[::1]:10808");
        assert_eq!(plan.bypass_list, "localhost;[::1]");
    }

    #[test]
    fn non_loopback_or_wildcard_entries_are_actionable_errors() {
        let mut config = GuardConfig::default();
        for entry in [
            "example.com",
            ".example.com",
            "*.example.com",
            "*",
            "<local>",
            "10.0.0.0/8",
            "8.8.8.8",
        ] {
            config.proxy.no_proxy = vec!["localhost".into(), entry.into()];
            let error = proxy_launch_plan(&config).unwrap_err();
            assert!(
                error.starts_with("PROXY_BYPASS_UNSUPPORTED:"),
                "{entry}: {error}"
            );
            assert!(error.contains(entry), "the error must name the entry");
        }
    }

    #[test]
    fn arguments_are_printable_ascii_compatible() {
        let plan = proxy_launch_plan(&GuardConfig::default()).unwrap();
        assert!(
            plan.chromium_arguments
                .chars()
                .all(|c| c == ' ' || c.is_ascii_graphic()),
            "activation arguments must satisfy the worker's printable-ASCII validation"
        );
    }
}
