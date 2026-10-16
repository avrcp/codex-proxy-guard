//! The single auditable Win32 boundary of Codex Proxy Guard. Every `unsafe`
//! call lives here behind safe-Rust wrappers with `SAFETY:` justifications;
//! no other crate touches Win32 directly.

pub mod activation;
pub mod appmodel;
pub mod cancellation;
pub mod clipboard;
pub mod console;
pub mod elevation;
pub mod environment;
pub mod fsio;
pub mod handles;
pub mod paths;
pub mod process;
pub mod process_state;
pub mod txf;

pub use cancellation::Cancellation;
pub use environment::UnicodeEnvironment;
pub use handles::OwnedHandle;
