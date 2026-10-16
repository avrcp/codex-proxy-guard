//! Activation-worker protocol (schema v1): strict request validation and
//! receipt-shape validation, plus the daemon-stop status classifier. Pure
//! JSON logic; the COM activation and stdio framing live in `cpg-windows`
//! and `app`.

use crate::error::{Error, Result};
use crate::json::{Json, parse_strict_json_object};
use std::path::Path;

pub const REQUEST_LIMIT: usize = 16 * 1024;
pub const RECEIPT_LIMIT: usize = 64 * 1024;
pub const ACTIVATION_ERROR: &str = "APPX_ACTIVATION_PROTOCOL_INVALID";

pub const RECEIPT_KEYS: &[&str] = &[
    "version",
    "failure",
    "activation_returned",
    "activation_hresult",
    "pid",
    "instance",
    "package_identity",
    "observed_package_full_name",
    "aumid",
    "observed_aumid",
    "elevation",
    "early_exit_code",
    "observation_failure",
];

/// The canonical empty receipt (also the shape template for validation).
pub fn empty_receipt() -> Json {
    Json::object_from([
        ("version", Json::Int(1)),
        ("failure", Json::Null),
        ("activation_returned", Json::Bool(false)),
        ("activation_hresult", Json::Null),
        ("pid", Json::Null),
        ("instance", Json::from("unknown")),
        ("package_identity", Json::from("not_queried")),
        ("observed_package_full_name", Json::Null),
        ("aumid", Json::from("not_queried")),
        ("observed_aumid", Json::Null),
        ("elevation", Json::Null),
        ("early_exit_code", Json::Null),
        ("observation_failure", Json::Null),
    ])
}

fn fail_activation<T>(message: &str) -> Result<T> {
    Err(Error::new(ACTIVATION_ERROR, message))
}

fn exact_keys(object: &Json, allowed: &[&str]) -> Result<()> {
    if let Json::Object(map) = object {
        for key in map.keys() {
            if !allowed.contains(&key.as_str()) {
                return fail_activation("Unexpected protocol field");
            }
        }
    }
    Ok(())
}

/// Port of `detail::activationRequest` — parses and fully validates the
/// bounded worker request, returning it unchanged on success.
pub fn validate_activation_request(bytes: &[u8]) -> Result<Json> {
    let request = parse_strict_json_object(bytes, REQUEST_LIMIT, ACTIVATION_ERROR)?;
    exact_keys(
        &request,
        &[
            "version",
            "aumid",
            "expected_package_full_name",
            "expected_executable",
            "arguments",
        ],
    )?;
    let version_ok = match request.at("version") {
        Json::Int(value) => *value == 1,
        Json::UInt(value) => *value == 1,
        Json::Double(value) => *value == 1.0,
        _ => false,
    };
    if !version_ok {
        return fail_activation("Unsupported activation protocol version");
    }
    let aumid = string_field(&request, "aumid")?;
    let package = string_field(&request, "expected_package_full_name")?;
    let executable = string_field(&request, "expected_executable")?;
    let arguments = string_field(&request, "arguments")?;
    let identity_printable = |value: &str| value.bytes().all(|b| (0x21..=0x7e).contains(&b));
    let argument_printable = |value: &str| value.bytes().all(|b| (0x20..=0x7e).contains(&b));
    let exclamations = aumid.bytes().filter(|b| *b == b'!').count();
    if aumid.len() > 2048
        || !identity_printable(&aumid)
        || exclamations != 1
        || aumid.starts_with('!')
        || aumid.ends_with('!')
    {
        return fail_activation("Invalid application identity");
    }
    if package.len() > 1024 || !identity_printable(&package) {
        return fail_activation("Invalid package identity");
    }
    if executable.is_empty() || !Path::new(&executable).is_absolute() {
        return fail_activation("Expected executable must be absolute");
    }
    if arguments.len() > 4096 {
        return fail_activation("Activation arguments exceed 4 KiB");
    }
    if !argument_printable(&arguments) {
        return fail_activation("Activation arguments must be printable ASCII");
    }
    Ok(request)
}

