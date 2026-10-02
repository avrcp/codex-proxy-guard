//! GUI protocol v1. No domain objects containing installation paths cross
//! this boundary. Malformed input is never echoed into errors.

use std::io::{self, BufRead};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json, value::RawValue};

pub const SCHEMA: u32 = 1;
pub const MAX_REQUEST: usize = 32 * 1024;
pub const MAX_RESPONSE: usize = 128 * 1024;
pub const MAX_ID: u64 = (1 << 53) - 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema: u32,
    id: u64,
    method: String,
    params: Box<RawValue>,
}

#[derive(Debug)]
pub struct Request {
    pub id: u64,
    pub method: Method,
}

#[derive(Debug)]
pub enum Method {
    Hello,
    Snapshot,
    SetProxy {
        host: String,
        port: u16,
    },
    Consent {
        enabled: bool,
        confirmation_token: String,
    },
    Launch {
        repair: bool,
        confirmation_token: Option<String>,
    },
    Cancel {
        operation_id: u64,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl BridgeError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }

    /// Domain errors can include executable paths and external text. Export
    /// only a bounded machine code and controlled, actionable public copy.
    pub fn engine(detail: &str) -> Self {
        // Unknown external text must not become a public code: even an
        // uppercase token may itself be a secret.
        let leading = detail.split([':', ' ', '\n', '\r']).next().unwrap_or("");
        let code = if PUBLIC_ENGINE_CODES.contains(&leading) {
            leading
        } else {
            "ENGINE_OPERATION_FAILED"
        };
        let message = match code {
            "BACKEND_PROXY_REQUIRED_FOR_REPAIR" => {
                "Authorize the Codex backend proxy configuration before repair."
            }
            "CODEX_ALREADY_RUNNING" | "DESKTOP_ALREADY_RUNNING" => {
                "Close Desktop before launching again."
            }
            "ELEVATED_LAUNCH_UNSUPPORTED" => "Restart Guard without administrator privileges.",
            "ELEVATION_QUERY_FAILED" => "Guard could not verify its elevation; launch is blocked.",
            "CONFIG_CHANGED" => "Configuration changed. Refresh and confirm again.",
            "CONFIG_LOCK_FAILED" => {
                "Configuration is busy or unavailable. Wait for the other Guard operation, then refresh."
            }
            "APPX_ACTIVATION_OUTCOME_UNKNOWN" => {
                "Activation may have been submitted. Check Desktop before retrying."
            }
            _ if code.contains("CANCEL") => {
                "The operation was cancelled. Check its activation outcome before retrying."
            }
            _ if code.contains("CONFLICT") => {
                "Existing proxy settings conflict with Guard's managed block."
            }
            _ => "The engine could not complete the operation. Refresh the state before retrying.",
        };
        Self::new(code, message)
    }
}

