use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose};
use percent_encoding::percent_decode_str;
use proxy_guard_core::SubscriptionProtocol;
use serde_json::{Value, json};
use url::Url;

use crate::NetworkError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionFormat {
    ShareLinks,
    Base64ShareLinks,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeCandidate {
    pub name: String,
    pub protocol: SubscriptionProtocol,
    pub outbound: Value,
    pub remote_key: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedSubscription {
    pub format: SubscriptionFormat,
    pub fetched: usize,
    pub unsupported: usize,
    pub failed: usize,
    pub candidates: Vec<NodeCandidate>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SubscriptionParser;

impl SubscriptionParser {
    /// Detect and parse a plain or Base64-wrapped share-link subscription.
    ///
    /// # Errors
    ///
    /// Returns a redacted parsing error when the response is malformed or has no
    /// valid supported nodes.
    pub fn parse(bytes: &[u8]) -> Result<ParsedSubscription, NetworkError> {
        let text =
            std::str::from_utf8(bytes).map_err(|_| parse_error("response is not valid UTF-8"))?;
        let (format, content) = if contains_share_link(text) {
            (SubscriptionFormat::ShareLinks, text.to_owned())
        } else {
            let decoded = decode_base64(text.trim())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .filter(|decoded| contains_share_link(decoded))
                .ok_or_else(|| parse_error("response is neither share links nor Base64 links"))?;
            (SubscriptionFormat::Base64ShareLinks, decoded)
        };

        let mut candidates = Vec::new();
        let mut unsupported = 0;
        let mut failed = 0;
        let mut fetched = 0;
        for line in content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            fetched += 1;
            let scheme = line
                .split_once("://")
                .map(|(scheme, _)| scheme.to_ascii_lowercase());
            let result = match scheme.as_deref() {
                Some("vless") => parse_vless(line),
                Some("trojan") => parse_trojan(line),
                Some("ss") => parse_shadowsocks(line),
                Some("socks" | "socks5") => parse_socks(line),
                Some(_) => {
                    unsupported += 1;
                    continue;
                }
                None => {
                    failed += 1;
                    continue;
                }
            };
            match result {
                Ok(candidate) => candidates.push(candidate),
                Err(()) => failed += 1,
            }
        }
        if candidates.is_empty() {
            return Err(parse_error(
                "subscription contains no valid supported nodes",
            ));
        }
        Ok(ParsedSubscription {
            format,
            fetched,
            unsupported,
            failed,
            candidates,
        })
    }
}

fn parse_vless(link: &str) -> Result<NodeCandidate, ()> {
    let url = Url::parse(link).map_err(|_| ())?;
    let server = url.host_str().filter(|value| !value.is_empty()).ok_or(())?;
    let port = url.port().ok_or(())?;
    let uuid = decode(url.username());
    if uuid.is_empty() {
        return Err(());
    }
    let query = query_map(&url);
    let mut outbound = json!({
        "type": "vless",
        "server": server,
        "server_port": port,
        "uuid": uuid,
    });
    let object = outbound.as_object_mut().ok_or(())?;
    if let Some(flow) = nonempty(query.get("flow")) {
        object.insert("flow".to_owned(), json!(flow));
    }
    let security = query
        .get("security")
        .map(String::as_str)
        .unwrap_or_default();
    if matches!(security, "tls" | "reality") {
        let mut tls = json!({"enabled": true});
        let tls_object = tls.as_object_mut().ok_or(())?;
        if let Some(sni) = nonempty(query.get("sni")) {
            tls_object.insert("server_name".to_owned(), json!(sni));
        }
        if let Some(fingerprint) = nonempty(query.get("fp")) {
            tls_object.insert(
                "utls".to_owned(),
                json!({"enabled": true, "fingerprint": fingerprint}),
            );
        }
        if security == "reality" {
            let public_key = nonempty(query.get("pbk")).ok_or(())?;
            let short_id = nonempty(query.get("sid")).unwrap_or_default();
            tls_object.insert(
                "reality".to_owned(),
                json!({
                    "enabled": true,
                    "public_key": public_key,
                    "short_id": short_id,
                }),
            );
        }
        object.insert("tls".to_owned(), tls);
    }
    if let Some(transport) = nonempty(query.get("type")).filter(|value| *value != "tcp") {
        let mut document = json!({"type": transport});
        match transport {
            "grpc" => {
                if let Some(service_name) = nonempty(query.get("servicename")) {
                    document["service_name"] = json!(service_name);
                }
            }
            "ws" => {
                if let Some(path) = nonempty(query.get("path")) {
                    document["path"] = json!(path);
                }
                if let Some(host) = nonempty(query.get("host")) {
                    document["headers"] = json!({"Host": host});
                }
            }
            _ => {}
        }
        object.insert("transport".to_owned(), document);
    }
    Ok(candidate(link, &url, SubscriptionProtocol::Vless, outbound))
}

fn parse_trojan(link: &str) -> Result<NodeCandidate, ()> {
    let url = Url::parse(link).map_err(|_| ())?;
    let server = url.host_str().filter(|value| !value.is_empty()).ok_or(())?;
    let port = url.port().ok_or(())?;
    let password = decode(url.username());
    if password.is_empty() {
        return Err(());
    }
    let query = query_map(&url);
    let mut tls = json!({"enabled": true});
    if let Some(sni) = nonempty(query.get("sni")) {
        tls["server_name"] = json!(sni);
    }
    Ok(candidate(
        link,
        &url,
        SubscriptionProtocol::Trojan,
        json!({
            "type": "trojan",
            "server": server,
            "server_port": port,
            "password": password,
            "tls": tls,
        }),
    ))
}

fn parse_socks(link: &str) -> Result<NodeCandidate, ()> {
    let normalized = link.replacen("socks5://", "socks://", 1);
    let url = Url::parse(&normalized).map_err(|_| ())?;
    let server = url.host_str().filter(|value| !value.is_empty()).ok_or(())?;
    let port = url.port().ok_or(())?;
    let mut outbound = json!({
        "type": "socks",
        "server": server,
        "server_port": port,
    });
    if !url.username().is_empty() {
        outbound["username"] = json!(decode(url.username()));
    }
    if let Some(password) = url.password() {
        outbound["password"] = json!(decode(password));
    }
    Ok(candidate(link, &url, SubscriptionProtocol::Socks, outbound))
}

fn parse_shadowsocks(link: &str) -> Result<NodeCandidate, ()> {
    let body = link.strip_prefix("ss://").ok_or(())?;
    let (without_fragment, fragment) = body.split_once('#').unwrap_or((body, ""));
    let without_query = without_fragment
        .split_once('?')
        .map_or(without_fragment, |x| x.0);
    let decoded_whole = if without_query.contains('@') {
        without_query.to_owned()
    } else {
        String::from_utf8(decode_base64(without_query).ok_or(())?).map_err(|_| ())?
    };
    let (credentials, endpoint) = decoded_whole.rsplit_once('@').ok_or(())?;
    let credentials = if credentials.contains(':') {
        credentials.to_owned()
    } else {
        String::from_utf8(decode_base64(credentials).ok_or(())?).map_err(|_| ())?
    };
    let (method, password) = credentials.split_once(':').ok_or(())?;
    if method.is_empty() || password.is_empty() {
        return Err(());
    }
    let endpoint_url = Url::parse(&format!("socks://unused@{endpoint}")).map_err(|_| ())?;
    let server = endpoint_url.host_str().ok_or(())?;
    let port = endpoint_url.port().ok_or(())?;
    Ok(NodeCandidate {
        name: decoded_name(fragment, server, port),
        protocol: SubscriptionProtocol::Shadowsocks,
        outbound: json!({
            "type": "shadowsocks",
            "server": server,
            "server_port": port,
            "method": method,
            "password": password,
        }),
        remote_key: remote_key(link),
    })
}

fn candidate(
    link: &str,
    url: &Url,
    protocol: SubscriptionProtocol,
    outbound: Value,
) -> NodeCandidate {
    let server = url.host_str().unwrap_or("node");
    let port = url.port().unwrap_or_default();
    NodeCandidate {
        name: decoded_name(url.fragment().unwrap_or_default(), server, port),
        protocol,
        outbound,
        remote_key: remote_key(link),
    }
}

fn decoded_name(fragment: &str, server: &str, port: u16) -> String {
    let name = decode(fragment);
    if name.trim().is_empty() {
        format!("{server}:{port}")
    } else {
        name.trim().to_owned()
    }
}

fn query_map(url: &Url) -> HashMap<String, String> {
    url.query_pairs()
        .map(|(key, value)| (key.to_ascii_lowercase(), value.into_owned()))
        .collect()
}

fn nonempty(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|value| !value.is_empty())
}

fn decode(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

/// Opaque identity of the remote share link, ignoring the `#` display fragment so
/// that a display-name-only rename does not change the node's stable key.
fn remote_key(link: &str) -> String {
    let identity = link
        .trim()
        .split_once('#')
        .map_or(link.trim(), |parts| parts.0);
    blake3::hash(identity.as_bytes()).to_hex().to_string()
}

fn contains_share_link(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ["vless://", "trojan://", "ss://", "socks://", "socks5://"]
        .iter()
        .any(|scheme| lower.contains(scheme))
        || lower.lines().any(|line| line.contains("://"))
}

fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let compact = value
        .chars()
        .filter(|char| !char.is_whitespace())
        .collect::<String>();
    for engine in [
        &general_purpose::STANDARD,
        &general_purpose::STANDARD_NO_PAD,
        &general_purpose::URL_SAFE,
        &general_purpose::URL_SAFE_NO_PAD,
    ] {
        if let Ok(bytes) = engine.decode(compact.as_bytes()) {
            return Some(bytes);
        }
    }
    None
}

fn parse_error(reason: impl Into<String>) -> NetworkError {
    NetworkError::Parse(reason.into())
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose};
    use serde_json::json;

    use super::{SubscriptionFormat, SubscriptionParser};

    const VLESS: &str = "vless://11111111-1111-1111-1111-111111111111@example.com:443?security=reality&flow=xtls-rprx-vision&sni=cdn.example.com&fp=chrome&pbk=public-key&sid=abcd&type=grpc&serviceName=verified#Tokyo";

    #[test]
    fn parses_plain_vless_reality() {
        let parsed = SubscriptionParser::parse(VLESS.as_bytes()).expect("parse");
        assert_eq!(parsed.format, SubscriptionFormat::ShareLinks);
        assert_eq!(parsed.candidates[0].outbound["type"], "vless");
        assert_eq!(
            parsed.candidates[0].outbound["tls"]["reality"]["public_key"],
            "public-key"
        );
        assert_eq!(parsed.candidates[0].outbound["flow"], "xtls-rprx-vision");
        assert_eq!(
            parsed.candidates[0].outbound["transport"]["service_name"],
            "verified"
        );
        assert!(parsed.candidates[0].outbound.get("tag").is_none());
    }

    #[test]
    fn parses_base64_wrapped_links_and_skips_unsupported() {
        let content = format!("{VLESS}\nvmess://unsupported");
        let encoded = general_purpose::STANDARD.encode(content);
        let parsed = SubscriptionParser::parse(encoded.as_bytes()).expect("parse");
        assert_eq!(parsed.format, SubscriptionFormat::Base64ShareLinks);
        assert_eq!(parsed.unsupported, 1);
        assert_eq!(parsed.candidates.len(), 1);
    }

    #[test]
    fn preserves_vless_websocket_host_and_path() {
        let link = "vless://11111111-1111-1111-1111-111111111111@example.com:443?security=tls&sni=cdn.example.com&type=ws&host=edge.example.com&path=%2Fverified#WS";
        let parsed = SubscriptionParser::parse(link.as_bytes()).expect("parse");

        assert_eq!(parsed.candidates[0].outbound["transport"]["type"], "ws");
        assert_eq!(
            parsed.candidates[0].outbound["transport"]["path"],
            "/verified"
        );
        assert_eq!(
            parsed.candidates[0].outbound["transport"]["headers"]["Host"],
            "edge.example.com"
        );
    }

    #[test]
    fn parses_trojan_shadowsocks_and_socks() {
        let ss_credentials = general_purpose::STANDARD_NO_PAD.encode("aes-256-gcm:secret");
        let body = format!(
            "trojan://password@example.com:443?sni=example.com#Trojan\nss://{ss_credentials}@example.net:8388#SS\nsocks://user:pass@example.org:1080#SOCKS"
        );
        let parsed = SubscriptionParser::parse(body.as_bytes()).expect("parse");
        assert_eq!(parsed.candidates.len(), 3);
        assert_eq!(parsed.candidates[0].outbound["type"], json!("trojan"));
        assert_eq!(parsed.candidates[1].outbound["type"], json!("shadowsocks"));
        assert_eq!(parsed.candidates[2].outbound["type"], json!("socks"));
    }

    #[test]
    fn remote_key_ignores_the_display_fragment() {
        let body = "socks://one.example:1080#JP-Tokyo\nsocks://one.example:1080#JP-Tokyo-Renamed";
        let parsed = SubscriptionParser::parse(body.as_bytes()).expect("parse");
        assert_eq!(
            parsed.candidates[0].remote_key,
            parsed.candidates[1].remote_key
        );
        assert_ne!(parsed.candidates[0].name, parsed.candidates[1].name);
    }
}