fn string_field(object: &Json, key: &str) -> Result<String> {
    match object.get(key) {
        Some(Json::String(text)) if !text.contains('\0') => Ok(text.clone()),
        _ => fail_activation(&format!("Invalid string field: {key}")),
    }
}

fn unsigned_number(value: &Json, zero_allowed: bool) -> bool {
    let within = |number: f64| {
        number.is_finite()
            && number.fract() == 0.0
            && number >= if zero_allowed { 0.0 } else { 1.0 }
            && number <= 0xffff_ffffu64 as f64
    };
    match value {
        Json::UInt(number) => *number >= if zero_allowed { 0 } else { 1 } && *number <= 0xffff_ffff,
        Json::Int(number) => *number >= if zero_allowed { 0 } else { 1 } && *number <= 0xffff_ffff,
        Json::Double(number) => within(*number),
        _ => false,
    }
}

/// Port of `detail::activationReceipt` — validates the complete receipt
/// shape and the internal fact consistency rules.
pub fn validate_activation_receipt(bytes: &[u8]) -> Result<Json> {
    let receipt = parse_strict_json_object(bytes, RECEIPT_LIMIT, ACTIVATION_ERROR)?;
    exact_keys(&receipt, RECEIPT_KEYS)?;
    for key in RECEIPT_KEYS {
        if receipt.get(key).is_none() {
            return fail_activation("Invalid activation receipt shape");
        }
    }
    let shape = empty_receipt();
    if !matches!(
        (receipt.at("version"), shape.at("version")),
        (Json::Int(a), Json::Int(b)) if a == b
    ) {
        return fail_activation("Invalid activation receipt shape");
    }
    if !receipt.at("activation_returned").is_bool() {
        return fail_activation("Invalid activation receipt shape");
    }
    for key in [
        "failure",
        "activation_hresult",
        "observed_package_full_name",
        "observed_aumid",
        "observation_failure",
    ] {
        match receipt.at(key) {
            Json::Null | Json::String(_) => {}
            _ => return fail_activation("Invalid activation receipt shape"),
        }
    }
    for key in ["pid", "early_exit_code"] {
        let value = receipt.at(key);
        if !value.is_null() && !unsigned_number(value, key == "early_exit_code") {
            return fail_activation("Invalid process identifier or exit code");
        }
    }
    let elevation = receipt.at("elevation");
    if !elevation.is_null() && !elevation.is_bool() {
        return fail_activation("Invalid target elevation");
    }
    let instance = receipt.at("instance").as_str().unwrap_or_default();
    if !matches!(instance, "unknown" | "created" | "reused") {
        return fail_activation("Invalid instance observation");
    }
    for key in ["package_identity", "aumid"] {
        let observation = receipt.at(key).as_str().unwrap_or_default();
        if !matches!(
            observation,
            "not_queried" | "matched" | "missing" | "mismatch" | "query_failed"
        ) {
            return fail_activation("Invalid identity observation");
        }
    }
    let returned = receipt.at("activation_returned").as_bool().unwrap();
    if returned == receipt.at("pid").is_null()
        || (returned
            && (!receipt.at("failure").is_null() || !receipt.at("activation_hresult").is_null()))
    {
        return fail_activation("Contradictory activation facts");
    }
    Ok(receipt)
}

