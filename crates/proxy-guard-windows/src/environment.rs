use std::process::Command;

use proxy_guard_core::GuardConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyEnvironment {
    pub proxy_url: String,
    pub no_proxy: String,
}

pub fn proxy_environment(config: &GuardConfig) -> ProxyEnvironment {
    ProxyEnvironment {
        proxy_url: config.proxy_url(),
        no_proxy: config.no_proxy_value(),
    }
}

pub fn apply_proxy_environment(command: &mut Command, environment: &ProxyEnvironment) {
    command
        .env("HTTP_PROXY", &environment.proxy_url)
        .env("HTTPS_PROXY", &environment.proxy_url)
        .env("http_proxy", &environment.proxy_url)
        .env("https_proxy", &environment.proxy_url)
        .env("NO_PROXY", &environment.no_proxy)
        .env("no_proxy", &environment.no_proxy)
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_contains_only_the_proxy_contract() {
        let environment = proxy_environment(&GuardConfig::default());
        assert_eq!(environment.proxy_url, "http://127.0.0.1:10808");
        assert!(environment.no_proxy.contains("localhost"));
    }
}
