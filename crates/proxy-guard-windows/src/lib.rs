pub mod appmodel_activation;
pub mod appx;
pub mod codex_daemon;
pub mod config_transaction;
pub mod elevation;
pub mod environment;
#[cfg(windows)]
pub mod package_identity;
pub mod process;
pub mod proxy_env_file;
pub mod proxy_launch_plan;
#[cfg(windows)]
pub mod system_tools;

pub use appmodel_activation::{
    ActivationOutcome, ActivationWorkerReceipt, ActivationWorkerRequest, PROTOCOL_VERSION,
    run_activation_worker,
};
pub use appx::discover_desktop_app;
#[cfg(windows)]
pub use appx::discovery_script;
pub use codex_daemon::{
    CodexCli, CodexHomeInput, DaemonStopBudget, resolve_codex_cli, resolve_codex_cli_from,
    stop_codex_daemon,
};
pub use config_transaction::{ConfigExpectation, GuardConfigLease, GuardConfigTransaction};
pub use elevation::{
    ELEVATED_LAUNCH_UNSUPPORTED, ElevationState, elevation_gate, ensure_non_elevated,
    query_elevation,
};
pub use environment::{apply_proxy_environment, proxy_environment};
pub use process::{
    BackendProxyScope, LaunchHooks, backend_proxy_scope, desktop_process_state,
    inspect_backend_proxy_state, launch_codex, launch_codex_with,
};
pub use proxy_env_file::{ProxyEnvValues, env_path, inspect, prepare, revoke};
pub use proxy_launch_plan::proxy_launch_plan;
