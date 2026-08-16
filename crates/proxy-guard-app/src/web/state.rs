use std::{path::PathBuf, sync::Arc, time::Instant};

use chrono::{DateTime, Utc};
use proxy_guard_core::{
    BenchmarkRunSummary, GuardConfig, NodeSelection, SubscriptionId, TaskResult,
};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

/// One currently tracked Web-initiated operation.
#[derive(Clone, Debug, Default)]
pub enum ManagerOperation {
    #[default]
    Idle,
    Syncing {
        subscription_id: SubscriptionId,
    },
    Benchmarking {
        started_at: DateTime<Utc>,
    },
    Failed {
        message: String,
    },
}

/// Shared state for one Local Web Manager server instance.
pub struct ManagerState {
    /// Route back into the TUI event loop for config / view / selection updates.
    pub tx: mpsc::Sender<TaskResult>,
    /// Current Guard configuration, kept in sync when the Web activates a change.
    pub config: Arc<Mutex<GuardConfig>>,
    pub config_path: PathBuf,
    /// Per-session 256-bit capability token; never persisted.
    pub token: String,
    /// Bound loopback port, used for Host/Origin checks.
    pub port: u16,
    /// Server lifecycle token; cancelling it stops the listener and the Web benchmark.
    pub shutdown: CancellationToken,
    pub last_activity: Arc<Mutex<Instant>>,
    pub operation: Arc<Mutex<ManagerOperation>>,
    /// Serializes Web mutations and benchmarks; busy requests get 409, not a queue.
    pub operation_permit: Arc<Semaphore>,
    pub manual_selection: Arc<Mutex<Option<NodeSelection>>>,
    pub last_benchmark: Arc<Mutex<Option<BenchmarkRunSummary>>>,
}

impl ManagerState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tx: mpsc::Sender<TaskResult>,
        config: GuardConfig,
        config_path: PathBuf,
        token: String,
        port: u16,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            tx,
            config: Arc::new(Mutex::new(config)),
            config_path,
            token,
            port,
            shutdown,
            last_activity: Arc::new(Mutex::new(Instant::now())),
            operation: Arc::new(Mutex::new(ManagerOperation::Idle)),
            operation_permit: Arc::new(Semaphore::new(1)),
            manual_selection: Arc::new(Mutex::new(None)),
            last_benchmark: Arc::new(Mutex::new(None)),
        }
    }
}
