//! Pure logic for the consented Codex Home `.env` managed block: the
//! read-only lexical gate (every physical line must be an independent
//! single-line dotenv construct), block recognition, conflict detection,
//! Guard-generated value validation and byte-splice computation. All file
//! I/O and the TxF transaction live in `cpg-engine`; this module decides
//! bytes only.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::proxy::{is_loopback_host, parse_port_strict};

pub const ENV_LIMIT: usize = 1024 * 1024;
pub const BLOCK_BEGIN: &str = "# BEGIN CODEX PROXY GUARD: proxy-v1";
pub const BLOCK_END: &str = "# END CODEX PROXY GUARD: proxy-v1";

/// One-based number of the first line the lexical gate refuses to edit, or
/// `None` when the whole file consists of independent single-line constructs.
/// Direct port of the C++ `unsafeEnvLine` state machine.
pub fn unsafe_env_line(bytes: &str) -> Option<usize> {
    #[derive(Clone, Copy, PartialEq)]
    enum Ctx {
        Start,
        Key,
        Value,
        Comment,
    }
    let mut ctx = Ctx::Start;
    let mut in_quote = false;
    let mut quote_char = '\0';
    let mut escaped = false;
    let mut spaced = false;
    let mut line = 1usize;
    let bytes = bytes.as_bytes();
    let mut offset = if bytes.starts_with("\u{feff}".as_bytes()) {
        "\u{feff}".len()
    } else {
        0
    };
    while offset < bytes.len() {
        let ch = bytes[offset] as char; // ASCII decisions only; UTF-8 bytes >= 0x80 fall through as inert values.
        let byte = bytes[offset];
        if in_quote {
            if byte == b'\n' {
                return Some(line);
            }
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote_char as u8 {
                in_quote = false;
                quote_char = '\0';
            }
            offset += 1;
            continue;
        }
        if byte == b'\n' {
            ctx = Ctx::Start;
            escaped = false;
            spaced = false;
            line += 1;
            offset += 1;
            continue;
        }
        if ctx == Ctx::Comment {
            offset += 1;
            continue;
        }
        if escaped {
            escaped = false;
            offset += 1;
            continue;
        }
        if byte == b'\\' {
            // Outside quotes dotenv joins a trailing backslash with the next
            // physical line; refuse to guess what that continuation means.
            if offset + 1 >= bytes.len() || bytes[offset + 1] == b'\n' {
                return Some(line);
            }
            escaped = true;
            offset += 1;
            continue;
        }
        if ctx == Ctx::Start {
            if byte == b' ' || byte == b'\t' {
                offset += 1;
                continue;
            }
            if byte == b'#' {
                ctx = Ctx::Comment;
                offset += 1;
                continue;
            }
            ctx = Ctx::Key;
        }
        if ctx == Ctx::Key {
            if byte == b'=' {
                ctx = Ctx::Value;
                spaced = false;
                offset += 1;
                continue;
            }
            if byte == b'\'' || byte == b'"' {
                return Some(line); // quoted key: assignment boundary undecidable
            }
            if byte == b'#' {
                ctx = Ctx::Comment;
                offset += 1;
                continue;
            }
            offset += 1;
            continue;
        }
        // Unquoted value context: '#' only starts a trailing comment after
        // whitespace; before that it is literal value content.
        if byte == b'\'' || byte == b'"' {
            in_quote = true;
            quote_char = ch;
            offset += 1;
            continue;
        }
        if (byte == b' ' || byte == b'\t') && !spaced {
            spaced = true;
            offset += 1;
            continue;
        }
        if byte == b'#' && spaced {
            ctx = Ctx::Comment;
        }
        offset += 1;
    }
    if in_quote || escaped {
        return Some(line);
    }
    None
}

