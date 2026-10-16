//! Strict JSON value, parser and serializer with parity for the C++
//! `parseStrictJsonObject` / nlohmann `dump()` contracts:
//!
//! * objects keep lexicographic key order (nlohmann's `std::map` default) and
//!   duplicate keys — including escape-decoded spellings such as `"\u0061"`
//!   versus `"a"` — are rejected at every nesting level;
//! * nesting depth is limited to 64;
//! * input must be strict UTF-8, optionally prefixed with one UTF-8 BOM that
//!   is skipped before parsing, and trailing data is rejected;
//! * integers stay integers (i64 / u64), everything with a fraction or
//!   exponent becomes f64, mirroring `is_number_integer` semantics;
//! * `dump()` emits the compact nlohmann form (`{"a":1,"b":[true]}`).

use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::fmt::Write as _;

const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// Signed integers and non-negative integers that fit i64.
    Int(i64),
    /// Integers above i64::MAX (nlohmann's number_unsigned fast path).
    UInt(u64),
    Double(f64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    pub fn object() -> Json {
        Json::Object(BTreeMap::new())
    }

    pub fn object_from<'a>(entries: impl IntoIterator<Item = (&'a str, Json)>) -> Json {
        let mut map = BTreeMap::new();
        for (key, value) in entries {
            map.insert(key.to_owned(), value);
        }
        Json::Object(map)
    }

    pub fn array() -> Json {
        Json::Array(Vec::new())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Json::Object(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Json::Array(_))
    }

    pub fn as_array(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Json::String(_))
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, Json::Bool(_))
    }

    /// nlohmann `is_number_integer()` — true for signed and unsigned integers.
    pub fn is_number_integer(&self) -> bool {
        matches!(self, Json::Int(_) | Json::UInt(_))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(value) => Some(value.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(value) => Some(*value),
            Json::UInt(value) => i64::try_from(*value).ok(),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Int(value) => u64::try_from(*value).ok(),
            Json::UInt(value) => Some(*value),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(map) => map.get(key),
            _ => None,
        }
    }

    /// Mirrors the C++ `at()` helper: a missing key reads as a null value.
    pub fn at(&self, key: &str) -> &Json {
        static NULL: Json = Json::Null;
        self.get(key).unwrap_or(&NULL)
    }

    pub fn string_at(&self, key: &str) -> String {
        self.at(key).as_str().unwrap_or_default().to_owned()
    }

    /// Object setter used while assembling receipts (no-op on non-objects,
    /// matching nlohmann's keyed assignment).
    pub fn insert(&mut self, key: &str, value: Json) {
        if let Json::Object(map) = self {
            map.insert(key.to_owned(), value);
        }
    }

    pub fn push(&mut self, value: Json) {
        if let Json::Array(items) = self {
            items.push(value);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        match self {
            Json::Array(items) => items.len(),
            Json::Object(map) => map.len(),
            Json::String(text) => text.len(),
            _ => 0,
        }
    }

    pub fn dump(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Int(value) => {
                let _ = write!(out, "{value}");
            }
            Json::UInt(value) => {
                let _ = write!(out, "{value}");
            }
            Json::Double(value) => out.push_str(&format_f64(*value)),
            Json::String(value) => write_json_string(out, value),
            Json::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(map) => {
                out.push('{');
                for (index, (key, value)) in map.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_json_string(out, key);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Json::String(value.to_owned())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Json::String(value)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Self {
        Json::Bool(value)
    }
}

impl From<i64> for Json {
    fn from(value: i64) -> Self {
        Json::Int(value)
    }
}

impl From<u64> for Json {
    fn from(value: u64) -> Self {
        if let Ok(signed) = i64::try_from(value) {
            Json::Int(signed)
        } else {
            Json::UInt(value)
        }
    }
}

impl From<u32> for Json {
    fn from(value: u32) -> Self {
        Json::Int(i64::from(value))
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(inner) => inner.into(),
            None => Json::Null,
        }
    }
}

fn write_json_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn format_f64(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        // nlohmann prints integral doubles without a fraction.
        format!("{value:.1}")
    } else {
        let mut text = format!("{value}");
        if text.contains('e') && !text.contains("e-") && !text.contains("e+") {
            text.insert(text.find('e').unwrap() + 1, '+');
        }
        text
    }
}

