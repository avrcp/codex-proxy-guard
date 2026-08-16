use proxy_guard_core::GuardError;
use thiserror::Error;

/// Redacted failures emitted by the managed-network layer.
///
/// Every `Display` implementation is safe to surface in the TUI or CLI: it never
/// contains the subscription URL, outbound credentials, or a full outbound document.
#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("SUBSCRIPTION_CREDENTIAL: operation failed")]
    Credential,
    #[error("SUBSCRIPTION_URL: {0}")]
    SubscriptionUrl(String),
    #[error("SUBSCRIPTION_FETCH: {0}")]
    Fetch(String),
    #[error("SUBSCRIPTION_PARSE: {0}")]
    Parse(String),
    #[error("SUBSCRIPTION_STORAGE: {0}")]
    Storage(String),
    #[error("SUBSCRIPTION_NOT_FOUND")]
    NotFound,
    #[error("NODE: {0}")]
    Node(String),
    #[error("SING_BOX: {0}")]
    SingBox(String),
    #[error("GEO: {0}")]
    Geo(String),
    #[error("BENCHMARK: {0}")]
    Benchmark(String),
    #[error("CANCELLED: {0}")]
    Cancelled(String),
}

impl From<GuardError> for NetworkError {
    fn from(error: GuardError) -> Self {
        Self::Node(error.to_string())
    }
}

impl From<NetworkError> for String {
    fn from(error: NetworkError) -> Self {
        error.to_string()
    }
}
