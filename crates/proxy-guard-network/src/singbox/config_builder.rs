use proxy_guard_core::ManagedNode;
use serde_json::{Map, Value, json};

use super::LoopbackProxyEndpoint;
use crate::NetworkError;

pub const GUARD_INBOUND_TAG: &str = "guard-in";
pub const ACTIVE_NODE_OUTBOUND_TAG: &str = "active-node";

/// Central builder for Guard-owned sing-box documents.
#[derive(Clone, Copy, Debug)]
pub struct SingBoxConfigBuilder<'a> {
    node: &'a ManagedNode,
    endpoint: LoopbackProxyEndpoint,
}

impl<'a> SingBoxConfigBuilder<'a> {
    #[must_use]
    pub const fn guard(node: &'a ManagedNode, endpoint: LoopbackProxyEndpoint) -> Self {
        Self { node, endpoint }
    }

    /// Build one fail-closed Guard configuration.
    ///
    /// # Errors
    ///
    /// Returns a typed node or configuration error. Full schema compatibility
    /// remains enforced by the mandatory `sing-box check` step.
    pub fn build(self) -> Result<Value, NetworkError> {
        self.node.validate().map_err(NetworkError::from)?;
        let address = self.endpoint.socket_addr();
        if !address.is_ipv4() || !address.ip().is_loopback() || address.port() == 0 {
            return Err(NetworkError::SingBox(
                "mixed inbound must use a non-zero IPv4 loopback endpoint".into(),
            ));
        }
        let mut outbound = self
            .node
            .outbound
            .document()
            .as_object()
            .cloned()
            .ok_or_else(|| NetworkError::SingBox("node outbound is not an object".into()))?;
        insert_string(&mut outbound, "tag", ACTIVE_NODE_OUTBOUND_TAG);

        Ok(json!({
            "log": { "disabled": true },
            "inbounds": [{
                "type": "mixed",
                "tag": GUARD_INBOUND_TAG,
                "listen": "127.0.0.1",
                "listen_port": self.endpoint.port(),
                "set_system_proxy": false
            }],
            "outbounds": [Value::Object(outbound)],
            "route": {
                "rules": [{
                    "inbound": [GUARD_INBOUND_TAG],
                    "action": "route",
                    "outbound": ACTIVE_NODE_OUTBOUND_TAG
                }],
                "final": ACTIVE_NODE_OUTBOUND_TAG
            }
        }))
    }
}

fn insert_string(object: &mut Map<String, Value>, key: &str, value: &str) {
    object.insert(key.to_owned(), Value::String(value.to_owned()));
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddrV4};

    use proxy_guard_core::{CodexRegion, ManagedNode, SingBoxOutbound, SubscriptionId};
    use serde_json::json;

    use super::{ACTIVE_NODE_OUTBOUND_TAG, GUARD_INBOUND_TAG, SingBoxConfigBuilder};
    use crate::LoopbackProxyEndpoint;

    #[test]
    fn builds_one_loopback_mixed_inbound_and_one_forced_outbound() {
        let node = ManagedNode::new(
            "JP Tokyo",
            SubscriptionId::new(),
            CodexRegion::JP,
            SingBoxOutbound::new(json!({
                "type": "socks",
                "server": "proxy.example",
                "server_port": 1080
            }))
            .expect("outbound"),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("node");
        let endpoint = LoopbackProxyEndpoint::from_socket_addr_v4(SocketAddrV4::new(
            Ipv4Addr::LOCALHOST,
            39_081,
        ));

        let document = SingBoxConfigBuilder::guard(&node, endpoint)
            .build()
            .expect("config");

        assert_eq!(document["inbounds"].as_array().expect("inbounds").len(), 1);
        assert_eq!(document["inbounds"][0]["type"], "mixed");
        assert_eq!(document["inbounds"][0]["tag"], GUARD_INBOUND_TAG);
        assert_eq!(document["inbounds"][0]["listen"], "127.0.0.1");
        assert_eq!(document["inbounds"][0]["listen_port"], 39_081);
        assert_eq!(document["inbounds"][0]["set_system_proxy"], false);
        assert_eq!(
            document["outbounds"].as_array().expect("outbounds").len(),
            1
        );
        assert_eq!(document["outbounds"][0]["tag"], ACTIVE_NODE_OUTBOUND_TAG);
        assert_eq!(document["route"]["final"], ACTIVE_NODE_OUTBOUND_TAG);
        assert_eq!(
            document["route"]["rules"][0]["outbound"],
            ACTIVE_NODE_OUTBOUND_TAG
        );
        assert!(node.outbound.document().get("tag").is_none());
    }
}
