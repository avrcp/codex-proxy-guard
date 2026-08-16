//! Concrete platform services for Codex Proxy Guard Managed Mode.
//!
//! This crate owns subscription fetching/parsing/storage, the JP/SG/US region
//! classifier, the sing-box sidecar runtime, exit-geography verification, the
//! two-stage benchmark, and the benchmark cache. It never touches Windows system
//! proxy settings, TUN/WFP/hooks, or Codex authentication.

pub mod benchmark;
pub mod error;
pub mod geo;
pub mod paths;
pub mod region;
pub mod secret;
pub mod singbox;
pub mod storage;
pub mod subscription;

pub use benchmark::{
    BenchmarkPhase, BenchmarkProgress, BenchmarkProgressSink, CodexPathProbe, NodeBenchmarkService,
    NodeSelector, NodeStatusView, ReqwestCodexPathProbe, VerifiedSidecar, build_report,
    node_fingerprint,
};
pub use error::NetworkError;
pub use geo::{
    GeoResolver, GeoTransport, IPWHOIS_PROVIDER_ID, IpWhoIsProvider, ReqwestGeoTransport,
};
pub use paths::ManagedPaths;
pub use region::RegionHintClassifier;
pub use secret::{KeyringSecretStore, SecretStore};
pub use singbox::{
    LoopbackPortReservation, LoopbackProxyEndpoint, PreparedSingBoxConfig, SingBoxConfigBuilder,
    SingBoxConfigFile, SingBoxInstallation, SingBoxLocator, SingBoxProcess, SingBoxRuntime,
};
pub use storage::{BenchmarkStore, NodeStore, StoredNode, StoredSubscription, SubscriptionStore};
pub use subscription::{
    HttpsSubscriptionFetcher, SubscriptionFetcher, SubscriptionParser, SubscriptionService,
    SubscriptionUpdate,
};
