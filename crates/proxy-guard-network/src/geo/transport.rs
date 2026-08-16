use std::{error::Error as _, io::Read, time::Duration};

use reqwest::{Proxy, blocking::Client, redirect::Policy};

use crate::{LoopbackProxyEndpoint, NetworkError};

/// Transport boundary for a Geo request that must traverse one ready sidecar.
pub trait GeoTransport: std::fmt::Debug + Send + Sync {
    /// Fetch one bounded provider response through the supplied loopback proxy.
    ///
    /// # Errors
    ///
    /// Returns a typed proxy, HTTP status, timeout, size, or response-read error.
    fn fetch(
        &self,
        provider: &str,
        endpoint: &str,
        proxy_endpoint: LoopbackProxyEndpoint,
        timeout: Duration,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, NetworkError>;
}

/// Blocking Rustls client with explicit proxy-only routing and bounded I/O.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReqwestGeoTransport;

impl GeoTransport for ReqwestGeoTransport {
    fn fetch(
        &self,
        provider: &str,
        endpoint: &str,
        proxy_endpoint: LoopbackProxyEndpoint,
        timeout: Duration,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, NetworkError> {
        let proxy = Proxy::all(proxy_endpoint.proxy_url())
            .map_err(|source| request_failed(provider, source.to_string()))?;
        let client = Client::builder()
            .no_proxy()
            .proxy(proxy)
            .https_only(true)
            .redirect(Policy::none())
            .connect_timeout(timeout.min(Duration::from_secs(5)))
            .timeout(timeout)
            .pool_max_idle_per_host(0)
            .user_agent(concat!("codex-proxy-guard/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|source| request_failed(provider, source.to_string()))?;
        let response = client
            .get(endpoint)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .map_err(|source| request_failed(provider, request_reason(&source)))?;
        let status = response.status();
        if !status.is_success() {
            return Err(NetworkError::Geo(format!(
                "provider {provider} returned HTTP {}",
                status.as_u16()
            )));
        }

        let read_limit = u64::try_from(maximum_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut body = Vec::with_capacity(maximum_bytes.min(16 * 1024));
        response
            .take(read_limit)
            .read_to_end(&mut body)
            .map_err(|source| request_failed(provider, source.to_string()))?;
        if body.len() > maximum_bytes {
            return Err(NetworkError::Geo(format!(
                "provider {provider} response exceeded {maximum_bytes} bytes"
            )));
        }
        Ok(body)
    }
}

fn request_failed(provider: &str, reason: impl Into<String>) -> NetworkError {
    NetworkError::Geo(format!(
        "provider {provider} request failed: {}",
        reason.into()
    ))
}

fn request_reason(error: &reqwest::Error) -> String {
    let category = if error.is_timeout() {
        "request timed out"
    } else if error.is_connect() {
        "connection failed"
    } else if error.is_request() {
        "request failed"
    } else {
        "response failed"
    };
    let mut details = Vec::new();
    let mut source = error.source();
    while let Some(current) = source {
        details.push(current.to_string());
        source = current.source();
        if details.len() == 4 {
            break;
        }
    }
    if details.is_empty() {
        category.to_owned()
    } else {
        format!("{category}: {}", details.join(": "))
    }
}
