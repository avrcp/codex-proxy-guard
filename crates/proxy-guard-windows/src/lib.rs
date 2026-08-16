pub mod appx;
pub mod environment;
pub mod process;

pub use appx::discover_desktop_app;
pub use environment::{
    apply_proxy_environment, proxy_environment, proxy_environment_with_endpoint,
};
pub use process::{desktop_process_state, launch_codex, launch_codex_with_proxy};
