pub mod appx;
pub mod codex_daemon;
pub mod elevation;
pub mod environment;
pub mod process;

pub use appx::discover_desktop_app;
pub use codex_daemon::{CodexCli, prepare_codex_daemon_for_launch, resolve_codex_cli};
pub use elevation::{ELEVATED_LAUNCH_UNSUPPORTED, ensure_non_elevated, is_elevated};
pub use environment::{apply_proxy_environment, proxy_environment};
pub use process::{desktop_process_state, launch_codex};
