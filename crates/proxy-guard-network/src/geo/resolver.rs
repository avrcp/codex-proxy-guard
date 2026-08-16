use std::{sync::Arc, time::Duration};

use chrono::Utc;
use proxy_guard_core::{CodexRegion, ExitObservation};

use crate::{LoopbackProxyEndpoint, NetworkError};

use super::{GeoTransport, IpWhoIsProvider, ReqwestGeoTransport, provider::GeoProvider};

pub const GEO_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const GEO_RESPONSE_MAX_BYTES: usize = 64 * 1024;

/// Resolves live exit identity through exactly one ready loopback proxy.
#[derive(Clone, Debug)]
pub struct GeoResolver {
    provider: Arc<dyn GeoProvider>,
    transport: Arc<dyn GeoTransport>,
    timeout: Duration,
    maximum_bytes: usize,
}

impl Default for GeoResolver {
    fn default() -> Self {
        Self::ipwhois()
    }
}

impl GeoResolver {
    #[must_use]
    pub fn ipwhois() -> Self {
        Self {
            provider: Arc::new(IpWhoIsProvider),
            transport: Arc::new(ReqwestGeoTransport),
            timeout: GEO_REQUEST_TIMEOUT,
            maximum_bytes: GEO_RESPONSE_MAX_BYTES,
        }
    }

    /// Construct the fixed provider resolver with a custom transport boundary.
    #[must_use]
    pub fn with_transport(transport: Arc<dyn GeoTransport>) -> Self {
        Self {
            transport,
            ..Self::ipwhois()
        }
    }

    #[must_use]
    pub const fn with_limits(mut self, timeout: Duration, maximum_bytes: usize) -> Self {
        self.timeout = timeout;
        self.maximum_bytes = maximum_bytes;
        self
    }

    #[must_use]
    pub fn provider_id(&self) -> &'static str {
        self.provider.id()
    }

    /// Perform a strict live query through the supplied loopback proxy and map the
    /// observed country to a fixed `CodexRegion`.
    ///
    /// # Errors
    ///
    /// Returns a typed transport or provider-response error. There is no direct
    /// or host-geography fallback, and a non-JP/SG/US country is rejected.
    pub fn resolve_live(
        &self,
        proxy_endpoint: LoopbackProxyEndpoint,
    ) -> Result<ExitObservation, NetworkError> {
        let body = self.transport.fetch(
            self.provider.id(),
            self.provider.endpoint(),
            proxy_endpoint,
            self.timeout,
            self.maximum_bytes,
        )?;
        let observation = self.provider.parse(&body)?;
        let country =
            CodexRegion::from_country_code(&observation.country_code).ok_or_else(|| {
                NetworkError::Geo(format!(
                    "exit country {} is not an allowed region",
                    observation.country_code
                ))
            })?;
        Ok(ExitObservation {
            ip: observation.exit_ip,
            country,
            observed_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use proxy_guard_core::CodexRegion;

    use super::GeoResolver;
    use crate::{GeoTransport, LoopbackPortReservation, LoopbackProxyEndpoint, NetworkError};

    #[derive(Debug)]
    struct FakeTransport {
        expected_proxy: LoopbackProxyEndpoint,
        body: Vec<u8>,
    }

    impl GeoTransport for FakeTransport {
        fn fetch(
            &self,
            provider: &str,
            endpoint: &str,
            proxy_endpoint: LoopbackProxyEndpoint,
            _timeout: Duration,
            maximum_bytes: usize,
        ) -> Result<Vec<u8>, NetworkError> {
            assert_eq!(provider, "ipwhois");
            assert!(endpoint.starts_with("https://ipwho.is/"));
            assert_eq!(proxy_endpoint, self.expected_proxy);
            assert!(maximum_bytes >= self.body.len());
            let _ = Mutex::new(());
            Ok(self.body.clone())
        }
    }

    #[test]
    fn resolver_maps_to_codex_region_through_the_proxy() {
        let reservation = LoopbackPortReservation::reserve().expect("port");
        let endpoint = reservation.endpoint();
        let transport = Arc::new(FakeTransport {
            expected_proxy: endpoint,
            body: br#"{"ip":"8.8.8.8","success":true,"country_code":"JP"}"#.to_vec(),
        });
        let resolver = GeoResolver::with_transport(transport);

        let observation = resolver.resolve_live(endpoint).expect("observation");
        assert_eq!(observation.ip.to_string(), "8.8.8.8");
        assert_eq!(observation.country, CodexRegion::JP);
    }

    #[test]
    fn resolver_rejects_non_allowed_countries() {
        let reservation = LoopbackPortReservation::reserve().expect("port");
        let endpoint = reservation.endpoint();
        let transport = Arc::new(FakeTransport {
            expected_proxy: endpoint,
            body: br#"{"ip":"8.8.8.8","success":true,"country_code":"KR"}"#.to_vec(),
        });
        let resolver = GeoResolver::with_transport(transport);

        let error = resolver
            .resolve_live(endpoint)
            .expect_err("KR must be rejected");
        assert!(error.to_string().contains("not an allowed region"));
    }
}
