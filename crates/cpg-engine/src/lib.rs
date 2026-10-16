//! The engine: configuration storage with held transactions, the consented
//! Home `.env` block, the launch pipeline and the authoritative
//! `EngineSession` state machine. The TUI consumes typed views; JSON is an
//! external adapter only.

pub mod config_store;
pub mod launch;
pub mod session;

pub use config_store::{
    ConfigLease, bound_home, consent_home, default_config_path, default_home, initialize_config,
    inspect_proxy_env, load_config, load_or_create_config, prepare_proxy_env, revoke_proxy_env,
    update_consent, update_proxy,
};
pub use launch::{
    LaunchOptions, ProxyPlan, activate_desktop, launch_pipeline, proxy_environment, proxy_plan,
    resolve_codex_cli, stop_codex_daemon,
};
pub use session::{
    ConsentProposal, EngineSession, Event, ForegroundState, LaunchOutcome, LaunchReceipt,
    MutationOutcome, RepairProposal, SessionBudgets, SessionServices, SessionView,
};
