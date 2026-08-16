mod child;
mod config_builder;
mod config_file;
mod discovery;
mod port;
mod runtime;

pub use config_builder::{ACTIVE_NODE_OUTBOUND_TAG, GUARD_INBOUND_TAG, SingBoxConfigBuilder};
pub use config_file::{PreparedSingBoxConfig, SING_BOX_CONFIG_FILE_NAME, SingBoxConfigFile};
pub use discovery::{SingBoxInstallation, SingBoxInstallationSource, SingBoxLocator};
pub use port::{LoopbackPortReservation, LoopbackProxyEndpoint};
pub use runtime::{
    SING_BOX_STARTUP_GRACE, SING_BOX_VALIDATION_TIMEOUT, SingBoxProcess, SingBoxRuntime,
    SingBoxShutdown,
};