/// Strict-object entry point with the C++ size/UTF-8/BOM/depth/duplicate
/// guarantees. Callers pass the public error code used for every rejection.
pub fn parse_strict_json_object(bytes: &[u8], limit: usize, error_code: &str) -> Result<Json> {
    let reject = |message: &str| Error::new(error_code, message);
    if bytes.len() > limit {
        return Err(reject("JSON input is oversized or not valid UTF-8."));
    }
    let payload = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return Err(reject("JSON input is oversized or not valid UTF-8.")),
    };
    let payload = payload.strip_prefix('\u{feff}').unwrap_or(payload);
    let mut parser = Parser {
        bytes: payload.as_bytes(),
        position: 0,
        depth: 0,
        over_depth: false,
    };
    let value = parser
        .parse_value()
        .map_err(|_| reject("Expected a valid JSON object."))?;
    parser.skip_whitespace();
    if parser.position != parser.bytes.len() {
        return Err(reject("Expected a valid JSON object."));
    }
    if parser.over_depth {
        return Err(reject("JSON nesting exceeds its limit."));
    }
    if !value.is_object() {
        return Err(reject("Expected a valid JSON object."));
    }
    Ok(value)
}

pub fn json_dump(value: &Json) -> String {
    value.dump()
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
    depth: usize,
    over_depth: bool,
}

