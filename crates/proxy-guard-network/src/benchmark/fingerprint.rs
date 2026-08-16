use serde_json::Value;

/// Canonical, order-insensitive fingerprint of a node outbound.
///
/// The fingerprint is derived from the canonical JSON of the outbound document so
/// that any protocol parameter change invalidates cached benchmark reports, while
/// key ordering never does.
#[must_use]
pub fn node_fingerprint(outbound: &Value) -> String {
    let canonical = canonicalize(outbound);
    let encoded = serde_json::to_string(&canonical).unwrap_or_default();
    blake3::hash(encoded.as_bytes()).to_hex().to_string()
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(entries.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::node_fingerprint;

    #[test]
    fn fingerprint_is_key_order_insensitive_but_value_sensitive() {
        let a = json!({"type": "vless", "server": "example.com", "uuid": "x"});
        let b = json!({"uuid": "x", "server": "example.com", "type": "vless"});
        let c = json!({"type": "vless", "server": "example.com", "uuid": "y"});

        assert_eq!(node_fingerprint(&a), node_fingerprint(&b));
        assert_ne!(node_fingerprint(&a), node_fingerprint(&c));
        assert_eq!(node_fingerprint(&a).len(), 64);
    }
}