/// Port of `detail::daemonStopStatus` — accepts only the agreed lifecycle
/// statuses from one strict JSON object.
pub fn daemon_stop_status(output: &str) -> Option<&'static str> {
    let object =
        parse_strict_json_object(output.as_bytes(), 64 * 1024, "CODEX_DAEMON_STOP_FAILED").ok()?;
    match object.at("status").as_str() {
        Some("stopped") => Some("stopped"),
        Some("notRunning") => Some("not_needed"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_request() {
        let request = br#"{"version":1,"aumid":"Family!App","expected_package_full_name":"OpenAI.Codex_2p2nqsd0c76g0","expected_executable":"C:\\x\\ChatGPT.exe","arguments":"--proxy-server=http://127.0.0.1:10808"}"#;
        assert!(validate_activation_request(request).is_ok());
    }

    #[test]
    fn rejects_invalid_requests() {
        let base = |aumid: &str, arguments: &str| {
            format!(
                r#"{{"version":1,"aumid":"{aumid}","expected_package_full_name":"P","expected_executable":"C:\\x\\e.exe","arguments":"{arguments}"}}"#
            )
        };
        assert!(validate_activation_request(base("Family", "x").as_bytes()).is_err());
        assert!(validate_activation_request(base("A!B!C", "x").as_bytes()).is_err());
        assert!(validate_activation_request(base("!A!B", "x").as_bytes()).is_err());
        assert!(validate_activation_request(base("A!B", "bad\u{a0}arg").as_bytes()).is_err());
        assert!(validate_activation_request(base("A!B", "non\u{2013}ascii").as_bytes()).is_err());
        // Embedded NULs and duplicates are rejected by the strict parser.
        assert!(validate_activation_request(b"{\"version\":1,\"version\":1}").is_err());
        // Unknown fields.
        let extra = r#"{"version":1,"aumid":"A!B","expected_package_full_name":"P","expected_executable":"C:\\e.exe","arguments":"x","extra":1}"#;
        assert!(validate_activation_request(extra.as_bytes()).is_err());
        // Relative executable.
        let relative = r#"{"version":1,"aumid":"A!B","expected_package_full_name":"P","expected_executable":"e.exe","arguments":"x"}"#;
        assert!(validate_activation_request(relative.as_bytes()).is_err());
    }

    #[test]
    fn validates_receipt_shape_and_facts() {
        assert!(validate_activation_receipt(empty_receipt().dump().as_bytes()).is_ok());
        let mut receipt = empty_receipt();
        receipt.insert("activation_returned", Json::Bool(true));
        receipt.insert("pid", Json::Int(1234));
        assert!(validate_activation_receipt(receipt.dump().as_bytes()).is_ok());

        let mut receipt = empty_receipt();
        receipt.insert("activation_returned", Json::Bool(true));
        // Returned without a pid is contradictory.
        assert!(validate_activation_receipt(receipt.dump().as_bytes()).is_err());

        let mut receipt = empty_receipt();
        receipt.insert("activation_returned", Json::Bool(true));
        receipt.insert("pid", Json::Int(9));
        receipt.insert("failure", Json::from("x"));
        assert!(validate_activation_receipt(receipt.dump().as_bytes()).is_err());

        let mut receipt = empty_receipt();
        receipt.insert("pid", Json::Int(-3));
        assert!(validate_activation_receipt(receipt.dump().as_bytes()).is_err());

        let mut receipt = empty_receipt();
        receipt.insert("instance", Json::from("maybe"));
        assert!(validate_activation_receipt(receipt.dump().as_bytes()).is_err());

        let mut receipt = empty_receipt();
        receipt.insert("extra", Json::Null);
        assert!(validate_activation_receipt(receipt.dump().as_bytes()).is_err());
    }

    #[test]
    fn daemon_status_classification() {
        assert_eq!(
            daemon_stop_status(r#"{"status":"stopped"}"#),
            Some("stopped")
        );
        assert_eq!(
            daemon_stop_status(r#"{"status":"notRunning"}"#),
            Some("not_needed")
        );
        assert_eq!(daemon_stop_status(r#"{"status":"weird"}"#), None);
        assert_eq!(daemon_stop_status("not json"), None);
        assert_eq!(daemon_stop_status(r#"{"status":"stopped"} {"x":1}"#), None);
    }
}