type ParseResult<T> = std::result::Result<T, ()>;

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek();
        if byte.is_some() {
            self.position += 1;
        }
        byte
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> ParseResult<()> {
        if self.peek() == Some(byte) {
            self.position += 1;
            Ok(())
        } else {
            Err(())
        }
    }

    fn parse_value(&mut self) -> ParseResult<Json> {
        self.skip_whitespace();
        match self.peek().ok_or(())? {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => Ok(Json::String(self.parse_string()?)),
            b't' => self.parse_literal("true", Json::Bool(true)),
            b'f' => self.parse_literal("false", Json::Bool(false)),
            b'n' => self.parse_literal("null", Json::Null),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => Err(()),
        }
    }

    fn parse_literal(&mut self, text: &str, value: Json) -> ParseResult<Json> {
        if self.bytes[self.position..].starts_with(text.as_bytes()) {
            self.position += text.len();
            Ok(value)
        } else {
            Err(())
        }
    }

    fn enter(&mut self) {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.over_depth = true;
        }
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn parse_object(&mut self) -> ParseResult<Json> {
        self.enter();
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            self.leave();
            return Ok(Json::Object(map));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.parse_value()?;
            // BTreeMap::insert silently replaces; duplicate detection must be
            // explicit so escape-decoded spellings collide exactly like the
            // nlohmann key walker.
            if map.insert(key, value).is_some() {
                return Err(());
            }
            self.skip_whitespace();
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => break,
                _ => return Err(()),
            }
        }
        self.leave();
        Ok(Json::Object(map))
    }

    fn parse_array(&mut self) -> ParseResult<Json> {
        self.enter();
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            self.leave();
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_whitespace();
            match self.bump() {
                Some(b',') => continue,
                Some(b']') => break,
                _ => return Err(()),
            }
        }
        self.leave();
        Ok(Json::Array(items))
    }

    fn parse_string(&mut self) -> ParseResult<String> {
        self.expect(b'"')?;
        let mut text = String::new();
        loop {
            match self.bump().ok_or(())? {
                b'"' => return Ok(text),
                b'\\' => match self.bump().ok_or(())? {
                    b'"' => text.push('"'),
                    b'\\' => text.push('\\'),
                    b'/' => text.push('/'),
                    b'b' => text.push('\u{8}'),
                    b'f' => text.push('\u{c}'),
                    b'n' => text.push('\n'),
                    b'r' => text.push('\r'),
                    b't' => text.push('\t'),
                    b'u' => {
                        let first = self.parse_hex4()?;
                        let point = if (0xd800..0xdc00).contains(&first) {
                            // High surrogate must be followed by \uDC00-\uDFFF.
                            if self.bump() != Some(b'\\') || self.bump() != Some(b'u') {
                                return Err(());
                            }
                            let second = self.parse_hex4()?;
                            if !(0xdc00..0xe000).contains(&second) {
                                return Err(());
                            }
                            0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                        } else if (0xdc00..0xe000).contains(&first) {
                            return Err(());
                        } else {
                            first
                        };
                        match char::from_u32(point) {
                            Some(character) => text.push(character),
                            None => return Err(()),
                        }
                    }
                    _ => return Err(()),
                },
                byte if byte < 0x20 => return Err(()),
                byte if byte < 0x80 => text.push(byte as char),
                _ => {
                    // Multi-byte UTF-8: copy the whole sequence, validating it.
                    let start = self.position - 1;
                    let width = utf8_width(self.bytes[start]).ok_or(())?;
                    if start + width > self.bytes.len() {
                        return Err(());
                    }
                    let chunk =
                        std::str::from_utf8(&self.bytes[start..start + width]).map_err(|_| ())?;
                    text.push_str(chunk);
                    self.position = start + width;
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> ParseResult<u32> {
        let mut value = 0u32;
        for _ in 0..4 {
            let digit = self.bump().ok_or(())?;
            let nibble = match digit {
                b'0'..=b'9' => u32::from(digit - b'0'),
                b'a'..=b'f' => u32::from(digit - b'a' + 10),
                b'A'..=b'F' => u32::from(digit - b'A' + 10),
                _ => return Err(()),
            };
            value = (value << 4) | nibble;
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> ParseResult<Json> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            _ => return Err(()),
        }
        let mut is_integer = true;
        if self.peek() == Some(b'.') {
            is_integer = false;
            self.position += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_integer = false;
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.position]).map_err(|_| ())?;
        if !is_integer {
            return Ok(Json::Double(text.parse::<f64>().map_err(|_| ())?));
        }
        if let Some(unsigned) = text.strip_prefix('-') {
            let magnitude: i128 = unsigned.parse().map_err(|_| ())?;
            let value = -magnitude;
            if value < i64::MIN as i128 {
                return Ok(Json::Double(text.parse::<f64>().map_err(|_| ())?));
            }
            Ok(Json::Int(value as i64))
        } else {
            let magnitude: u128 = text.parse().map_err(|_| ())?;
            if magnitude <= u64::MAX as u128 {
                Ok(Json::from(magnitude as u64))
            } else {
                Ok(Json::Double(text.parse::<f64>().map_err(|_| ())?))
            }
        }
    }
}

fn utf8_width(byte: u8) -> Option<usize> {
    match byte {
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "TEST_JSON";

    fn parse(text: &str) -> Result<Json> {
        parse_strict_json_object(text.as_bytes(), 64 * 1024, CODE)
    }

    #[test]
    fn parses_nested_object_with_sorted_keys() {
        let value = parse(r#"{"b":1,"a":{"z":[1,2.5,true,null,"x"]}}"#).unwrap();
        let object = value.get("a").unwrap();
        assert_eq!(value.string_at("b"), "");
        assert_eq!(object.get("z").unwrap().dump(), "[1,2.5,true,null,\"x\"]");
        let dumped = value.dump();
        assert!(dumped.starts_with("{\"a\":"), "keys sorted: {dumped}");
    }

    #[test]
    fn rejects_duplicate_keys_at_every_level() {
        assert!(parse(r#"{"a":1,"a":2}"#).is_err());
        assert!(parse(r#"{"outer":{"x":1,"x":2}}"#).is_err());
        assert!(
            parse(r#"{"list":[{"k":1},{"k":1}]}"#).is_ok(),
            "array items are separate"
        );
    }

    #[test]
    fn rejects_escape_decoded_duplicate_spellings() {
        assert!(parse(r#"{"a":1,"\u0061":2}"#).is_err());
        assert!(parse(r#"{"é":1,"\u00e9":2}"#).is_err());
    }

    #[test]
    fn rejects_trailing_data_and_non_objects() {
        assert!(parse("{} {}").is_err());
        assert!(parse("{}x").is_err());
        assert!(parse("[1,2]").is_err());
        assert!(parse("null").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn skips_single_leading_bom() {
        let mut bytes = "\u{feff}".as_bytes().to_vec();
        bytes.extend_from_slice(b"{\"a\":1}");
        assert!(parse_strict_json_object(&bytes, 1024, CODE).is_ok());
    }

    #[test]
    fn enforces_depth_limit() {
        let nested = |depth: usize| {
            let mut text = String::new();
            for _ in 0..depth {
                text.push_str("{\"a\":");
            }
            text.push('1');
            for _ in 0..depth {
                text.push('}');
            }
            text
        };
        assert!(parse(&nested(64)).is_ok(), "depth 64 objects is allowed");
        assert!(parse(&nested(65)).is_err(), "depth 65 objects is rejected");
    }

    #[test]
    fn keeps_integer_shapes() {
        let value = parse(r#"{"a":42,"b":-7,"c":1.5,"d":2e3,"e":18446744073709551615}"#).unwrap();
        assert!(matches!(value.get("a"), Some(Json::Int(42))));
        assert!(matches!(value.get("b"), Some(Json::Int(-7))));
        assert!(matches!(value.get("c"), Some(Json::Double(_))));
        assert!(matches!(value.get("d"), Some(Json::Double(_))));
        assert!(matches!(value.get("e"), Some(Json::UInt(_))));
    }

    #[test]
    fn rejects_bad_numbers_and_escapes() {
        assert!(parse("{\"a\":01}").is_err());
        assert!(parse("{\"a\":1.}").is_err());
        assert!(parse("{\"a\":+1}").is_err());
        assert!(parse(r#"{"a":"\uD800"}"#).is_err(), "lone surrogate");
        assert!(parse(r#"{"a":"\q"}"#).is_err());
        assert!(parse("{\"a\":1,\u{fffd}}").is_err());
    }

    #[test]
    fn enforces_size_limit() {
        let big = format!("{{\"a\":\"{}\"}}", "x".repeat(200));
        assert!(parse_strict_json_object(big.as_bytes(), 100, CODE).is_err());
    }
}