/// A `no_proxy` value Guard could itself have generated: single physical
/// line, ≤4096 bytes, partitionable into at most 32 valid bounded entries
/// (entries themselves may contain commas). Direct port of `generatedNoProxy`.
pub fn generated_no_proxy(bytes: &str) -> bool {
    if bytes.is_empty() || bytes.len() > 4096 || bytes.contains(['\r', '\n', '\0']) {
        return false;
    }
    let text = bytes.as_bytes();
    // Boundary positions; index 0 is the virtual "before the first byte"
    // boundary so entry spans are (previous_boundary, next_boundary).
    let mut boundaries: Vec<usize> = vec![0];
    let mut fewest: Vec<usize> = vec![0];
    for (index, byte) in text.iter().enumerate() {
        if *byte == b',' {
            boundaries.push(index + 1);
            fewest.push(33);
        }
    }
    boundaries.push(text.len() + 1);
    fewest.push(33);
    for index in 0..boundaries.len() - 1 {
        if fewest[index] >= 32 {
            continue;
        }
        for next in (index + 1)..boundaries.len() {
            // Entry bytes are (boundaries[index], boundaries[next]-1), i.e.
            // between the previous comma (exclusive) and the next comma.
            let start = boundaries[index];
            let stop = boundaries[next] - 1;
            let length = stop.saturating_sub(start);
            if length > 255 {
                break;
            }
            if length == 0 {
                continue;
            }
            let entry = &bytes[start..stop];
            if entry.trim_matches(|c: char| c.is_ascii_whitespace()).len() != entry.len() {
                continue;
            }
            fewest[next] = fewest[next].min(fewest[index] + 1);
        }
    }
    fewest.last().map(|last| *last <= 32).unwrap_or(false)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnvFile {
    pub exists: bool,
    pub bytes: String,
    /// Byte offset of the managed block's BEGIN line (excluding any BOM).
    pub begin: Option<usize>,
    /// Byte offset one past the managed block's END newline.
    pub end: Option<usize>,
    pub values: std::collections::BTreeMap<String, String>,
    pub conflict: bool,
}

impl EnvFile {
    pub fn has_block(&self) -> bool {
        self.begin.is_some()
    }
}

fn block_invalid<T>(message: &str) -> Result<T> {
    Err(Error::new("BACKEND_PROXY_BLOCK_INVALID", message))
}

fn ascii_trimmed(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_ascii_whitespace())
}

fn ascii_folded(value: &str) -> String {
    value.to_ascii_lowercase()
}

fn starts_with(text: &str, prefix: &str) -> bool {
    text.starts_with(prefix)
}

/// Validates the values inside a recognized Guard block: HTTP_PROXY must be a
/// loopback endpoint Guard would generate, match HTTPS_PROXY, and NO_PROXY
/// must be a generated value. Port of `validateManagedValues`.
pub fn validate_managed_values(env: &EnvFile) -> Result<()> {
    let edited = "The managed block proxy endpoint was edited.";
    let http = env.values.get("HTTP_PROXY").ok_or_else(|| {
        Error::new(
            "BACKEND_PROXY_BLOCK_INVALID",
            "The managed block contains values Guard could not have generated.",
        )
    })?;
    let https = env.values.get("HTTPS_PROXY").ok_or_else(|| {
        Error::new(
            "BACKEND_PROXY_BLOCK_INVALID",
            "The managed block contains values Guard could not have generated.",
        )
    })?;
    let no_proxy = env.values.get("NO_PROXY").ok_or_else(|| {
        Error::new(
            "BACKEND_PROXY_BLOCK_INVALID",
            "The managed block contains values Guard could not have generated.",
        )
    })?;
    if http != https || !generated_no_proxy(no_proxy) {
        return block_invalid("The managed block contains values Guard could not have generated.");
    }
    let prefix = "http://";
    if !http.starts_with(prefix) {
        return block_invalid(edited);
    }
    let mut offset = prefix.len();
    let bytes = http.as_bytes();
    let host: String;
    if offset < bytes.len() && bytes[offset] == b'[' {
        let close = http[offset..].find(']').map(|p| p + offset);
        let close = match close {
            Some(close) if close > offset + 1 => close,
            _ => return block_invalid(edited),
        };
        host = http[offset + 1..close].to_owned();
        offset = close + 1;
    } else {
        let start = offset;
        while offset < bytes.len() {
            let ch = bytes[offset];
            if ch == b':'
                || ch == b'/'
                || ch == b'?'
                || ch == b'#'
                || (ch as char).is_ascii_whitespace()
            {
                break;
            }
            offset += 1;
        }
        host = http[start..offset].to_owned();
    }
    if host.is_empty() || offset >= bytes.len() || bytes[offset] != b':' {
        return block_invalid(edited);
    }
    offset += 1;
    let digits_start = offset;
    while offset < bytes.len() && bytes[offset].is_ascii_digit() {
        offset += 1;
    }
    let digit_count = offset - digits_start;
    if !(1..=5).contains(&digit_count) || offset != bytes.len() {
        return block_invalid(edited);
    }
    let port = match parse_port_strict(&http[digits_start..offset]) {
        Ok(port) => port,
        Err(_) => {
            return block_invalid(
                "The managed block proxy endpoint is not a valid loopback proxy.",
            );
        }
    };
    let generated = Config {
        host,
        port: i64::from(port),
        ..Config::default()
    };
    generated.validate().map_err(|_| {
        Error::new(
            "BACKEND_PROXY_BLOCK_INVALID",
            "The managed block proxy endpoint is not a valid loopback proxy.",
        )
    })?;
    if !is_loopback_host(&generated.host) || generated.proxy_url() != http.as_str() {
        return block_invalid(edited);
    }
    Ok(())
}

