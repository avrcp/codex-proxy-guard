//! Strict UTF-8 handling and the deterministic redaction scanner
//! (`publicText`): USERPROFILE masking, ANSI CSI/OSC removal, URI credential
//! redaction, secret-bearing line redaction, control/format code-point
//! filtering, whitespace collapsing and code-point-safe truncation.

use crate::error::{Error, Result};

/// Strict UTF-8 validation (surrogates, overlongs and 5/6-byte forms
/// refused) — `std` decoding has exactly these semantics.
pub fn strict_utf8(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
}

pub fn utf8_to_wide(utf8: &str) -> Vec<u16> {
    utf8.encode_utf16().collect()
}

pub fn wide_to_utf8(wide: &[u16]) -> Result<String> {
    String::from_utf16(wide)
        .map_err(|_| Error::new("TEXT_ENCODING_INVALID", "Text is not valid UTF-16."))
}

pub fn wide_to_utf8_lossless(wide: &[u16]) -> String {
    String::from_utf16_lossy(wide)
}

/// True when `text` opens with the ASCII `prefix` case-insensitively and holds
/// more bytes after it. A match proves the prefix bytes are ASCII, so callers
/// may slice at `prefix.len()` without crossing a char boundary — text that
/// starts with a multi-byte character simply does not carry the prefix.
pub fn opens_with_ascii_prefix(text: &str, prefix: &str) -> bool {
    let (body, head) = (text.as_bytes(), prefix.as_bytes());
    body.len() > head.len() && body[..head.len()].eq_ignore_ascii_case(head)
}

fn ascii_lower(byte: u8) -> u8 {
    if byte.is_ascii_uppercase() {
        byte - b'A' + b'a'
    } else {
        byte
    }
}

fn iequals(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(l, r)| ascii_lower(*l) == ascii_lower(*r))
}

/// Case-insensitive replace over UTF-8 bytes; only ASCII letters fold so a
/// match can never start inside a multi-byte sequence.
fn replace_all_insensitive(text: &str, needle: &[u8], replacement: &str) -> String {
    if needle.is_empty() || needle.len() > text.len() {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut copied = 0usize;
    let limit = bytes.len() - needle.len();
    while copied <= limit {
        if iequals(&bytes[copied..copied + needle.len()], needle) {
            result.extend_from_slice(replacement.as_bytes());
            copied += needle.len();
        } else {
            result.push(bytes[copied]);
            copied += 1;
        }
    }
    result.extend_from_slice(&bytes[copied..]);
    String::from_utf8(result).unwrap_or_default()
}

/// Remove CSI (`ESC [ … <0x40-0x7E>`) and OSC (`ESC ] … BEL|ESC \`) sequences.
fn remove_ansi_osc_sequences(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            result.push(bytes[index]);
            index += 1;
            continue;
        }
        let Some(introducer) = bytes.get(index + 1).copied() else {
            result.push(bytes[index]);
            index += 1;
            continue;
        };
        if introducer == b'[' {
            let mut scan = index + 2;
            while scan < bytes.len() && (0x30..=0x3f).contains(&bytes[scan]) {
                scan += 1;
            }
            while scan < bytes.len() && (0x20..=0x2f).contains(&bytes[scan]) {
                scan += 1;
            }
            if scan < bytes.len() && (0x40..=0x7e).contains(&bytes[scan]) {
                index = scan + 1;
                continue;
            }
        } else if introducer == b']' {
            let mut scan = index + 2;
            let mut terminated = false;
            while scan < bytes.len() {
                if bytes[scan] == 0x07 {
                    index = scan + 1;
                    terminated = true;
                    break;
                }
                if bytes[scan] == 0x1b && scan + 1 < bytes.len() && bytes[scan + 1] == b'\\' {
                    index = scan + 2;
                    terminated = true;
                    break;
                }
                scan += 1;
            }
            if terminated {
                continue;
            }
            // Unterminated OSC: keep the ESC verbatim, like the C++ scanner.
        }
        result.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(result).unwrap_or_default()
}

fn ascii_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

