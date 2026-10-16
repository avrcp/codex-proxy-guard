//! Pure domain contracts for Codex Proxy Guard: configuration, proxy
//! validation, redaction, strict JSON, dotenv block editing and worker
//! protocol validation. This crate performs no I/O and links no Win32 API;
//! every filesystem or OS boundary lives in `cpg-windows` / `cpg-engine`.

pub mod config;
pub mod discovery;
pub mod dotenv;
pub mod error;
pub mod json;
pub mod proxy;
pub mod text;
pub mod worker;

pub use config::Config;
pub use error::{Error, Result, fail};
pub use json::{Json, json_dump, parse_strict_json_object};
pub use proxy::parse_port_strict;
pub use text::{public_text, strict_utf8, utf8_to_wide, wide_to_utf8};
