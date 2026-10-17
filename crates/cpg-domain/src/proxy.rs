//! Strict proxy endpoint validation: ASCII port parsing and loopback host
//! checks with `InetPtonW` parsing semantics (full dotted-quad IPv4, full
//! IPv6 text with `::` compression and embedded IPv4, no shorthands).

use crate::error::{Error, Result};

pub fn parse_port_strict(text: &str) -> Result<u16> {
    let invalid_range = || {
        Error::with_field(
            "CONFIG_INVALID",
            "Proxy port must be an integer from 1 to 65535.",
            Some("proxy.port"),
        )
    };
    if text.is_empty() || text.len() > 5 {
        return Err(invalid_range());
    }
    let mut value: u32 = 0;
    for digit in text.bytes() {
        if !digit.is_ascii_digit() {
            return Err(Error::with_field(
                "CONFIG_INVALID",
                "Proxy port must contain only ASCII digits.",
                Some("proxy.port"),
            ));
        }
        value = value * 10 + u32::from(digit - b'0');
    }
    if value == 0 || value > 65535 {
        return Err(invalid_range());
    }
    Ok(value as u16)
}

/// Strict dotted-quad IPv4 parse (each octet 0-255, no leading zeros except
/// the digit 0 itself) — `InetPtonW(AF_INET, …)` semantics.
pub fn parse_ipv4(text: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut octets = [0u8; 4];
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if part.len() > 1 && part.starts_with('0') {
            return None;
        }
        octets[index] = part.parse::<u16>().ok().filter(|v| *v <= 255)? as u8;
    }
    Some(octets)
}

/// Strict IPv6 parse into eight groups, supporting `::` compression (at most
/// once) and an optional trailing embedded IPv4 quad — `InetPtonW(AF_INET6)`
/// semantics. Zone ids (`%scope`) are refused like InetPton.
pub fn parse_ipv6(text: &str) -> Option<[u16; 8]> {
    if text.is_empty() || text.len() > 45 {
        return None;
    }
    let (head, tail) = match text.split_once("::") {
        Some((head, tail)) => (head, Some(tail)),
        None => (text, None),
    };
    let parse_groups = |section: &str| -> Option<Vec<u16>> {
        if section.is_empty() {
            return Some(Vec::new());
        }
        let mut groups = Vec::new();
        let parts: Vec<&str> = section.split(':').collect();
        for (index, part) in parts.iter().enumerate() {
            let is_last = index + 1 == parts.len();
            if is_last && part.contains('.') {
                let quad = parse_ipv4(part)?;
                groups.push(u16::from_be_bytes([quad[0], quad[1]]));
                groups.push(u16::from_be_bytes([quad[2], quad[3]]));
            } else {
                if part.is_empty() || part.len() > 4 || !part.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return None;
                }
                groups.push(u16::from_str_radix(part, 16).ok()?);
            }
        }
        Some(groups)
    };
    let head_groups = parse_groups(head)?;
    let tail_groups = match tail {
        Some(tail) => parse_groups(tail)?,
        None => {
            if head_groups.len() != 8 {
                return None;
            }
            let mut groups = [0u16; 8];
            groups.copy_from_slice(&head_groups);
            return Some(groups);
        }
    };
    let total = head_groups.len() + tail_groups.len();
    // Reject an over-full expansion before any width arithmetic: the plain
    // subtraction below would underflow (and panic in Debug builds) on
    // inputs like `1:2:3:4:5:6:7:8::9`.
    let fill = 8usize.checked_sub(total)?;
    // With an explicit "::" the fill must be >= 1 unless the whole address is
    // exactly "::" — "1:2:3:4:5:6:7:8::" is invalid IPv6 text.
    let whole_address_is_unspecified = head.is_empty() && tail.unwrap().is_empty();
    if !whole_address_is_unspecified && fill < 1 {
        return None;
    }
    let mut groups = [0u16; 8];
    groups[..head_groups.len()].copy_from_slice(&head_groups);
    groups[8 - tail_groups.len()..].copy_from_slice(&tail_groups);
    Some(groups)
}

/// Loopback classification with the C++ `Config::validate` semantics:
/// `localhost` (case-insensitive), IPv4 in 127/8, or IPv6 `::1`.
pub fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Some(octets) = parse_ipv4(host) {
        return octets[0] == 127;
    }
    if let Some(groups) = parse_ipv6(host) {
        return groups[..7].iter().all(|g| *g == 0) && groups[7] == 1;
    }
    false
}

