use std::{io::Read, time::Duration};

use proxy_guard_core::SubscriptionId;
use reqwest::{blocking::Client, redirect::Policy};
use url::Url;

use crate::NetworkError;

pub const HTTPS_RESPONSE_LIMIT: u64 = 5 * 1024 * 1024;
pub const HTTPS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const HTTPS_TOTAL_TIMEOUT: Duration = Duration::from_secs(15);

pub trait SubscriptionFetcher: Send + Sync {
    /// Fetch one credential-bearing subscription without exposing its URL.
    ///
    /// # Errors
    ///
    /// Returns a redacted network, status, or size-limit error.
    fn fetch(&self, subscription_id: SubscriptionId, url: &str) -> Result<Vec<u8>, NetworkError>;
}

/// Bounded HTTPS client that fetches over the host's real network, never through
/// the managed node (so a broken node can never block a subscription refresh).
#[derive(Clone, Debug)]
pub struct HttpsSubscriptionFetcher {
    client: Client,
}

impl HttpsSubscriptionFetcher {
    /// Construct the bounded HTTPS client with system proxies disabled.
    ///
    /// # Errors
    ///
    /// Returns a redacted configuration error if the client cannot be built.
    pub fn new() -> Result<Self, NetworkError> {
        let client = Client::builder()
            .connect_timeout(HTTPS_CONNECT_TIMEOUT)
            .timeout(HTTPS_TOTAL_TIMEOUT)
            .redirect(Policy::limited(3))
            .no_proxy()
            .user_agent(concat!("codex-proxy-guard/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| NetworkError::Fetch("could not initialize the HTTPS client".into()))?;
        Ok(Self { client })
    }

    /// Validate the subscription transport without retaining or reporting the URL.
    ///
    /// # Errors
    ///
    /// Returns a redacted configuration error unless the URL is HTTPS with a host.
    pub fn validate_url(url: &str) -> Result<Url, NetworkError> {
        let parsed =
            Url::parse(url).map_err(|_| NetworkError::SubscriptionUrl("URL is invalid".into()))?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err(NetworkError::SubscriptionUrl("URL must use HTTPS".into()));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(NetworkError::SubscriptionUrl(
                "URL must not contain HTTP user information".into(),
            ));
        }
        Ok(parsed)
    }
}

impl SubscriptionFetcher for HttpsSubscriptionFetcher {
    fn fetch(&self, _subscription_id: SubscriptionId, url: &str) -> Result<Vec<u8>, NetworkError> {
        let url = Self::validate_url(url)?;
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|_| NetworkError::Fetch("request failed".into()))?;
        if !response.status().is_success() {
            return Err(NetworkError::Fetch(format!(
                "server returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let content_length = response.content_length();
        read_bounded(response, content_length)
    }
}

fn read_bounded(reader: impl Read, content_length: Option<u64>) -> Result<Vec<u8>, NetworkError> {
    if content_length.is_some_and(|length| length > HTTPS_RESPONSE_LIMIT) {
        return Err(NetworkError::Fetch("response exceeded 5 MiB".into()));
    }
    let mut body = Vec::new();
    reader
        .take(HTTPS_RESPONSE_LIMIT + 1)
        .read_to_end(&mut body)
        .map_err(|_| NetworkError::Fetch("response could not be read".into()))?;
    if u64::try_from(body.len()).unwrap_or(u64::MAX) > HTTPS_RESPONSE_LIMIT {
        return Err(NetworkError::Fetch("response exceeded 5 MiB".into()));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::{HTTPS_RESPONSE_LIMIT, HttpsSubscriptionFetcher, read_bounded};

    #[test]
    fn https_is_required() {
        for rejected in [
            "http://example.com/sub",
            "file:///tmp/sub",
            "data:text/plain,test",
            "javascript:alert(1)",
        ] {
            HttpsSubscriptionFetcher::validate_url(rejected).expect_err("must reject");
        }
        HttpsSubscriptionFetcher::validate_url("https://example.com/sub?token=secret")
            .expect("HTTPS URL");
    }

    #[test]
    fn validation_errors_do_not_echo_the_secret_url() {
        let secret = "not-a-url-token-123";
        let error = HttpsSubscriptionFetcher::validate_url(secret).expect_err("invalid");
        assert!(!error.to_string().contains(secret));
    }

    #[test]
    fn response_size_limit_checks_header_and_stream() {
        read_bounded(&b"small"[..], Some(5)).expect("small response");
        let error = read_bounded(&b"small"[..], Some(HTTPS_RESPONSE_LIMIT + 1))
            .expect_err("oversized header");
        assert!(error.to_string().contains("exceeded 5 MiB"));

        let oversized = vec![0_u8; usize::try_from(HTTPS_RESPONSE_LIMIT + 1).expect("size")];
        read_bounded(&oversized[..], None).expect_err("oversized stream");
    }
}