/// `://user:pass@host` → `://[REDACTED]@host` for non-empty credential spans
/// free of `/`, whitespace and end-of-text.
fn redact_uri_credentials(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes.len() - index >= 3 && &bytes[index..index + 3] == b"://" {
            let mut scan = index + 3;
            while scan < bytes.len() {
                let value = bytes[scan];
                if ascii_whitespace(value) || value == b'/' || value == b'@' {
                    break;
                }
                scan += 1;
            }
            if scan < bytes.len() && bytes[scan] == b'@' && scan > index + 3 {
                result.extend_from_slice(b"://[REDACTED]@");
                index = scan + 1;
            } else {
                result.extend_from_slice(b"://");
                index += 3;
            }
        } else {
            result.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(result).unwrap_or_default()
}

fn matches_word(text: &[u8], position: usize, word: &[u8]) -> bool {
    position + word.len() <= text.len()
        && text[position..position + word.len()]
            .iter()
            .zip(word)
            .all(|(l, r)| ascii_lower(*l) == ascii_lower(*r))
}

/// Secret-bearing prefix vocabulary from the C++ scanner: header forms
/// include the colon; assignment forms expect optional whitespace and `=`
/// after the word ("api" additionally takes `_`/`-` + "key").
fn secret_prefix_length(line: &[u8], position: usize) -> usize {
    const PREFIXES: &[(&[u8], bool)] = &[
        (b"proxy-authorization:", false),
        (b"authorization:", false),
        (b"set-cookie:", false),
        (b"cookie:", false),
        (b"token", true),
        (b"api", true),
        (b"authorization", true),
        (b"cookie", true),
        (b"password", true),
        (b"secret", true),
    ];
    for (word, assignment) in PREFIXES {
        if !matches_word(line, position, word) {
            continue;
        }
        if !assignment {
            return word.len();
        }
        let mut scan = position + word.len();
        if *word == b"api" {
            if scan < line.len() && (line[scan] == b'_' || line[scan] == b'-') {
                scan += 1;
            }
            if !matches_word(line, scan, b"key") {
                continue;
            }
            scan += 3;
        }
        while scan < line.len() && (line[scan] == b' ' || line[scan] == b'\t') {
            scan += 1;
        }
        if scan < line.len() && line[scan] == b'=' {
            return scan - position + 1;
        }
    }
    0
}

fn redact_secret_lines(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for line in text.split('\n') {
        let bytes = line.as_bytes();
        let mut matched: Option<usize> = None;
        for index in 0..bytes.len() {
            let length = secret_prefix_length(bytes, index);
            if length > 0 {
                matched = Some(index + length);
                break;
            }
        }
        match matched {
            None => result.push_str(line),
            Some(cut) => {
                result.push_str(&line[..cut]);
                result.push_str("[REDACTED]");
            }
        }
        result.push('\n');
    }
    result.pop();
    result
}

fn is_whitespace_code_point(point: char) -> bool {
    matches!(
        point,
        '\u{9}'
            | '\u{a}'
            | '\u{b}'
            | '\u{c}'
            | '\u{d}'
            | '\u{20}'
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    ) || ('\u{2000}'..='\u{200a}').contains(&point)
}

fn is_format_code_point(point: char) -> bool {
    matches!(
        point,
        '\u{ad}'
            | '\u{600}'..='\u{605}'
            | '\u{61c}'
            | '\u{6dd}'
            | '\u{70f}'
            | '\u{8e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'..='\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// Redact and flatten untrusted text for display or protocol emission,
/// truncated to `limit` code points.
pub fn public_text(input: &str, limit: usize) -> String {
    let mut text = input.to_owned();
    if let Ok(profile) = std::env::var("USERPROFILE")
        && !profile.is_empty()
    {
        let forward = profile.replace('\\', "/");
        text = replace_all_insensitive(&text, profile.as_bytes(), "%USERPROFILE%");
        text = replace_all_insensitive(&text, forward.as_bytes(), "%USERPROFILE%");
    }
    let text = remove_ansi_osc_sequences(&text);
    let text = redact_uri_credentials(&text);
    let text = redact_secret_lines(&text);
    // Control/format/surrogate filtering: chars() drops nothing for valid
    // UTF-8 (no surrogates can appear), whitespace maps to ' '.
    let mut filtered = String::with_capacity(text.len());
    for point in text.chars() {
        if is_whitespace_code_point(point) {
            filtered.push(' ');
        } else if point.is_control() && !matches!(point, '\u{9}' | '\u{a}')
            || is_format_code_point(point)
        {
            continue;
        } else {
            filtered.push(point);
        }
    }
    // The C++ drops every code point below 0x20 / in 0x7f-0x9f. Whitespace
    // code points were already mapped to ' ', so remaining controls go.
    let filtered: String = filtered
        .chars()
        .filter(|point| {
            let value = *point as u32;
            !(value < 0x20 || (0x7f..=0x9f).contains(&value))
        })
        .collect();
    let collapsed = collapse_whitespace(&filtered);
    collapsed.chars().take(limit).collect()
}

fn collapse_whitespace(text: &str) -> String {
    let trimmed = text.trim_matches(' ');
    let mut result = String::with_capacity(trimmed.len());
    let mut pending = false;
    for character in trimmed.chars() {
        if character == ' ' {
            pending = true;
            continue;
        }
        if pending && !result.is_empty() {
            result.push(' ');
        }
        pending = false;
        result.push(character);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(input: &str) -> String {
        // The reference machine always has a USERPROFILE; none of the other
        // fixtures contain it, so tests must not race by mutating the
        // process environment.
        public_text(input, 160)
    }

    #[test]
    fn masks_userprofile_case_insensitively() {
        let profile = std::env::var("USERPROFILE").expect("USERPROFILE is set on Windows");
        let forward = profile.replace('\\', "/");
        assert!(
            public_text(&format!("{}\\.codex", profile), 160).starts_with("%USERPROFILE%"),
            "backslash form masked"
        );
        assert!(
            public_text(&format!("{forward}/x"), 160).starts_with("%USERPROFILE%"),
            "forward-slash form masked"
        );
    }

    #[test]
    fn strips_ansi_and_osc() {
        assert_eq!(
            pt("a\x1b[31mred\x1b[0mb"),
            "a red b".replace(" red ", "red")
        );
        assert_eq!(pt("x\x1b]0;title\x07y"), "xy");
        assert_eq!(pt("x\x1b]8;;url\x1b\\y"), "xy");
    }

    #[test]
    fn redacts_uri_credentials() {
        assert_eq!(pt("http://user:pass@host/x"), "http://[REDACTED]@host/x");
        assert_eq!(pt("http://host/x"), "http://host/x");
        assert_eq!(pt("http://@host/"), "http://@host/");
    }

    #[test]
    fn redacts_secret_lines_earliest_match() {
        assert_eq!(pt("authorization: Bearer abc"), "authorization:[REDACTED]");
        assert_eq!(pt("cookie: a=b"), "cookie:[REDACTED]");
        assert_eq!(pt("token=abc"), "token=[REDACTED]");
        assert_eq!(pt("api_key=zz"), "api_key=[REDACTED]");
        assert_eq!(pt("api-key = zz"), "api-key =[REDACTED]");
        assert_eq!(
            pt("password: hunter2"),
            "password: hunter2",
            "colon form of assignment word is not a header"
        );
        assert_eq!(pt("my token = x"), "my token =[REDACTED]");
    }

    #[test]
    fn collapses_whitespace_and_drops_controls() {
        assert_eq!(pt("  a\u{2003}b  c  "), "a b c");
        assert_eq!(pt("a\u{200b}b"), "ab");
        assert_eq!(pt("a\u{1}b"), "ab");
    }

    #[test]
    fn truncates_on_code_point_boundaries() {
        let text = "日本語".repeat(10);
        assert_eq!(public_text(&text, 5).chars().count(), 5);
    }

    #[test]
    fn utf8_validation_matches() {
        assert!(strict_utf8(b"abc"));
        assert!(strict_utf8("日本".as_bytes()));
        assert!(!strict_utf8(&[0xff, 0xfe]));
        assert!(!strict_utf8(&[0xed, 0xa0, 0x80]));
    }

    #[test]
    fn an_ascii_prefix_match_keeps_the_slice_on_a_char_boundary() {
        assert!(opens_with_ascii_prefix("//?/D:\\x", "//?/"));
        assert!(opens_with_ascii_prefix("//?/UNC/s/b", "//?/unc"));
        assert!(!opens_with_ascii_prefix("//?/", "//?/"));
        assert!(!opens_with_ascii_prefix("数据\\codex", "//?/"));
        let text = "//?/D:\\数据\\codex";
        assert!(opens_with_ascii_prefix(text, "//?/"));
        assert_eq!(&text[4..], "D:\\数据\\codex");
    }
}