/// Chromium `--proxy-bypass-list` spelling for a `no_proxy` entry
/// (`launch.cpp bypassEntry` semantics).
pub fn bypass_entry(entry: &str) -> Result<String> {
    if entry.eq_ignore_ascii_case("localhost") {
        return Ok("localhost".to_owned());
    }
    if let Some(octets) = parse_ipv4(entry)
        && octets[0] == 127
    {
        return Ok(entry.to_owned());
    }
    if let Some(groups) = parse_ipv6(entry)
        && groups[..7].iter().all(|g| *g == 0)
        && groups[7] == 1
    {
        return Ok(format!("[{entry}]"));
    }
    Err(Error::new(
        "PROXY_BYPASS_UNSUPPORTED",
        "A no_proxy entry has no exact Chromium bypass equivalent; use only localhost or loopback IP literals.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_parsing_bounds() {
        assert_eq!(parse_port_strict("1").unwrap(), 1);
        assert_eq!(parse_port_strict("65535").unwrap(), 65535);
        assert_eq!(parse_port_strict("10808").unwrap(), 10808);
        assert!(parse_port_strict("0").is_err());
        assert!(parse_port_strict("65536").is_err());
        assert!(parse_port_strict("").is_err());
        assert!(parse_port_strict("12a").is_err());
        assert!(parse_port_strict(" 12").is_err());
        assert!(
            parse_port_strict("＋1").is_err(),
            "non-ASCII digits rejected"
        );
    }

    #[test]
    fn ipv4_strict() {
        assert_eq!(parse_ipv4("127.0.0.1"), Some([127, 0, 0, 1]));
        assert_eq!(parse_ipv4("127.255.0.1"), Some([127, 255, 0, 1]));
        assert_eq!(parse_ipv4("0.0.0.0"), Some([0, 0, 0, 0]));
        assert_eq!(parse_ipv4("127.1"), None);
        assert_eq!(parse_ipv4("127.01.0.1"), None, "leading zeros refused");
        assert_eq!(parse_ipv4("256.0.0.1"), None);
        assert_eq!(parse_ipv4("::1"), None);
    }

    #[test]
    fn ipv6_loopback_forms() {
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("0:0:0:0:0:0:0:1"));
        assert!(is_loopback_host("::0.0.0.1"));
        assert!(!is_loopback_host("::2"));
        assert!(!is_loopback_host("::1:1"));
        assert!(!is_loopback_host("2001:db8::1"));
        assert!(!is_loopback_host("::1%1"), "zone ids refused");
        assert!(parse_ipv6("::").is_some());
        assert!(parse_ipv6("1:2:3:4:5:6:7:8").is_some());
        assert!(parse_ipv6("1:2:3:4:5:6:7:8:9").is_none());
        assert!(parse_ipv6("1::2::3").is_none());
        assert!(parse_ipv6("::ffff:127.0.0.1").is_some());
    }

    #[test]
    fn ipv6_overfull_expansion_is_rejected_without_panicking() {
        // Nine groups with a `::` made the old width subtraction underflow
        // (a Debug-build panic) before the `total > 8` rejection ran.
        let overfull = [
            "1:2:3:4:5:6:7:8::9",
            "9::1:2:3:4:5:6:7:8",
            "::1:2:3:4:5:6:7:8:9",
            "1:2:3:4:5:6:7:8::",
            "::1:2:3:4:5:6:7:8",
            "1:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1:1",
            "1:2:3:4:5:6:7:1.2.3.4::9",
        ];
        for text in overfull {
            let result = std::panic::catch_unwind(|| parse_ipv6(text));
            assert!(
                result.is_ok(),
                "parse of {text:?} panicked instead of rejecting"
            );
            assert!(result.unwrap().is_none(), "{text:?} must be rejected");
        }
        // Adjacent boundaries stay classified: one fill group is valid,
        // eight explicit groups plus `::` are not (InetPton semantics).
        assert!(parse_ipv6("1:2:3:4:5:6::8").is_some());
        assert!(parse_ipv6("::ffff:1.2.3.4").is_some());
        assert!(parse_ipv6("1:2:3:4:5:6:7::8").is_none());
    }

    #[test]
    fn loopback_classification() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LOCALHOST"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.3.4.5"));
        assert!(!is_loopback_host("128.0.0.1"));
        assert!(!is_loopback_host("169.254.1.1"));
        assert!(!is_loopback_host("example.com"));
        assert!(!is_loopback_host("::1."));
    }

    #[test]
    fn bypass_entries() {
        assert_eq!(bypass_entry("LOCALHOST").unwrap(), "localhost");
        assert_eq!(bypass_entry("127.0.0.1").unwrap(), "127.0.0.1");
        assert_eq!(bypass_entry("::1").unwrap(), "[::1]");
        assert_eq!(
            bypass_entry("0:0:0:0:0:0:0:1").unwrap(),
            "[0:0:0:0:0:0:0:1]"
        );
        assert!(bypass_entry("example.com").is_err());
    }
}
