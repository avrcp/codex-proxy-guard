use std::{
    fmt,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener},
};

use serde::Serialize;

use crate::NetworkError;

/// A non-zero ephemeral TCP endpoint bound exclusively to IPv4 loopback.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct LoopbackProxyEndpoint(SocketAddrV4);

impl LoopbackProxyEndpoint {
    pub(crate) const fn from_socket_addr_v4(address: SocketAddrV4) -> Self {
        Self(address)
    }

    #[must_use]
    pub const fn socket_addr(self) -> SocketAddr {
        SocketAddr::V4(self.0)
    }

    #[must_use]
    pub const fn port(self) -> u16 {
        self.0.port()
    }

    #[must_use]
    pub fn proxy_url(self) -> String {
        format!("http://{self}")
    }
}

impl fmt::Display for LoopbackProxyEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Holds an ephemeral loopback port until the validated sidecar is ready to spawn.
#[derive(Debug)]
pub struct LoopbackPortReservation {
    listener: TcpListener,
    endpoint: LoopbackProxyEndpoint,
}

impl LoopbackPortReservation {
    /// Bind `127.0.0.1:0` and retain ownership of the selected port.
    ///
    /// # Errors
    ///
    /// Returns a typed runtime error when the operating system cannot reserve a port.
    pub fn reserve() -> Result<Self, NetworkError> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .map_err(|source| NetworkError::SingBox(format!("reserve loopback port: {source}")))?;
        let address = listener
            .local_addr()
            .map_err(|source| NetworkError::SingBox(format!("read reserved port: {source}")))?;
        let SocketAddr::V4(address) = address else {
            return Err(NetworkError::SingBox(
                "the operating system returned a non-IPv4 reservation".into(),
            ));
        };
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(NetworkError::SingBox(
                "the reserved endpoint is not a non-zero IPv4 loopback socket".into(),
            ));
        }
        Ok(Self {
            listener,
            endpoint: LoopbackProxyEndpoint::from_socket_addr_v4(address),
        })
    }

    #[must_use]
    pub const fn endpoint(&self) -> LoopbackProxyEndpoint {
        self.endpoint
    }

    pub(crate) fn release(self) {
        drop(self.listener);
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::LoopbackPortReservation;

    #[test]
    fn reservation_is_ipv4_loopback_and_exclusive_until_release() {
        let reservation = LoopbackPortReservation::reserve().expect("reserve port");
        let endpoint = reservation.endpoint();

        assert!(endpoint.socket_addr().is_ipv4());
        assert!(endpoint.socket_addr().ip().is_loopback());
        assert_ne!(endpoint.port(), 0);
        assert!(TcpListener::bind(endpoint.socket_addr()).is_err());

        reservation.release();
        let rebound = TcpListener::bind(endpoint.socket_addr()).expect("port released");
        drop(rebound);
    }
}