/// Pure recognition pass over the (already UTF-8-validated and lexically
/// gated) file bytes. Port of `readEnv` minus the file open.
pub fn parse_env(bytes: &str) -> Result<EnvFile> {
    let mut env = EnvFile {
        exists: true,
        bytes: bytes.to_owned(),
        ..EnvFile::default()
    };
    let unsafe_line = unsafe_env_line(bytes);
    if let Some(line) = unsafe_line {
        return Err(Error::new(
            "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED",
            format!(
                "The proxy file uses multi-line or unterminated quoting at line {line}; \
                 Guard cannot safely edit it automatically. Fix the file manually and retry."
            ),
        ));
    }
    let mut inside = false;
    let mut offset = 0usize;
    while offset < bytes.len() {
        let newline = bytes[offset..].find('\n').map(|p| p + offset);
        let stop = newline.unwrap_or(bytes.len());
        let mut line = &bytes[offset..stop];
        if line.ends_with('\r') {
            line = &line[..line.len() - 1];
        }
        let bom = offset == 0 && line.starts_with('\u{feff}');
        if bom {
            line = &line['\u{feff}'.len_utf8()..];
        }
        let next = newline.map(|p| p + 1).unwrap_or(bytes.len());
        if line == BLOCK_BEGIN {
            if env.begin.is_some() || env.end.is_some() {
                return block_invalid("Duplicate or misordered Guard block.");
            }
            env.begin = Some(offset + if bom { '\u{feff}'.len_utf8() } else { 0 });
            inside = true;
        } else if line == BLOCK_END {
            if !inside || env.end.is_some() {
                return block_invalid("Duplicate or misordered Guard block end.");
            }
            env.end = Some(next);
            inside = false;
        } else if starts_with(line, "# BEGIN CODEX PROXY GUARD:")
            || starts_with(line, "# END CODEX PROXY GUARD:")
        {
            return block_invalid("Unknown Guard block version.");
        } else if inside {
            let (key, value) = match line.split_once('=') {
                Some((key, value)) => (key, value),
                None => {
                    return block_invalid(
                        "The Guard block has been edited or contains duplicate keys.",
                    );
                }
            };
            if !matches!(key, "HTTP_PROXY" | "HTTPS_PROXY" | "NO_PROXY")
                || env.values.contains_key(key)
            {
                return block_invalid(
                    "The Guard block has been edited or contains duplicate keys.",
                );
            }
            env.values.insert(key.to_owned(), value.to_owned());
        } else {
            let text = ascii_trimmed(line);
            if !text.is_empty()
                && !text.starts_with('#')
                && let Some(equal) = text.find('=')
            {
                let mut key = ascii_trimmed(&text[..equal]);
                let folded_prefix = "export";
                if key.starts_with(folded_prefix)
                    && key.len() > folded_prefix.len()
                    && key.as_bytes()[folded_prefix.len()].is_ascii_whitespace()
                {
                    key = ascii_trimmed(&key[folded_prefix.len()..]);
                }
                let key = ascii_folded(key);
                if matches!(
                    key.as_str(),
                    "http_proxy" | "https_proxy" | "no_proxy" | "all_proxy"
                ) {
                    env.conflict = true;
                }
            }
        }
        offset = next;
    }
    if inside
        || env.begin.is_some() != env.end.is_some()
        || (env.begin.is_some() && env.values.len() != 3)
    {
        return block_invalid("The Guard block is truncated or incomplete.");
    }
    if env.begin.is_some() {
        validate_managed_values(&env)?;
    }
    Ok(env)
}

pub fn current_values(env: &EnvFile, config: &Config) -> bool {
    env.begin.is_some()
        && env.values.get("HTTP_PROXY").map(String::as_str) == Some(config.proxy_url().as_str())
        && env.values.get("HTTPS_PROXY").map(String::as_str) == Some(config.proxy_url().as_str())
        && env.values.get("NO_PROXY").map(String::as_str) == Some(config.no_proxy_value().as_str())
}