const PUBLIC_ENGINE_CODES: &[&str] = &[
    "CONFIG_CHANGED",
    "CONFIG_INVALID",
    "CONFIG_LOCK_FAILED",
    "CONFIG_LOCK_UNSUPPORTED",
    "CONFIG_READ_FAILED",
    "CONFIG_ROLLBACK_FAILED",
    "CONFIG_TOO_LARGE",
    "CONFIG_WRITE_FAILED",
    "ACTIVATION_ONLY_UNSUPPORTED",
    "APPX_ACTIVATION_COM_INIT_FAILED",
    "APPX_ACTIVATION_FAILED",
    "APPX_ACTIVATION_MANAGER_UNAVAILABLE",
    "APPX_ACTIVATION_OUTCOME_UNKNOWN",
    "APPX_ACTIVATION_PROTOCOL_INVALID",
    "APPX_ACTIVATION_UNSUPPORTED",
    "APPX_ACTIVATION_WORKER_IO",
    "APPX_ACTIVATION_WORKER_NO_RECEIPT",
    "APPX_ACTIVATION_WORKER_PATH_FAILED",
    "APPX_ACTIVATION_WORKER_START_FAILED",
    "APPX_APPLICATION_AMBIGUOUS",
    "APPX_APPLICATION_MISSING",
    "APPX_AUMID_MISMATCH",
    "APPX_AUMID_MISSING",
    "APPX_AUMID_QUERY_FAILED",
    "APPX_DISCOVERY_CANCELLED",
    "APPX_DISCOVERY_FAILED",
    "APPX_DISCOVERY_INVALID",
    "APPX_DISCOVERY_IO",
    "APPX_DISCOVERY_OUTPUT_LIMIT",
    "APPX_DISCOVERY_PROTOCOL_INVALID",
    "APPX_DISCOVERY_PROTOCOL_UNSUPPORTED",
    "APPX_DISCOVERY_TIMEOUT",
    "APPX_DISCOVERY_WAIT_FAILED",
    "APPX_EXECUTABLE_INVALID",
    "APPX_IDENTITY_MISMATCH",
    "APPX_IDENTITY_MISSING",
    "APPX_IDENTITY_QUERY_FAILED",
    "APPX_INSTALL_LOCATION_INVALID",
    "APPX_METADATA_INCOMPLETE",
    "APPX_OVERRIDE_INVALID",
    "APPX_PACKAGE_CHANGED",
    "APPX_TARGET_EXITED_EARLY",
    "APPX_TARGET_IMAGE_MISMATCH",
    "APPX_TARGET_OBSERVATION_FAILED",
    "APPX_TARGET_VERIFY_FAILED",
    "APPX_UNSUPPORTED",
    "BACKEND_PROXY_BLOCK_INVALID",
    "BACKEND_PROXY_CONFIG_CONFLICT",
    "BACKEND_PROXY_CONSENT_SAVE_FAILED",
    "BACKEND_PROXY_ENV_ENCODING",
    "BACKEND_PROXY_ENV_INVALID",
    "BACKEND_PROXY_ENV_IO",
    "BACKEND_PROXY_ENV_TASK_FAILED",
    "BACKEND_PROXY_REQUIRED_FOR_REPAIR",
    "BACKEND_PROXY_REVOKE_FAILED",
    "BACKEND_PROXY_REVOKE_TASK_FAILED",
    "BACKEND_PROXY_SCOPE_UNCONFIRMED",
    "CODEX_ALREADY_RUNNING",
    "CODEX_CLI_OVERRIDE_INVALID",
    "CODEX_CLI_UNAVAILABLE",
    "CODEX_DAEMON_STOP_FAILED",
    "CODEX_DAEMON_STOP_TIMEOUT",
    "CODEX_DAEMON_UNSUPPORTED",
    "CODEX_EXECUTABLE_INVALID",
    "CODEX_EXECUTABLE_MISSING",
    "CODEX_HOME_INVALID",
    "CODEX_HOME_UNRESOLVED",
    "CODEX_LAUNCH_FAILED",
    "CODEX_NOT_INSTALLED",
    "CODEX_RUNNING_UNKNOWN",
    "CONFIG_CHANGED",
    "CONFIG_INVALID",
    "CONFIG_LOCK_FAILED",
    "CONFIG_LOCK_UNSUPPORTED",
    "CONFIG_READ_FAILED",
    "CONFIG_ROLLBACK_FAILED",
    "CONFIG_TOO_LARGE",
    "CONFIG_WRITE_FAILED",
    "ELEVATED_LAUNCH_UNSUPPORTED",
    "ELEVATION_QUERY_FAILED",
    "INVALID_LAUNCH_OPTIONS",
    "LAUNCH_BUSY",
    "LAUNCH_CANCELLED",
    "PROXY_BYPASS_UNSUPPORTED",
    "PROXY_LAUNCH_PLAN_INVALID",
    "TARGET_ELEVATION_QUERY_FAILED",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyParams {
    host: String,
    port: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsentParams {
    enabled: bool,
    confirmation_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchParams {
    repair: bool,
    confirmation_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelParams {
    operation_id: u64,
}

fn params<T: serde::de::DeserializeOwned>(raw: &RawValue) -> Result<T, BridgeError> {
    serde_json::from_str(raw.get())
        .map_err(|_| BridgeError::new("BRIDGE_PARAMS_INVALID", "Request parameters are invalid."))
}

pub fn decode(line: &[u8]) -> Result<Request, (Option<u64>, BridgeError)> {
    let fail = |code, message| (None, BridgeError::new(code, message));
    if line.len() > MAX_REQUEST {
        return Err(fail("BRIDGE_REQUEST_TOO_LARGE", "Request exceeds 32 KiB."));
    }
    let text = std::str::from_utf8(line)
        .map_err(|_| fail("BRIDGE_INVALID_UTF8", "Request must be UTF-8."))?;
    let envelope: Envelope = serde_json::from_str(text).map_err(|_| {
        fail(
            "BRIDGE_INVALID_JSON",
            "Expected one strict JSON request object.",
        )
    })?;
    if envelope.id == 0 || envelope.id > MAX_ID {
        return Err(fail(
            "BRIDGE_ID_INVALID",
            "Request id must be a positive safe integer.",
        ));
    }
    let id = envelope.id;
    if envelope.schema != SCHEMA {
        return Err((
            Some(id),
            BridgeError::new(
                "BRIDGE_SCHEMA_UNSUPPORTED",
                "Engine requires protocol version 1.",
            ),
        ));
    }
    let raw = &envelope.params;
    let parsed = (|| {
        Ok(match envelope.method.as_str() {
            "hello" => {
                let _: Empty = params(raw)?;
                Method::Hello
            }
            "snapshot" => {
                let _: Empty = params(raw)?;
                Method::Snapshot
            }
            "shutdown" => {
                let _: Empty = params(raw)?;
                Method::Shutdown
            }
            "set_proxy" => {
                let p: ProxyParams = params(raw)?;
                Method::SetProxy {
                    host: p.host,
                    port: p.port,
                }
            }
            "set_backend_proxy_consent" => {
                let p: ConsentParams = params(raw)?;
                Method::Consent {
                    enabled: p.enabled,
                    confirmation_token: p.confirmation_token,
                }
            }
            "start_launch" => {
                let p: LaunchParams = params(raw)?;
                Method::Launch {
                    repair: p.repair,
                    confirmation_token: p.confirmation_token,
                }
            }
            "cancel_operation" => {
                let p: CancelParams = params(raw)?;
                if p.operation_id == 0 || p.operation_id > MAX_ID {
                    return Err(BridgeError::new(
                        "BRIDGE_PARAMS_INVALID",
                        "Invalid operation id.",
                    ));
                }
                Method::Cancel {
                    operation_id: p.operation_id,
                }
            }
            _ => {
                return Err(BridgeError::new(
                    "BRIDGE_METHOD_UNSUPPORTED",
                    "Unknown bridge method.",
                ));
            }
        })
    })();
    parsed
        .map(|method| Request { id, method })
        .map_err(|error| (Some(id), error))
}

pub fn success(id: u64, result: Value) -> Value {
    json!({ "schema": SCHEMA, "id": id, "ok": true, "result": result })
}

pub fn failure(id: Option<u64>, error: BridgeError) -> Value {
    json!({ "schema": SCHEMA, "id": id, "ok": false, "error": error })
}

pub fn encode(value: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_RESPONSE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bridge output exceeds limit",
        ));
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Bounded reader; oversized input closes the session instead of allocating
/// or attempting to drain an unbounded attacker-controlled line. EOF never
/// submits a partial command (especially a partial consent or launch).
pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(None);
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let count = newline.unwrap_or(chunk.len());
        if line.len() + count > MAX_REQUEST {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request exceeds 32 KiB",
            ));
        }
        line.extend_from_slice(&chunk[..count]);
        reader.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_requests_and_errors() {
        let valid = br#"{"schema":1,"id":1,"method":"hello","params":{}}"#;
        assert!(matches!(decode(valid).unwrap().method, Method::Hello));
        for bad in [
            b"{}".as_slice(),
            b"{}{}",
            b"[]",
            b"\xff",
            br#"{"schema":1,"schema":1,"id":1,"method":"hello","params":{}}"#,
        ] {
            assert!(decode(bad).is_err());
        }
        assert_eq!(
            decode(br#"{"schema":2,"id":1,"method":"hello","params":{}}"#)
                .unwrap_err()
                .1
                .code,
            "BRIDGE_SCHEMA_UNSUPPORTED"
        );
        assert_eq!(
            decode(br#"{"schema":1,"id":1,"method":"exec","params":{}}"#)
                .unwrap_err()
                .1
                .code,
            "BRIDGE_METHOD_UNSUPPORTED"
        );
        for bad in [
            r#"{"host":"localhost","host":"127.0.0.1","port":1}"#,
            r#"{"host":"localhost","port":1,"command":"x"}"#,
        ] {
            assert!(
                decode(
                    format!(r#"{{"schema":1,"id":2,"method":"set_proxy","params":{bad}}}"#)
                        .as_bytes()
                )
                .is_err()
            );
        }
    }
    #[test]
    fn framing_is_bounded_and_eof_does_not_submit() {
        assert!(read_frame(&mut io::Cursor::new(vec![b'a'; MAX_REQUEST + 1])).is_err());
        assert!(decode(&vec![b' '; MAX_REQUEST + 1]).is_err());
        assert_eq!(read_frame(&mut io::Cursor::new(b"partial")).unwrap(), None);
        assert_eq!(
            read_frame(&mut io::Cursor::new(b"{}\n")).unwrap(),
            Some(b"{}".to_vec())
        );
        assert!(encode(&json!("x".repeat(MAX_RESPONSE))).is_err());
    }
    #[test]
    fn errors_never_echo_external_paths_or_secrets() {
        let error = BridgeError::engine(
            "APPX_DISCOVERY_FAILED: C:\\Program Files\\WindowsApps\\secret token=hidden",
        );
        let wire = serde_json::to_string(&error).unwrap();
        assert!(wire.contains("APPX_DISCOVERY_FAILED"));
        assert!(!wire.contains("WindowsApps"));
        assert!(!wire.contains("hidden"));
        assert_eq!(
            BridgeError::engine("unknown error token=VERY_SECRET_VALUE").code,
            "ENGINE_OPERATION_FAILED"
        );
    }
}
