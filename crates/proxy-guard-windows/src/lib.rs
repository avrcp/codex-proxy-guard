pub mod appx;
pub mod codex_daemon;
pub mod elevation;
pub mod environment;
#[cfg(windows)]
pub mod package_identity;
pub mod packaged_launch;
pub mod process;
#[cfg(windows)]
mod system_tools;

pub use appx::discover_desktop_app;
pub use codex_daemon::{
    CodexCli, CodexHomeInput, DaemonStopBudget, resolve_codex_cli, resolve_codex_cli_from,
    stop_codex_daemon,
};
pub use elevation::{
    ELEVATED_LAUNCH_UNSUPPORTED, ElevationState, elevation_gate, ensure_non_elevated,
    query_elevation,
};
pub use environment::{apply_proxy_environment, proxy_environment};
pub use process::{LaunchHooks, desktop_process_state, launch_codex, launch_codex_with};