/// The replacement block Guard writes for the current configuration.
pub fn managed_block(config: &Config) -> String {
    let url = config.proxy_url();
    format!(
        "{BLOCK_BEGIN}\nHTTP_PROXY={url}\nHTTPS_PROXY={url}\nNO_PROXY={}\n{BLOCK_END}\n",
        config.no_proxy_value()
    )
}

/// Computes the updated file bytes for `prepare` (insert or replace Guard's
/// block) and for `revoke` (remove it). Returns `None` when there is nothing
/// to change.
pub fn prepare_updated_bytes(env: &EnvFile, config: &Config) -> Option<String> {
    if current_values(env, config) {
        return None;
    }
    let block = managed_block(config);
    let bytes = env.bytes.as_str();
    Some(match env.begin {
        Some(begin) => {
            let end = env.end.expect("parser guarantees an end with a begin");
            format!("{}{}{}", &bytes[..begin], block, &bytes[end..])
        }
        None => {
            let separator = if !bytes.is_empty() && !bytes.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            format!("{bytes}{separator}{block}")
        }
    })
}

pub fn revoke_updated_bytes(env: &EnvFile) -> Option<String> {
    let begin = env.begin?;
    let end = env.end?;
    let bytes = env.bytes.as_str();
    Some(format!("{}{}", &bytes[..begin], &bytes[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_gate_accepts_simple_lines() {
        assert_eq!(unsafe_env_line("A=1\nB=2\n"), None);
        assert_eq!(
            unsafe_env_line("# comment ' unpaired\nX=\"quoted value\"\n"),
            None
        );
        assert_eq!(unsafe_env_line("X='a#b'\nY=z # trailing\n"), None);
        assert_eq!(unsafe_env_line("X=\"say \\\"hi\\\"\"\n"), None);
        assert_eq!(unsafe_env_line("\u{feff}X=1\n"), None);
        assert_eq!(unsafe_env_line(""), None);
        assert_eq!(unsafe_env_line("export KEY=value\n"), None);
    }

    #[test]
    fn lexical_gate_refuses_multiline_constructs() {
        assert_eq!(unsafe_env_line("X=\"a\nb\"\n"), Some(1));
        assert_eq!(unsafe_env_line("A=1\nX='open\n"), Some(2));
        assert_eq!(
            unsafe_env_line("X=a\\\nb\n"),
            Some(1),
            "trailing backslash continues"
        );
        assert_eq!(unsafe_env_line("X=a\nY=b\\"), Some(2), "eof backslash");
        assert_eq!(unsafe_env_line("\"quoted key\"=x\n"), Some(1));
        // Marker inside a quoted value is content: the gate refuses the file.
        assert_eq!(
            unsafe_env_line(&format!("X=\"a\n{BLOCK_BEGIN}\nb\"")),
            Some(1)
        );
    }

    #[test]
    fn marker_in_quoted_single_line_value_is_content_not_block() {
        let text = format!("X=\"{BLOCK_BEGIN}\"\n");
        let env = parse_env(&text).unwrap();
        assert!(!env.has_block());
        assert!(!env.conflict);
    }

    #[test]
    fn recognizes_block_values_and_conflicts() {
        let text = format!(
            "# prior\n{BLOCK_BEGIN}\nHTTP_PROXY=http://127.0.0.1:10808\nHTTPS_PROXY=http://127.0.0.1:10808\nNO_PROXY=localhost,127.0.0.1\n{BLOCK_END}\nTRAILING=1\n"
        );
        let env = parse_env(&text).unwrap();
        assert!(env.has_block());
        assert_eq!(env.begin, Some("# prior\n".len()));
        assert!(!env.conflict);
        assert_eq!(
            env.values.get("HTTP_PROXY").unwrap(),
            "http://127.0.0.1:10808"
        );

        let conflicting = "http_proxy=1\n".to_owned() + &text;
        assert!(parse_env(&conflicting).unwrap().conflict);
        let conflicting = format!("export ALL_PROXY=socks5://x\n{text}");
        assert!(parse_env(&conflicting).unwrap().conflict);
        let conflicting = format!("HTTP_PROXY = 1\n{text}");
        assert!(parse_env(&conflicting).unwrap().conflict);
        assert!(
            !parse_env(&format!("X_HTTP_PROXY=1\n{text}"))
                .unwrap()
                .conflict
        );
    }

    #[test]
    fn rejects_malformed_blocks() {
        assert!(parse_env(&format!("{BLOCK_BEGIN}\nHTTP_PROXY=x\n{BLOCK_END}\n")).is_err());
        assert!(parse_env(&format!("{BLOCK_BEGIN}\n")).is_err());
        assert!(parse_env(&format!("{BLOCK_END}\n")).is_err());
        assert!(
            parse_env(&format!("{BLOCK_BEGIN}\nX=1\nHTTP_PROXY=http://127.0.0.1:1\nHTTPS_PROXY=http://127.0.0.1:1\nNO_PROXY=localhost\n{BLOCK_END}\n")).is_err()
        );
        assert!(
            parse_env(&format!("{BLOCK_BEGIN}\nHTTP_PROXY=http://127.0.0.1:1\nHTTP_PROXY=http://127.0.0.1:1\nHTTPS_PROXY=http://127.0.0.1:1\nNO_PROXY=localhost\n{BLOCK_END}\n")).is_err()
        );
        assert!(parse_env("# BEGIN CODEX PROXY GUARD: proxy-v2\n").is_err());
        assert!(parse_env(&format!("{BLOCK_BEGIN}\nHTTP_PROXY=http://example.com:1\nHTTPS_PROXY=http://example.com:1\nNO_PROXY=x\n{BLOCK_END}\n")).is_err());
        assert!(parse_env(&format!("{BLOCK_BEGIN}\nHTTP_PROXY=http://127.0.0.1:99999\nHTTPS_PROXY=http://127.0.0.1:99999\nNO_PROXY=x\n{BLOCK_END}\n")).is_err());
        assert!(parse_env(&format!("{BLOCK_BEGIN}\nHTTP_PROXY=http://127.0.0.1:1\nHTTPS_PROXY=http://127.0.0.1:1\nNO_PROXY={}\n{BLOCK_END}\n", "x".repeat(256))).is_err());
    }

    #[test]
    fn generated_no_proxy_partitions() {
        assert!(generated_no_proxy("localhost,127.0.0.1"));
        assert!(generated_no_proxy("a".repeat(255).as_str()));
        assert!(!generated_no_proxy(&"a".repeat(256)));
        assert!(!generated_no_proxy(" leading"));
        assert!(
            generated_no_proxy("a,b,,c"),
            "comma-bearing entries can generate it"
        );
        // Within the 4096-byte ceiling, 33 unmergeable (>=128 byte) entries
        // always exceed the size limit first; 32 short entries stay valid.
        let thirty_three = (0..33)
            .map(|_| "b".repeat(128))
            .collect::<Vec<_>>()
            .join(",");
        assert!(!generated_no_proxy(&thirty_three));
        let thirty_two = (0..32)
            .map(|_| "c".repeat(120))
            .collect::<Vec<_>>()
            .join(",");
        assert!(generated_no_proxy(&thirty_two));
    }

    #[test]
    fn splice_semantics() {
        let config = Config::default();
        // Fresh file without trailing newline gains a separator newline.
        let env = parse_env("X=1").unwrap();
        let updated = prepare_updated_bytes(&env, &config).unwrap();
        assert!(updated.starts_with("X=1\n"));
        assert!(updated.ends_with(&format!("{BLOCK_END}\n")));
        // Empty file: no separator.
        let env = parse_env("").unwrap();
        let updated = prepare_updated_bytes(&env, &config).unwrap();
        assert!(updated.starts_with(BLOCK_BEGIN));
        // Replace keeps outside bytes verbatim; the regenerated block itself
        // always uses LF, exactly like the C++ splice.
        let stale = format!(
            "KEEP=1\r\n{BLOCK_BEGIN}\nHTTP_PROXY=http://127.0.0.1:1\nHTTPS_PROXY=http://127.0.0.1:1\nNO_PROXY=x\n{BLOCK_END}\r\nTAIL\r\n"
        );
        let env = parse_env(&stale).unwrap();
        let updated = prepare_updated_bytes(&env, &config).unwrap();
        assert!(updated.starts_with("KEEP=1\r\n"));
        assert!(
            updated.ends_with("TAIL\r\n"),
            "trailing bytes preserved: {updated:?}"
        );
        assert!(updated.contains(&format!("{BLOCK_END}\nTAIL")));
        assert!(updated.contains("HTTP_PROXY=http://127.0.0.1:10808"));
        // Current values mean no rewrite.
        let current = managed_block(&config);
        let env = parse_env(&current).unwrap();
        assert!(prepare_updated_bytes(&env, &config).is_none());
        // Revoke removes exactly the block.
        let revoked = revoke_updated_bytes(&env).unwrap();
        assert_eq!(revoked, "");
        let file = format!("A=1\n{current}B=2\n");
        let env = parse_env(&file).unwrap();
        assert_eq!(revoke_updated_bytes(&env).unwrap(), "A=1\nB=2\n");
    }
}
