use std::{
    fmt::Debug,
    time::{Duration, Instant},
};

use proxy_guard_core::PathSample;
use reqwest::{blocking::Client, redirect::Policy};

use crate::{LoopbackProxyEndpoint, NetworkError};

pub const CODEX_PATH_URL: &str = "https://chatgpt.com/";
pub const CODEX_PATH_TIMEOUT: Duration = Duration::from_secs(10);

/// Lightweight HTTPS-path reachability probe for one managed proxy.
pub trait CodexPathProbe: Debug + Send + Sync {
    /// Perform one HEAD request through the supplied loopback proxy.
    ///
    /// Success means the HTTP request completed the proxy CONNECT/TLS handshake
    /// and received response headers; the business status is only diagnostic.
    ///
    /// # Errors
    ///
    /// Returns a redacted network error when no response headers were received.
    fn probe(&self, proxy: LoopbackProxyEndpoint) -> Result<PathSample, NetworkError>;
}

/// Blocking Rustls client that opens a fresh connection per probe so each sample
/// measures the full CONNECT + TLS + response-header path independently.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReqwestCodexPathProbe;

impl CodexPathProbe for ReqwestCodexPathProbe {
    fn probe(&self, proxy: LoopbackProxyEndpoint) -> Result<PathSample, NetworkError> {
        let client = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(CODEX_PATH_TIMEOUT.min(Duration::from_secs(5)))
            .timeout(CODEX_PATH_TIMEOUT)
            .pool_max_idle_per_host(0)
            .proxy(
                reqwest::Proxy::all(proxy.proxy_url())
                    .map_err(|source| NetworkError::Benchmark(format!("build proxy: {source}")))?,
            )
            .build()
            .map_err(|source| NetworkError::Benchmark(format!("build client: {source}")))?;
        let started = Instant::now();
        let response = client.head(CODEX_PATH_URL).send().map_err(|source| {
            NetworkError::Benchmark(format!("HEAD failed: {}", request_reason(&source)))
        })?;
        let header_latency = started.elapsed();
        Ok(PathSample {
            header_latency,
            http_status: response.status().as_u16(),
        })
    }
}

fn request_reason(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "request timed out".to_owned()
    } else if error.is_connect() {
        "connection failed".to_owned()
    } else {
        "request failed".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::CODEX_PATH_URL;

    #[test]
    fn probe_target_is_the_codex_path() {
        assert!(CODEX_PATH_URL.starts_with("https://chatgpt.com/"));
    }
}
