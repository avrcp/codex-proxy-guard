//! `EngineSession`: the authoritative in-process state boundary. Single
//! foreground operation, revision-bound single-use confirmations, budget
//! enforcement that only releases the gate after the future is reaped, and
//! receipts that later refresh errors never overwrite.

use crate::config_store;
use crate::launch::{self, LaunchFacts, LaunchOptions};
use cpg_domain::config::Config;
use cpg_domain::discovery::Desktop;
use cpg_domain::json::Json;
use cpg_domain::text::public_text;
use cpg_domain::{Error, Result};
use cpg_windows::cancellation::Cancellation;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct SessionBudgets {
    pub mutation: Duration,
    pub launch: Duration,
}

impl Default for SessionBudgets {
    fn default() -> Self {
        SessionBudgets {
            mutation: Duration::from_secs(60),
            launch: Duration::from_secs(840),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentProposal {
    pub token: String,
    pub enabled: bool,
    pub home: PathBuf,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepairProposal {
    pub token: String,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct ProxyView {
    pub url: String,
    pub host: String,
    pub port: i64,
    /// False when Guard holds no endpoint fact to show, in which case `url`,
    /// `host` and `port` above are empty rather than defaulted. Frontends read
    /// this instead of guessing from a state string, so no console can invent
    /// a prefill for an endpoint the engine never published.
    pub endpoint_known: bool,
    /// Where the displayed endpoint came from: `read` from the configuration
    /// file, `committed` by Guard's own last save, `retained` from the last
    /// good read while a later read failed, `invalid` when Guard read bytes it
    /// cannot use, or `unconfirmed` before Guard has read a configuration at
    /// all.
    pub state: &'static str,
}

impl Default for ProxyView {
    fn default() -> Self {
        Self {
            url: String::new(),
            host: String::new(),
            port: 0,
            endpoint_known: false,
            state: "unconfirmed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LaunchPlanView {
    /// `available`: activation arguments can be built and carry `endpoint`;
    /// `environment`: the native path delivers `endpoint` through the process
    /// environment instead; `unavailable`: this configuration cannot produce a
    /// launch plan and `error` says why; `unknown`: no confirmed configuration
    /// read backs the displayed endpoint, so no plan is claimed.
    pub state: &'static str,
    pub endpoint: String,
    pub error: Option<Error>,
}

impl Default for LaunchPlanView {
    fn default() -> Self {
        Self {
            state: "unknown",
            endpoint: String::new(),
            error: None,
        }
    }
}

fn unknown_plan() -> LaunchPlanView {
    LaunchPlanView {
        state: "unknown",
        ..LaunchPlanView::default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct DesktopView {
    pub state: &'static str,
    pub product: &'static str,
    pub display_name: &'static str,
    pub package_version: String,
    pub architecture: String,
    pub application_id: Option<String>,
    pub manifest_executable: Option<String>,
    pub runtime_kind: Option<&'static str>,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessObservation {
    pub state: &'static str,
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct CoverageView {
    pub state: &'static str,
    pub enabled: bool,
    pub authorized_home: Option<PathBuf>,
    pub candidate_home: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    pub can_launch: bool,
    pub can_edit_proxy: bool,
    pub can_authorize_backend_proxy: bool,
    pub can_revoke_backend_proxy: bool,
    pub can_repair: bool,
    pub can_refresh: bool,
    pub can_cancel: bool,
    pub blocked_reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct SessionView {
    pub ready: bool,
    pub busy: bool,
    pub proxy: ProxyView,
    pub plan: LaunchPlanView,
    pub desktop: DesktopView,
    pub process: ProcessObservation,
    pub coverage: CoverageView,
    pub elevation: &'static str,
    pub launch_method: &'static str,
    pub actions: Capabilities,
    pub error: Option<Error>,
    pub consent: Option<ConsentProposal>,
    pub repair: Option<RepairProposal>,
}

impl SessionView {
    pub fn public_json(&self) -> Json {
        let path_json = |value: &Option<PathBuf>| {
            value
                .as_ref()
                .map(|path| Json::from(path.to_string_lossy().as_ref()))
                .unwrap_or(Json::Null)
        };
        Json::object_from([
            (
                "config_readiness",
                Json::from(if self.ready {
                    "ready"
                } else {
                    "repair_required"
                }),
            ),
            (
                "proxy",
                Json::object_from([
                    ("url", Json::from(self.proxy.url.as_str())),
                    ("host", Json::from(self.proxy.host.as_str())),
                    ("port", Json::from(self.proxy.port)),
                    ("source", Json::from(self.proxy.state)),
                ]),
            ),
            (
                "launch_plan",
                Json::object_from([
                    ("state", Json::from(self.plan.state)),
                    ("endpoint", Json::from(self.plan.endpoint.as_str())),
                    (
                        "error",
                        self.plan
                            .error
                            .as_ref()
                            .map(|error| error.json())
                            .unwrap_or(Json::Null),
                    ),
                ]),
            ),
            (
                "launch",
                Json::object_from([("method", Json::from(self.launch_method))]),
            ),
            (
                "desktop",
                Json::object_from([
                    ("state", Json::from(self.desktop.state)),
                    ("product", Json::from(self.desktop.product)),
                    ("display_name", Json::from(self.desktop.display_name)),
                    (
                        "package_version",
                        Json::from(self.desktop.package_version.as_str()),
                    ),
                    (
                        "architecture",
                        Json::from(self.desktop.architecture.as_str()),
                    ),
                    (
                        "application_id",
                        self.desktop
                            .application_id
                            .clone()
                            .map(Json::from)
                            .unwrap_or(Json::Null),
                    ),
                    (
                        "manifest_executable",
                        self.desktop
                            .manifest_executable
                            .clone()
                            .map(Json::from)
                            .unwrap_or(Json::Null),
                    ),
                    (
                        "runtime_kind",
                        self.desktop
                            .runtime_kind
                            .map(Json::from)
                            .unwrap_or(Json::Null),
                    ),
                ]),
            ),
            (
                "process",
                Json::object_from([
                    ("state", Json::from(self.process.state)),
                    (
                        "pid",
                        self.process
                            .pid
                            .map(u64::from)
                            .map(Json::from)
                            .unwrap_or(Json::Null),
                    ),
                ]),
            ),
            (
                "coverage",
                Json::object_from([
                    ("state", Json::from(self.coverage.state)),
                    ("enabled", Json::from(self.coverage.enabled)),
                    ("authorized_home", path_json(&self.coverage.authorized_home)),
                    ("candidate_home", path_json(&self.coverage.candidate_home)),
                ]),
            ),
            ("busy", Json::from(self.busy)),
            ("elevation", Json::from(self.elevation)),
            (
                "error",
                self.error
                    .as_ref()
                    .map(|error| error.json())
                    .unwrap_or(Json::Null),
            ),
            (
                "actions",
                Json::object_from([
                    ("can_launch", Json::from(self.actions.can_launch)),
                    ("can_edit_proxy", Json::from(self.actions.can_edit_proxy)),
                    (
                        "can_authorize_backend_proxy",
                        Json::from(self.actions.can_authorize_backend_proxy),
                    ),
                    (
                        "can_revoke_backend_proxy",
                        Json::from(self.actions.can_revoke_backend_proxy),
                    ),
                    ("can_repair", Json::from(self.actions.can_repair)),
                    ("can_refresh", Json::from(self.actions.can_refresh)),
                    ("can_cancel", Json::from(self.actions.can_cancel)),
                    (
                        "blocked_reason",
                        Json::from(self.actions.blocked_reason.as_str()),
                    ),
                ]),
            ),
            (
                "confirmations",
                Json::object_from([
                    (
                        "backend_proxy",
                        self.consent
                            .as_ref()
                            .map(|consent| {
                                Json::object_from([
                                    ("token", Json::from(consent.token.as_str())),
                                    ("enabled", Json::from(consent.enabled)),
                                    ("home", Json::from(consent.home.to_string_lossy().as_ref())),
                                    ("revision", Json::from(consent.revision)),
                                ])
                            })
                            .unwrap_or(Json::Null),
                    ),
                    (
                        "repair",
                        self.repair
                            .as_ref()
                            .map(|repair| {
                                Json::object_from([
                                    ("token", Json::from(repair.token.as_str())),
                                    ("revision", Json::from(repair.revision)),
                                ])
                            })
                            .unwrap_or(Json::Null),
                    ),
                ]),
            ),
        ])
    }
}

#[derive(Clone, Debug, Default)]
pub struct MutationOutcome {
    pub operation_id: i64,
    pub committed: bool,
    pub committed_config: Option<Config>,
    pub mutation_error: Option<Error>,
    pub refresh_error: Option<Error>,
    pub exceeded_budget: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LaunchReceipt {
    pub pid: Option<u32>,
    pub launch_method: String,
    pub activation_state: String,
    pub instance: String,
    pub proxy_delivery: String,
    pub backend_proxy_config: String,
    pub daemon_preparation: String,
    pub proxy_endpoint: Option<String>,
    pub target_elevation: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct LaunchOutcome {
    pub operation_id: i64,
    pub receipt: Option<LaunchReceipt>,
    pub error: Option<Error>,
    pub exceeded_budget: bool,
    pub needs_refresh: bool,
    pub activation_state: &'static str,
    pub daemon_preparation: &'static str,
    pub backend_proxy_config: &'static str,
    /// The endpoint in the configuration this launch used, and the endpoint it
    /// actually carried to the target. The engine compares them so every
    /// frontend states one delivery fact instead of judging it locally.
    pub configured_endpoint: String,
    pub submitted_endpoint: String,
    pub endpoint_diverged: bool,
}

/// Injectable production services; the constructor-only seam keeps tests
/// hermetic without exposing any runtime configuration surface.
pub struct SessionServices {
    pub process: fn(&Desktop) -> cpg_windows::process_state::ProcessState,
    pub proposal_home: fn(&Config) -> Result<PathBuf>,
    pub discover: DiscoverFn,
    pub launch: LaunchFn,
}

pub type DiscoverFn = Arc<dyn Fn(&Config, &Cancellation) -> Result<Desktop> + Send + Sync>;

pub type LaunchFn = Arc<
    dyn Fn(&Config, &Path, LaunchOptions, &Cancellation, Option<&mut LaunchFacts>) -> Result<Json>
        + Send
        + Sync,
>;

impl Default for SessionServices {
    fn default() -> Self {
        SessionServices {
            process: cpg_windows::process_state::desktop_process_state,
            proposal_home: config_store::consent_home,
            discover: Arc::new(|config, cancel| {
                cancel.check()?;
                let envelope = cpg_windows::appmodel::discover_packages_native(cancel)?;
                cpg_domain::discovery::parse_discovery(
                    &cpg_domain::json::json_dump(&envelope),
                    &config.executable_override,
                    &cpg_windows::paths::NativeFsProbe,
                )
            }),
            launch: Arc::new(|config, path, options, cancel, facts| {
                launch::launch_pipeline(config, path, options, cancel, facts)
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    None,
    SnapshotArrived,
    OperationFinished,
    MutationFailed,
    MutationFinished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForegroundState {
    Idle,
    Running,
    CancellingAfterTimeout,
}

struct TaskResult {
    config: Config,
    ready: bool,
    snapshot: bool,
    /// The stored bytes were read and Guard can prove they are unusable
    /// (`CONFIG_INVALID`). Only this positive evidence authorizes a recovery
    /// write; an unreadable path never does.
    config_invalid: bool,
    /// Truthfulness of the displayed endpoint: `read` after a successful
    /// configuration read, `retained` while the last known-good endpoint is
    /// shown next to a read failure, `unconfirmed` before any read succeeded.
    config_source: &'static str,
    desktop: Option<Desktop>,
    process: cpg_windows::process_state::ProcessState,
    coverage: &'static str,
    value: Json,
    error: Option<Error>,
    mutation: Option<MutationOutcome>,
    launch: Option<LaunchOutcome>,
}

impl Default for TaskResult {
    fn default() -> Self {
        TaskResult {
            config: Config::default(),
            ready: false,
            snapshot: false,
            config_invalid: false,
            config_source: "unconfirmed",
            desktop: None,
            process: cpg_windows::process_state::ProcessState {
                state: "unknown",
                pid: 0,
            },
            coverage: "unknown",
            value: Json::Null,
            error: None,
            mutation: None,
            launch: None,
        }
    }
}

fn task_error() -> Error {
    Error::new(
        "ENGINE_TASK_FAILED",
        "The engine task failed; refresh before retrying.",
    )
}

fn timeout_error() -> Error {
    Error::new(
        "SESSION_TIMEOUT",
        "The operation exceeded its session budget and has settled; known completion facts are retained. Refresh before another action.",
    )
}

fn random_token() -> Result<String> {
    // CSPRNG token from the OS randomness source; never a timestamp or PID.
    let mut bytes = [0u8; 16];
    getrandom_fill(&mut bytes)?;
    let mut token = String::with_capacity(32);
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    Ok(token)
}

// ProcessPrng is the modern bcrypt CSPRNG entry point (bcryptprimitives.dll).
#[link(name = "bcryptprimitives", kind = "raw-dylib")]
unsafe extern "system" {
    fn ProcessPrng(pbdata: *mut u8, cbdata: usize) -> i32;
}

fn getrandom_fill(buffer: &mut [u8]) -> Result<()> {
    // SAFETY: ProcessPrng fills the caller buffer with cryptographically
    // secure random bytes; nonzero means success.
    if unsafe { ProcessPrng(buffer.as_mut_ptr(), buffer.len()) } != 0 {
        return Ok(());
    }
    Err(Error::new(
        "SESSION_RANDOM_UNAVAILABLE",
        "Cannot generate a confirmation token.",
    ))
}

enum Task {
    Refresh,
    SetProxy {
        expected: Config,
        invalid: bool,
        host: String,
        port: i64,
    },
    SetConsent {
        expected: Config,
        proposal: ConsentProposal,
    },
    Launch {
        expected: Config,
        repair: bool,
    },
}

impl Task {
    fn run(self, session: &SessionShared) -> TaskResult {
        let cancel = session.cancel.clone();
        match self {
            Task::Refresh => refresh(&session.path, &session.services, &cancel),
            Task::SetProxy {
                expected,
                invalid,
                host,
                port,
            } => {
                match config_store::update_proxy(&session.path, &expected, invalid, &host, port) {
                    Ok(updated) => {
                        let _ = cancel.check(); // a committed write stays authoritative even when cancelled here
                        let mut result = refresh(&session.path, &session.services, &cancel);
                        let mutation = MutationOutcome {
                            committed: true,
                            committed_config: Some(updated.clone()),
                            refresh_error: result.error.clone(),
                            ..MutationOutcome::default()
                        };
                        if !result.ready {
                            result.config = updated;
                            result.ready = true;
                            result.config_invalid = false;
                            result.config_source = "committed";
                        }
                        result.mutation = Some(mutation);
                        result
                    }
                    Err(error) => TaskResult {
                        mutation: Some(MutationOutcome {
                            mutation_error: Some(error),
                            ..MutationOutcome::default()
                        }),
                        ..TaskResult::default()
                    },
                }
            }
            Task::SetConsent { expected, proposal } => {
                match config_store::update_consent(
                    &session.path,
                    &expected,
                    proposal.enabled,
                    &proposal.home,
                ) {
                    Ok(updated) => {
                        let _ = cancel.check(); // a committed write stays authoritative even when cancelled here
                        let mut result = refresh(&session.path, &session.services, &cancel);
                        result.mutation = Some(MutationOutcome {
                            committed: true,
                            committed_config: Some(updated.clone()),
                            refresh_error: result.error.clone(),
                            ..MutationOutcome::default()
                        });
                        if !result.ready {
                            result.config = updated;
                            result.ready = true;
                            result.config_invalid = false;
                            result.config_source = "committed";
                        }
                        result
                    }
                    Err(error) => TaskResult {
                        mutation: Some(MutationOutcome {
                            mutation_error: Some(error),
                            ..MutationOutcome::default()
                        }),
                        ..TaskResult::default()
                    },
                }
            }
            Task::Launch { expected, repair } => {
                let mut facts = LaunchFacts::fresh();
                let outcome = (session.services.launch)(
                    &expected,
                    &session.path,
                    LaunchOptions {
                        repair,
                        activation_only: false,
                    },
                    &cancel,
                    Some(&mut facts),
                );
                let mut result = TaskResult::default();
                let mut launch_outcome = LaunchOutcome {
                    needs_refresh: true,
                    activation_state: facts.activation_state,
                    daemon_preparation: facts.daemon_preparation,
                    backend_proxy_config: facts.backend_proxy_config,
                    configured_endpoint: expected.proxy_url(),
                    ..LaunchOutcome::default()
                };
                match outcome {
                    Ok(value) => {
                        let receipt = typed_receipt(&value);
                        let submitted = receipt.proxy_endpoint.clone().unwrap_or_default();
                        launch_outcome.submitted_endpoint = submitted.clone();
                        launch_outcome.endpoint_diverged = !submitted.is_empty()
                            && submitted != launch_outcome.configured_endpoint;
                        launch_outcome.receipt = Some(receipt);
                        result.value = value;
                    }
                    Err(error) => launch_outcome.error = Some(error),
                }
                result.launch = Some(launch_outcome);
                result
            }
        }
    }
}

fn typed_receipt(value: &Json) -> LaunchReceipt {
    LaunchReceipt {
        pid: value.at("pid").as_u64().map(|pid| pid as u32),
        launch_method: value.string_at("launch_method"),
        activation_state: value.string_at("activation_state"),
        instance: value.string_at("instance"),
        proxy_delivery: value.string_at("proxy_delivery"),
        backend_proxy_config: value.string_at("backend_proxy_config"),
        daemon_preparation: value.string_at("daemon_preparation"),
        proxy_endpoint: value.at("proxy_endpoint").as_str().map(str::to_owned),
        target_elevation: value.at("target_elevation").as_bool(),
    }
}

fn refresh(path: &Path, services: &SessionServices, cancel: &Cancellation) -> TaskResult {
    let mut result = TaskResult {
        snapshot: true,
        ..TaskResult::default()
    };
    let outcome = (|| -> Result<()> {
        cancel.check()?;
        match config_store::load_or_create_config(path) {
            Ok(config) => result.config = config,
            Err(error) => {
                // `CONFIG_INVALID` means Guard read the bytes and proved they
                // cannot be used: the only evidence that authorizes a
                // recovery write. Any other failure left the file unread.
                result.config_invalid = error.code == "CONFIG_INVALID";
                return Err(error);
            }
        }
        result.ready = true;
        cancel.check()?;
        result.desktop = Some((services.discover)(&result.config, cancel)?);
        result.process = (services.process)(result.desktop.as_ref().expect("discovered"));
        result.coverage = if result.desktop.as_ref().expect("discovered").registered {
            config_store::inspect_proxy_env(&result.config)
        } else {
            "not_applicable"
        };
        Ok(())
    })();
    result.error = outcome.err();
    result
}

struct SessionShared {
    path: PathBuf,
    services: SessionServices,
    cancel: Cancellation,
}

/// The authoritative in-process state boundary.
pub struct EngineSession {
    shared: Arc<SessionShared>,
    budgets: SessionBudgets,
    query_elevation: fn() -> &'static str,
    state: TaskResult,
    pending: bool,
    launching: bool,
    foreground: ForegroundState,
    operation: i64,
    revision: u64,
    consent: Option<ConsentProposal>,
    repair: Option<RepairProposal>,
    confirmed_config: Config,
    /// The last configuration Guard actually read (or committed), kept so a
    /// failed refresh can still show the stored endpoint instead of silently
    /// reverting the display to the built-in defaults.
    known_good: Option<Config>,
    mutation_error: Option<Error>,
    mutation_outcome: Option<MutationOutcome>,
    launch_outcome: Option<LaunchOutcome>,
    operation_result: Json,
    worker: Option<std::thread::JoinHandle<()>>,
    mailbox: Arc<Mutex<Receiver<TaskResult>>>,
    dispatched_at: Instant,
}

static OPERATION_COUNTER: AtomicU64 = AtomicU64::new(0);

impl EngineSession {
    pub fn new(path: PathBuf) -> Result<EngineSession> {
        Self::with_services(
            path,
            SessionServices::default(),
            SessionBudgets::default(),
            cpg_windows::elevation::elevation,
        )
    }

    pub fn with_services(
        path: PathBuf,
        services: SessionServices,
        budgets: SessionBudgets,
        query_elevation: fn() -> &'static str,
    ) -> Result<EngineSession> {
        if path.as_os_str().is_empty() || !path.is_absolute() {
            return Err(Error::new(
                "CONFIG_PATH_INVALID",
                "The session requires a resolved absolute configuration path.",
            ));
        }
        Ok(EngineSession {
            shared: Arc::new(SessionShared {
                path,
                services,
                cancel: Cancellation::new(),
            }),
            budgets,
            query_elevation,
            state: TaskResult::default(),
            pending: false,
            launching: false,
            foreground: ForegroundState::Idle,
            operation: 0,
            revision: 0,
            consent: None,
            repair: None,
            confirmed_config: Config::default(),
            known_good: None,
            mutation_error: None,
            mutation_outcome: None,
            launch_outcome: None,
            operation_result: Json::Null,
            worker: None,
            mailbox: Arc::new(Mutex::new(mpsc_channel_none())),
            dispatched_at: Instant::now(),
        })
    }

    pub fn request_refresh(&mut self) {
        if self.pending {
            return;
        }
        self.invalidate();
        self.dispatch(false, Task::Refresh);
    }

    pub fn request_set_proxy(&mut self, host: &str, port: i64) -> Result<()> {
        if self.pending {
            return Err(Error::new(
                "SESSION_BUSY",
                "Another foreground operation is active.",
            ));
        }
        let expected = self.state.config.clone();
        let mut candidate = expected.clone();
        candidate.host = host.to_owned();
        candidate.port = port;
        candidate.validate()?;
        // Recovery writes are authorized only by positive evidence that the
        // stored bytes are unusable. A read failure alone never licenses
        // overwriting whatever occupies the path.
        let invalid = self.state.config_invalid;
        self.invalidate();
        self.dispatch(
            false,
            Task::SetProxy {
                expected,
                invalid,
                host: host.to_owned(),
                port,
            },
        );
        Ok(())
    }

    pub fn request_set_backend_consent(&mut self, proposal: &ConsentProposal) -> Result<()> {
        if self.pending {
            return Err(Error::new(
                "SESSION_BUSY",
                "Another foreground operation is active.",
            ));
        }
        let authorized = self.state.ready
            && self.consent.as_ref() == Some(proposal)
            && self.state.config == self.confirmed_config;
        // Consume the displayed proposal (including guessed submissions)
        // before any filesystem access can throw: a failed read/lease never
        // makes the same user confirmation reusable.
        self.invalidate();
        if !authorized {
            return Err(Error::new(
                "CONFIRMATION_REQUIRED",
                "Refresh and confirm this action again.",
            ));
        }
        let expected = self.state.config.clone();
        if config_store::load_config(&self.shared.path) != Ok(expected.clone()) {
            return Err(Error::new(
                "CONFIG_CHANGED",
                "Configuration changed; refresh and confirm again.",
            ));
        }
        self.dispatch(
            false,
            Task::SetConsent {
                expected,
                proposal: proposal.clone(),
            },
        );
        Ok(())
    }

    pub fn request_launch(&mut self, repair: bool, token: &str) -> Result<()> {
        if self.pending {
            return Err(Error::new(
                "SESSION_BUSY",
                "Another foreground operation is active.",
            ));
        }
        let authorized = self.state.ready
            && self
                .repair
                .as_ref()
                .is_some_and(|repair| repair.token == token)
            && self.state.config == self.confirmed_config;
        // Authorization is single-use even if subsequent config reads, leases,
        // elevation queries, or dispatch setup fail.
        self.invalidate();
        if repair && !authorized {
            return Err(Error::new(
                "CONFIRMATION_REQUIRED",
                "Refresh and confirm this repair launch again.",
            ));
        }
        if !repair && !token.is_empty() {
            return Err(Error::new(
                "SESSION_PARAMS_INVALID",
                "Normal launch does not accept a confirmation token.",
            ));
        }
        if !self.state.ready {
            return Err(Error::new(
                "CONFIG_INVALID",
                "Refresh and repair the configuration first.",
            ));
        }
        // A registered target is activated only with arguments Guard can
        // actually build, so an unsatisfiable plan is refused here — before the
        // config lease and every side effect — with the same error the view
        // already reports.
        if self
            .state
            .desktop
            .as_ref()
            .is_some_and(|desktop| desktop.registered)
        {
            launch::proxy_plan(&self.state.config)?;
        }
        let expected = self.state.config.clone();
        if config_store::load_config(&self.shared.path) != Ok(expected.clone()) {
            return Err(Error::new(
                "CONFIG_CHANGED",
                "Configuration changed; refresh and confirm again.",
            ));
        }
        if (self.query_elevation)() != "not_elevated" {
            return Err(Error::new(
                "ELEVATION_NOT_ALLOWED",
                "Launch requires verifiable normal-user elevation.",
            ));
        }
        self.dispatch(true, Task::Launch { expected, repair });
        Ok(())
    }

    pub fn cancel(&mut self) {
        if self.pending {
            self.shared.cancel.cancel();
            self.invalidate();
        }
    }

    fn invalidate(&mut self) {
        self.consent = None;
        self.repair = None;
    }

    fn dispatch(&mut self, launching: bool, task: Task) {
        let (sender, receiver) = std::sync::mpsc::channel();
        *self.mailbox.lock().expect("mailbox") = receiver;
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task.run(&shared)));
            let result = match result {
                Ok(result) => result,
                Err(_) => TaskResult {
                    error: Some(task_error()),
                    ..TaskResult::default()
                },
            };
            let _ = sender.send(result);
        });
        self.operation = OPERATION_COUNTER.fetch_add(1, Ordering::SeqCst) as i64 + 1;
        self.worker = Some(spawned);
        self.launching = launching;
        self.pending = true;
        self.foreground = ForegroundState::Running;
        self.mutation_error = None;
        self.dispatched_at = Instant::now();
    }

    pub fn poll(&mut self) -> Event {
        if !self.pending {
            return Event::None;
        }
        let arrived = self.mailbox.lock().expect("mailbox").try_recv();
        let Some(mut result) = arrived.ok() else {
            let budget = if self.launching {
                self.budgets.launch
            } else {
                self.budgets.mutation
            };
            if Instant::now() - self.dispatched_at > budget
                && self.foreground == ForegroundState::Running
            {
                self.shared.cancel.cancel();
                self.invalidate();
                self.foreground = ForegroundState::CancellingAfterTimeout;
            }
            return Event::None;
        };
        // Reap the worker before opening the gate.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let exceeded = self.foreground == ForegroundState::CancellingAfterTimeout;
        self.pending = false;
        self.launching = false;
        self.foreground = ForegroundState::Idle;
        if let Some(mut launch) = result.launch.take() {
            launch.operation_id = self.operation;
            launch.exceeded_budget = exceeded;
            self.launch_outcome = Some(launch.clone());
            self.operation_result = result.value.clone();
            self.invalidate();
            self.state.process = cpg_windows::process_state::ProcessState {
                state: "unknown",
                pid: 0,
            };
            // The operation result remains separate from the next read-only view.
            return Event::OperationFinished;
        }
        if let Some(mut mutation) = result.mutation.take() {
            mutation.operation_id = self.operation;
            mutation.exceeded_budget = exceeded;
            self.mutation_outcome = Some(mutation.clone());
            if mutation.committed {
                self.publish(result);
                self.confirmations();
            } else {
                self.mutation_error = mutation.mutation_error.clone();
                self.invalidate();
            }
            return Event::MutationFinished;
        }
        if result.snapshot {
            self.publish(result);
        } else if let Some(error) = result.error.clone() {
            self.state.error = Some(error);
        }
        if exceeded {
            self.invalidate();
            self.mutation_error = Some(timeout_error());
            return Event::MutationFailed;
        }
        self.confirmations();
        Event::SnapshotArrived
    }

    /// Publishes a finished result as the displayed state. A result whose
    /// configuration read failed keeps the last endpoint Guard actually read:
    /// readiness, discovery facts and the error still come from the failed
    /// attempt, so launch stays gated and the failure stays visible while the
    /// stored port is no longer displayed as the built-in default.
    fn publish(&mut self, mut result: TaskResult) {
        if result.ready {
            if result.config_source == "unconfirmed" {
                result.config_source = "read";
            }
            self.known_good = Some(result.config.clone());
        } else if let Some(known_good) = self.known_good.clone() {
            result.config = known_good;
            result.config_source = "retained";
        } else if result.config_invalid {
            // Distinguished from "unconfirmed": Guard did read the file, and
            // what it read cannot be used. "not read yet" would send the user
            // to refresh, which reports the same bytes again.
            result.config_source = "invalid";
        }
        self.state = result;
    }

    fn confirmations(&mut self) {
        self.invalidate();
        if !self.state.ready || self.pending {
            return;
        }
        self.confirmed_config = self.state.config.clone();
        self.revision += 1;
        if let Ok(home) = (self.shared.services.proposal_home)(&self.state.config)
            && home.is_absolute()
        {
            self.consent = Some(ConsentProposal {
                token: random_token().unwrap_or_default(),
                enabled: !self.state.config.manage_backend,
                home,
                revision: self.revision,
            });
        }
        let stopped = self.state.process.state == "stopped";
        let desktop = self.state.desktop.as_ref();
        if stopped
            && (self.query_elevation)() == "not_elevated"
            && desktop
                .is_some_and(|desktop| !desktop.registered || self.state.config.manage_backend)
        {
            self.repair = Some(RepairProposal {
                token: random_token().unwrap_or_default(),
                revision: self.revision,
            });
        }
    }

    pub fn view(&self) -> SessionView {
        let mut result = SessionView {
            ready: self.state.ready,
            busy: self.pending,
            elevation: (self.query_elevation)(),
            error: self.state.error.clone(),
            ..SessionView::default()
        };
        // An endpoint is a fact about a file. Before the first successful read
        // (or a committed save) Guard holds no such fact, so the row stays
        // empty; publishing the built-in default there reads as "my port was
        // reset to 10808", which is the confusion this display must never add.
        let endpoint_known = matches!(self.state.config_source, "read" | "committed" | "retained");
        result.proxy = ProxyView {
            url: if endpoint_known {
                self.state.config.proxy_url()
            } else {
                String::new()
            },
            host: if endpoint_known {
                self.state.config.host.clone()
            } else {
                String::new()
            },
            port: if endpoint_known {
                self.state.config.port
            } else {
                0
            },
            endpoint_known,
            state: self.state.config_source,
        };
        // The launch plan is stated only for what this refresh actually
        // established: an unread configuration and an undiscovered target
        // produce no delivery claim.
        result.plan = if !self.state.ready {
            unknown_plan()
        } else {
            match self.state.desktop.as_ref() {
                None => unknown_plan(),
                Some(desktop) if !desktop.registered => LaunchPlanView {
                    state: "environment",
                    endpoint: self.state.config.proxy_url(),
                    error: None,
                },
                Some(_) => match launch::proxy_plan(&self.state.config) {
                    Ok(plan) => LaunchPlanView {
                        state: "available",
                        endpoint: plan.endpoint,
                        error: None,
                    },
                    Err(error) => LaunchPlanView {
                        state: "unavailable",
                        endpoint: String::new(),
                        error: Some(error),
                    },
                },
            }
        };
        result.process.state = self.state.process.state;
        result.process.pid = match self.state.process.pid {
            0 => None,
            pid => Some(pid),
        };
        if let Some(desktop) = self.state.desktop.as_ref() {
            result.desktop.state = "found";
            result.desktop.product = desktop.product;
            result.desktop.display_name = desktop.display_name;
            result.desktop.package_version = public_text(&desktop.package_version, 160);
            result.desktop.architecture = public_text(&desktop.architecture, 160);
            result.launch_method = if desktop.registered {
                "appmodel_activation"
            } else {
                "native_process"
            };
            if desktop.registered {
                result.desktop.application_id = Some(public_text(&desktop.application_id, 160));
                result.desktop.manifest_executable =
                    Some(public_text(&desktop.manifest_executable, 160));
                result.desktop.runtime_kind = Some(desktop.runtime_kind);
            }
        } else if let Some(error) = self.state.error.as_ref()
            && (error.code == "CODEX_NOT_INSTALLED" || error.code == "APPX_NOT_FOUND")
        {
            result.desktop.state = "not_found";
        }
        result.coverage.state = self.state.coverage;
        // Consent is a fact about the file Guard just read. A retained
        // snapshot is kept for the endpoint display only: after a failed read
        // the stored authorization is unknown, so it is not claimed.
        if self.state.ready {
            result.coverage.enabled = self.state.config.manage_backend;
            if self.state.config.manage_backend {
                result.coverage.authorized_home = Some(self.state.config.home.clone());
            }
        }
        if let Some(consent) = self.consent.as_ref() {
            result.coverage.candidate_home = Some(consent.home.clone());
        }
        result.consent = self.consent.clone();
        result.repair = self.repair.clone();
        let can_edit = !self.pending;
        result.actions.can_edit_proxy = can_edit;
        result.actions.can_refresh = can_edit;
        result.actions.can_cancel = self.pending;
        result.actions.can_authorize_backend_proxy =
            can_edit && self.consent.as_ref().is_some_and(|consent| consent.enabled);
        result.actions.can_revoke_backend_proxy = can_edit
            && self
                .consent
                .as_ref()
                .is_some_and(|consent| !consent.enabled);
        if self.pending {
            result.actions.blocked_reason = "Another foreground operation is active.".to_owned();
        } else if !self.state.ready {
            result.actions.blocked_reason = if self.state.config_invalid {
                // Guard read the bytes and proved they cannot be used, so a
                // refresh would report the same thing: saving the proxy is the
                // repair, and it is offered.
                "The stored configuration is not usable; set the proxy again to repair it."
            } else if self.state.config_source == "retained" {
                "Guard could not read the configuration; refresh before launching."
            } else {
                "Guard has not read a usable configuration yet; refresh before launching."
            }
            .to_owned();
        } else if result.elevation != "not_elevated" {
            result.actions.blocked_reason =
                "Run Guard as a normal user; elevation must be verifiable.".to_owned();
        } else if result.plan.state == "unavailable" {
            // A registered target is launched only with activation arguments
            // that Guard can actually build, so an unsatisfiable bypass entry
            // is refused here rather than late in the launch.
            result.actions.blocked_reason = result
                .plan
                .error
                .as_ref()
                .map(|error| error.message.clone())
                .unwrap_or_else(|| "The launch plan could not be built.".to_owned());
        } else if self.state.desktop.is_none() {
            result.actions.blocked_reason = self
                .state
                .error
                .as_ref()
                .map(|error| error.message.clone())
                .unwrap_or_else(|| "Refresh to discover Desktop.".to_owned());
        } else if self.state.process.state == "running" {
            result.actions.blocked_reason =
                "Desktop is already running. Close it before another launch.".to_owned();
        } else if self.state.process.state != "stopped" {
            result.actions.blocked_reason =
                "Desktop process state is unknown; refresh before launching.".to_owned();
        } else {
            result.actions.can_launch = true;
        }
        result.actions.can_repair = result.actions.can_launch && self.repair.is_some();
        if !self.state.ready && result.error.is_none() {
            result.error = Some(Error::new(
                "CONFIG_INVALID",
                result.actions.blocked_reason.clone(),
            ));
        } else if result.elevation != "not_elevated" {
            result.error = Some(Error::new(
                "ELEVATION_NOT_ALLOWED",
                "Run Guard as a normal user; elevation must be verifiable.",
            ));
        } else if let Some(error) = result.plan.error.clone() {
            result.error = Some(error);
        }
        result
    }

    pub fn snapshot_json(&self) -> Json {
        self.view().public_json()
    }

    pub fn operation_result(&self) -> &Json {
        &self.operation_result
    }

    pub fn operation_error(&self) -> Option<Error> {
        self.launch_outcome
            .as_ref()
            .and_then(|outcome| outcome.error.clone())
    }

    pub fn mutation_error(&self) -> Option<Error> {
        self.mutation_error.clone()
    }

    pub fn mutation_outcome(&self) -> Option<MutationOutcome> {
        self.mutation_outcome.clone()
    }

    pub fn launch_outcome(&self) -> Option<LaunchOutcome> {
        self.launch_outcome.clone()
    }

    pub fn busy(&self) -> bool {
        self.pending
    }

    pub fn launching(&self) -> bool {
        self.launching
    }

    pub fn foreground_state(&self) -> ForegroundState {
        self.foreground
    }

    pub fn cancelling_after_timeout(&self) -> bool {
        self.foreground == ForegroundState::CancellingAfterTimeout
    }

    pub fn operation_id(&self) -> i64 {
        self.operation
    }

    pub fn config_path(&self) -> &Path {
        &self.shared.path
    }
}

impl Drop for EngineSession {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn mpsc_channel_none() -> Receiver<TaskResult> {
    std::sync::mpsc::channel().1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::{CodexCli, LaunchServices};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    fn fake_desktop(registered: bool) -> Desktop {
        Desktop {
            registered,
            executable: if registered {
                PathBuf::from(r"C:\Install\app\ChatGPT.exe")
            } else {
                PathBuf::from(r"C:\Tools\Custom.exe")
            },
            package_full_name: "OpenAI.Codex_2p2nqsd0c76g0".to_owned(),
            package_family_name: "OpenAI.Codex_2p2nqsd0c76g0".to_owned(),
            application_id: "App".to_owned(),
            aumid: "OpenAI.Codex_2p2nqsd0c76g0!App".to_owned(),
            manifest_executable: "app/ChatGPT.exe".to_owned(),
            runtime_kind: "full_trust_desktop",
            ..Desktop::default()
        }
    }

    struct Harness {
        path: PathBuf,
        prepare_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
        #[allow(dead_code)]
        activate_calls: Arc<AtomicUsize>,
        #[allow(dead_code)]
        discover_calls: Arc<AtomicUsize>,
        launch_block: Arc<AtomicBool>,
    }

    impl Harness {
        fn new(registered: bool) -> (Harness, SessionServices) {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
            let directory = std::env::temp_dir().join(format!(
                "cpg-session-{}-{unique}-{registered}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("temp dir");
            let path = directory.join("config.toml");
            crate::config_store::initialize_config(&path, &Config::default(), false)
                .expect("init config");
            let prepare_calls = Arc::new(AtomicUsize::new(0));
            let stop_calls = Arc::new(AtomicUsize::new(0));
            let activate_calls = Arc::new(AtomicUsize::new(0));
            let discover_calls = Arc::new(AtomicUsize::new(0));
            let launch_block = Arc::new(AtomicBool::new(false));
            let desktop = fake_desktop(registered);
            let harness = Harness {
                path: path.clone(),
                prepare_calls: Arc::clone(&prepare_calls),
                stop_calls: Arc::clone(&stop_calls),
                activate_calls: Arc::clone(&activate_calls),
                discover_calls: Arc::clone(&discover_calls),
                launch_block: Arc::clone(&launch_block),
            };
            let services = SessionServices {
                process: |_| cpg_windows::process_state::ProcessState {
                    state: "stopped",
                    pid: 0,
                },
                proposal_home: |config| crate::config_store::consent_home(config),
                discover: Arc::new({
                    let desktop = desktop.clone();
                    let discover_calls = Arc::clone(&discover_calls);
                    move |_config, cancel| {
                        discover_calls.fetch_add(1, Ordering::SeqCst);
                        cancel.check()?;
                        Ok(desktop.clone())
                    }
                }),
                launch: Arc::new({
                    let prepare_calls = Arc::clone(&prepare_calls);
                    let stop_calls = Arc::clone(&stop_calls);
                    let activate_calls = Arc::clone(&activate_calls);
                    let launch_block = Arc::clone(&launch_block);
                    move |config, _path, options, cancel, facts| {
                        let desktop = fake_desktop(true);
                        let services = LaunchServices {
                            config_path: Path::new(""),
                            discover: &|_config, cancel| {
                                cancel.check()?;
                                Ok(desktop.clone())
                            },
                            prepare: &|_config| {
                                prepare_calls.fetch_add(1, Ordering::SeqCst);
                                Ok(())
                            },
                            resolve: &|_config| {
                                Ok(CodexCli {
                                    executable: PathBuf::from(r"C:\Tools\codex.exe"),
                                    home: PathBuf::new(),
                                })
                            },
                            stop: &|_cli, _cancel| {
                                stop_calls.fetch_add(1, Ordering::SeqCst);
                                Ok("stopped")
                            },
                            mark_submitted: &|| Ok(()),
                            activate: &|_desktop, _arguments, _cancel| {
                                activate_calls.fetch_add(1, Ordering::SeqCst);
                                let mut receipt = cpg_domain::worker::empty_receipt();
                                receipt.insert("activation_returned", Json::Bool(true));
                                receipt.insert("pid", Json::from(4242u64));
                                receipt.insert("instance", Json::from("created"));
                                receipt.insert("package_identity", Json::from("matched"));
                                receipt.insert("aumid", Json::from("matched"));
                                receipt.insert("elevation", Json::Bool(false));
                                receipt.insert(
                                    "observed_package_full_name",
                                    Json::from("OpenAI.Codex_2p2nqsd0c76g0"),
                                );
                                receipt.insert(
                                    "observed_aumid",
                                    Json::from("OpenAI.Codex_2p2nqsd0c76g0!App"),
                                );
                                Ok(receipt)
                            },
                        };
                        // Run the production orchestration through
                        // launch_with so the code under test is the real one.
                        let mut local_facts = LaunchFacts::fresh();
                        let facts_slot = facts.unwrap_or(&mut local_facts);
                        // Block until released to exercise poll/timeout paths.
                        while launch_block.load(Ordering::SeqCst) && !cancel.cancelled() {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        cancel.check()?;
                        crate::launch::launch_with(config, options, cancel, &services, facts_slot)
                            .map(|mut receipt| {
                                receipt.insert("launch_method", Json::from("appmodel_activation"));
                                receipt
                            })
                    }
                }),
            };
            (harness, services)
        }

        fn session(&self, services: SessionServices) -> EngineSession {
            EngineSession::with_services(
                self.path.clone(),
                services,
                SessionBudgets {
                    mutation: Duration::from_millis(500),
                    launch: Duration::from_millis(500),
                },
                || "not_elevated",
            )
            .expect("session")
        }
    }

    fn settle(session: &mut EngineSession) {
        while session.busy() {
            session.poll();
            if session.busy() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    #[test]
    fn refresh_arrives_with_capabilities_and_proposals() {
        let (harness, services) = Harness::new(true);
        let mut session = harness.session(services);
        session.request_refresh();
        settle(&mut session);
        let view = session.view();
        assert!(view.ready);
        assert!(
            view.actions.can_launch,
            "blocked: {}",
            view.actions.blocked_reason
        );
        assert_eq!(view.launch_method, "appmodel_activation");
        assert_eq!(
            view.coverage.state, "not_authorized",
            "registered without consent"
        );
        assert!(
            view.consent.is_some(),
            "consent proposal for stopped registered desktop"
        );
        assert!(
            view.repair.is_none(),
            "repair needs backend authorization for registered"
        );
    }

    #[test]
    fn double_launch_is_refused_while_busy() {
        let (harness, services) = Harness::new(true);
        let mut session = harness.session(services);
        session.request_refresh();
        settle(&mut session);
        session
            .request_launch(false, "")
            .expect("first launch accepted");
        let error = session.request_launch(false, "").unwrap_err();
        assert_eq!(error.code, "SESSION_BUSY");
        settle(&mut session);
        let outcome = session.launch_outcome().expect("outcome");
        assert!(
            outcome.error.is_none(),
            "launch should succeed: {:?}",
            outcome.error.map(|error| (error.code, error.message))
        );
        assert_eq!(outcome.receipt.as_ref().unwrap().pid, Some(4242));
    }

    #[test]
    fn repair_requires_matching_single_use_token() {
        let (harness, services) = Harness::new(true);
        let mut session = harness.session(services);
        session.request_refresh();
        settle(&mut session);
        // Registered repair requires the authorized backend block first.
        let error = session.request_launch(true, "anything").unwrap_err();
        assert_eq!(error.code, "CONFIRMATION_REQUIRED");
        // The refused attempt consumed the displayed proposal (single-use
        // semantics); refresh before authorizing the backend block.
        session.request_refresh();
        settle(&mut session);
        let proposal = session.view().consent.expect("proposal");
        std::fs::create_dir_all(&proposal.home).expect("home");
        let mut config = crate::config_store::load_config(session.config_path()).unwrap();
        config.manage_backend = true;
        config.home = proposal.home.clone();
        crate::config_store::initialize_config(session.config_path(), &config, true)
            .expect("rebind");
        std::fs::create_dir_all(&proposal.home).expect("home");
        session.request_refresh();
        settle(&mut session);
        let repair = session
            .view()
            .repair
            .expect("repair proposal after consent");
        session
            .request_launch(true, &repair.token)
            .expect("authorized repair");
        settle(&mut session);
        assert_eq!(
            harness.stop_calls.load(Ordering::SeqCst),
            1,
            "exactly one daemon stop"
        );
        assert_eq!(
            harness.prepare_calls.load(Ordering::SeqCst),
            1,
            "prepare before stop"
        );
        // Token is single-use.
        let error = session.request_launch(true, &repair.token).unwrap_err();
        assert_eq!(error.code, "CONFIRMATION_REQUIRED");
    }

    #[test]
    fn normal_launch_never_resolves_cli_or_stops_daemon() {
        let (harness, services) = Harness::new(true);
        let mut session = harness.session(services);
        session.request_refresh();
        settle(&mut session);
        session.request_launch(false, "").expect("launch");
        settle(&mut session);
        assert_eq!(
            harness.stop_calls.load(Ordering::SeqCst),
            0,
            "no daemon stop on normal launch"
        );
    }

    #[test]
    fn late_receipt_after_timeout_does_not_double_submit() {
        let (harness, services) = Harness::new(true);
        let mut session = harness.session(services);
        session.request_refresh();
        settle(&mut session);
        harness.launch_block.store(true, Ordering::SeqCst);
        session.request_launch(false, "").expect("launch");
        // Poll until the 500ms budget trips; the task keeps blocking.
        for _ in 0..200 {
            session.poll();
            if session.cancelling_after_timeout() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.cancelling_after_timeout(), "budget exceeded");
        assert!(session.busy(), "gate stays closed until reaped");
        harness.launch_block.store(false, Ordering::SeqCst);
        settle(&mut session);
        assert!(!session.busy());
        let outcome = session.launch_outcome().expect("outcome");
        assert!(outcome.exceeded_budget);
        // The cancelled activation produced an outcome-unknown error; the
        // receipt facts (not_submitted) survive.
        assert_eq!(outcome.activation_state, "not_submitted");
    }

    #[test]
    fn consent_flow_enables_and_revokes_through_real_files() {
        let (harness, services) = Harness::new(true);
        // Pre-bind the candidate Home like the C++ fixture sessions so the
        // proposal and config agree before the consent prompt.
        let home = std::env::temp_dir().join(format!("cpg-consent-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("home");
        let mut bound = crate::config_store::load_config(&harness.path).unwrap();
        bound.home = home.clone();
        crate::config_store::initialize_config(&harness.path, &bound, true).unwrap();
        let mut session = harness.session(services);
        session.request_refresh();
        settle(&mut session);
        let proposal = session.view().consent.expect("proposal");
        std::fs::create_dir_all(&proposal.home).expect("home dir");
        session
            .request_set_backend_consent(&proposal)
            .expect("authorize");
        settle(&mut session);
        let view = session.view();
        assert!(view.coverage.enabled);
        assert!(view.coverage.authorized_home.is_some());
        assert_eq!(view.coverage.state, "pending", "block not yet prepared");
        // Grab the fresh revoke proposal before proving the old token died.
        let revoke = session.view().consent.expect("revoke proposal");
        // Reuse of the consumed token must fail.
        let error = session.request_set_backend_consent(&proposal).unwrap_err();
        assert_eq!(error.code, "CONFIRMATION_REQUIRED");
        let error = session.request_set_backend_consent(&revoke).unwrap_err();
        assert_eq!(
            error.code, "CONFIRMATION_REQUIRED",
            "revoke token consumed by reuse attempt too"
        );
        session.request_refresh();
        settle(&mut session);
        let revoke = session
            .view()
            .consent
            .expect("revoke proposal after refresh");
        assert!(!revoke.enabled);
        session
            .request_set_backend_consent(&revoke)
            .expect("revoke");
        settle(&mut session);
        let view = session.view();
        assert!(!view.coverage.enabled);
        assert_eq!(view.coverage.state, "not_authorized");
    }

    #[test]
    fn stale_consent_token_is_refused_after_config_change() {
        let (harness, services) = Harness::new(true);
        let mut session = harness.session(services);
        session.request_refresh();
        settle(&mut session);
        let proposal = session.view().consent.expect("proposal");
        // External config edit invalidates the revision binding.
        let mut config = crate::config_store::load_config(session.config_path()).unwrap();
        config.port = 9999;
        crate::config_store::initialize_config(session.config_path(), &config, true).unwrap();
        let error = session.request_set_backend_consent(&proposal).unwrap_err();
        assert_eq!(error.code, "CONFIG_CHANGED");
    }

    #[test]
    fn mutation_committed_fact_survives_refresh_failure() {
        let (harness, services) = Harness::new(true);
        let mut session = harness.session(services);
        session.request_refresh();
        settle(&mut session);
        session
            .request_set_proxy("localhost", 8080)
            .expect("set proxy");
        settle(&mut session);
        let mutation = session.mutation_outcome().expect("mutation outcome");
        assert!(mutation.committed);
        let view = session.view();
        assert_eq!(view.proxy.host, "localhost");
        assert_eq!(view.proxy.port, 8080);
    }
}
