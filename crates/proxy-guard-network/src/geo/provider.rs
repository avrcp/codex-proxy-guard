use std::net::IpAddr;

use serde::Deserialize;

use crate::NetworkError;

pub const IPWHOIS_PROVIDER_ID: &str = "ipwhois";
const IPWHOIS_ENDPOINT: &str = "https://ipwho.is/?fields=ip,success,message,country,country_code,region,city,latitude,longitude,timezone.id";

/// Raw exit identity observed through one managed proxy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeoObservation {
    pub exit_ip: IpAddr,
    pub country_code: String,
}

pub(crate) trait GeoProvider: std::fmt::Debug + Send + Sync {
    fn id(&self) -> &'static str;
    fn endpoint(&self) -> &'static str;
    fn parse(&self, body: &[u8]) -> Result<GeoObservation, NetworkError>;
}

/// Adapter for the fixed, no-key `ipwho.is` HTTPS endpoint.
#[derive(Clone, Copy, Debug, Default)]
pub struct IpWhoIsProvider;

impl GeoProvider for IpWhoIsProvider {
    fn id(&self) -> &'static str {
        IPWHOIS_PROVIDER_ID
    }

    fn endpoint(&self) -> &'static str {
        IPWHOIS_ENDPOINT
    }

    fn parse(&self, body: &[u8]) -> Result<GeoObservation, NetworkError> {
        let response: IpWhoIsResponse = serde_json::from_slice(body)
            .map_err(|source| invalid_response(self.id(), source.to_string()))?;
        if !response.success {
            return Err(invalid_response(
                self.id(),
                response.message.as_deref().map_or_else(
                    || "provider reported an unsuccessful response".to_owned(),
                    sanitize_message,
                ),
            ));
        }
        let exit_ip = response
            .ip
            .parse()
            .map_err(|source: std::net::AddrParseError| {
                invalid_response(self.id(), source.to_string())
            })?;
        let country_code = response.country_code.trim().to_ascii_uppercase();
        if country_code.len() != 2 || !country_code.bytes().all(|byte| byte.is_ascii_uppercase()) {
            return Err(invalid_response(self.id(), "country_code is invalid"));
        }
        Ok(GeoObservation {
            exit_ip,
            country_code,
        })
    }
}

#[derive(Debug, Deserialize)]
struct IpWhoIsResponse {
    #[serde(default)]
    ip: String,
    #[serde(default)]
    success: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    country_code: String,
}

fn sanitize_message(message: &str) -> String {
    message
        .chars()
        .filter(|character| !character.is_control())
        .take(200)
        .collect()
}

fn invalid_response(provider: &str, reason: impl Into<String>) -> NetworkError {
    NetworkError::Geo(format!(
        "provider {provider} returned invalid data: {}",
        reason.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::{GeoProvider, IpWhoIsProvider};

    #[test]
    fn parses_a_complete_exit_observation() {
        let observation = IpWhoIsProvider
            .parse(
                br#"{"ip":"8.8.8.8","success":true,"country":"United States","country_code":"US","timezone":{"id":"America/Los_Angeles"}}"#,
            )
            .expect("observation");

        assert_eq!(observation.exit_ip.to_string(), "8.8.8.8");
        assert_eq!(observation.country_code, "US");
    }

    #[test]
    fn rejects_provider_failure_and_malformed_country() {
        for body in [
            br#"{"success":false,"message":"rate limit"}"#.as_slice(),
            br#"{"ip":"8.8.8.8","success":true,"country_code":"usa"}"#.as_slice(),
        ] {
            assert!(IpWhoIsProvider.parse(body).is_err());
        }
    }
}
