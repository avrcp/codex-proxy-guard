//! Thin GUI adapter over the same reducer/capability/dispatcher boundary as
//! the TUI. The GUI never supplies a Home, executable, or command line.

use std::{
    io::{self, BufReader, Write},
    path::PathBuf,
    time::Duration,
};

use proxy_guard_core::{
    AppAction, AppEffect, AppState, BackendProxyRuntimeState, Capabilities, ConfigReadiness,
    DesktopAppDiscovery, DesktopProcessState, DesktopTargetKind, GuardConfig, ProxyField,
    TaskResult, UserIntent, redact_text, reduce,
};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;

use crate::{
    bridge_protocol::{self as protocol, BridgeError, Method, Request},
    dispatcher::EffectDispatcher,
};

const REQUEST_BUDGET: Duration = Duration::from_secs(60);
// Includes the public daemon-stop's 720 second budget plus discovery,
// activation, and helper cleanup. A watchdog failure closes the session;
// an uncertain operation must never be retried automatically.
const LAUNCH_BUDGET: Duration = Duration::from_secs(840);
const WRITE_BUDGET: Duration = Duration::from_secs(2);

type Input = io::Result<Option<Vec<u8>>>;
struct Output {
    bytes: Vec<u8>,
    done: oneshot::Sender<io::Result<()>>,
}

pub async fn run(config_path: PathBuf) -> anyhow::Result<()> {
    let (input_tx, input_rx) = mpsc::channel(4);
    // Tokio's stdin uses an uncancellable blocking task, which can keep its
    // runtime alive after an explicit shutdown while the parent holds stdin.
    // Dedicated threads instead die with this process; bounded channels apply
    // backpressure and neither thread touches business state.
    std::thread::spawn(move || {
        let mut reader = BufReader::new(io::stdin().lock());
        loop {
            let frame = protocol::read_frame(&mut reader);
            let terminal = !matches!(&frame, Ok(Some(_)));
            if input_tx.blocking_send(frame).is_err() || terminal {
                break;
            }
        }
    });
    let (output_tx, mut output_rx) = mpsc::channel::<Output>(1);
    std::thread::spawn(move || {
        let mut writer = io::stdout().lock();
        while let Some(output) = output_rx.blocking_recv() {
            let result = writer
                .write_all(&output.bytes)
                .and_then(|()| writer.flush());
            let failed = result.is_err();
            let _ = output.done.send(result);
            if failed {
                break;
            }
        }
    });
    let (task_tx, task_rx) = mpsc::channel(4);
    let cancellation = CancellationToken::new();
    let dispatcher = EffectDispatcher::new(task_tx, cancellation.clone());
    let mut session = Session::new(crate::tui_state(&config_path), dispatcher);
    let result = serve(&mut session, input_rx, task_rx, &output_tx).await;
    cancellation.cancel();
    session.dispatcher.shutdown().await;
    result
}

async fn write(output: &mpsc::Sender<Output>, value: Value) -> anyhow::Result<()> {
    let bytes = protocol::encode(&value)?;
    let (done, ack) = oneshot::channel();
    timeout(WRITE_BUDGET, async {
        output
            .send(Output { bytes, done })
            .await
            .map_err(|_| io::Error::other("bridge writer closed"))?;
        ack.await
            .map_err(|_| io::Error::other("bridge writer closed"))?
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "bridge output stalled"))??;
    Ok(())
}

async fn serve(
    session: &mut Session,
    mut input: mpsc::Receiver<Input>,
    mut tasks: mpsc::Receiver<TaskResult>,
    output: &mpsc::Sender<Output>,
) -> anyhow::Result<()> {
    let mut health_tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        let deadline = session
            .pending
            .as_ref()
            .map_or(Instant::now() + LAUNCH_BUDGET, |p| p.deadline);
        tokio::select! {
            frame = input.recv() => {
                let frame = match frame {
                    Some(Ok(Some(frame))) => frame,
                    Some(Err(error)) => {
                        let code = if error.kind() == io::ErrorKind::InvalidData { "BRIDGE_REQUEST_TOO_LARGE" } else { "BRIDGE_IO_FAILED" };
                        write(output, protocol::failure(None, BridgeError::new(code, "Bridge input was invalid; the session is closing."))).await?;
                        return Ok(());
                    }
                    Some(Ok(None)) | None => return Ok(()),
                };
                match protocol::decode(&frame) {
                    Ok(request) => {
                        let id = request.id;
                        match session.request(request) {
                            Ok(messages) => for message in messages { write(output, message).await?; },
                            Err(error) => write(output, protocol::failure(Some(id), error)).await?,
                        }
                        if session.closed { return Ok(()); }
                    }
                    Err((id, error)) => write(output, protocol::failure(id, error)).await?,
                }
            }
            Some(result) = tasks.recv() => {
                for message in session.complete(result) { write(output, message).await?; }
                if session.closed { return Ok(()); }
            }
            _ = health_tick.tick(), if session.pending.is_some() => {
                // Check completion before channel emptiness: the finished
                // handle guarantees a legitimate result was already sent.
                if session.dispatcher.foreground_finished().await && tasks.is_empty() {
                    if let Some(message) = session.fail_pending("ENGINE_TASK_FAILED", "The engine task ended without a result. Its outcome is unknown; check Desktop before retrying.") {
                        write(output, message).await?;
                    }
                    return Ok(());
                }
            }
            _ = tokio::time::sleep_until(deadline), if session.pending.is_some() => {
                session.dispatcher.cancel_foreground();
                if let Some(message) = session.fail_pending("OPERATION_OUTCOME_UNKNOWN", "The operation exceeded its deadline. Check Desktop and refresh before retrying.") {
                    write(output, message).await?;
                }
                return Ok(());
            }
        }
    }
}

enum ResponseKind {
    Snapshot,
    Proxy,
    Consent,
}
enum PendingKind {
    Response { id: u64, kind: ResponseKind },
    Launch { operation_id: u64 },
}
struct Pending {
    kind: PendingKind,
    deadline: Instant,
}

#[derive(Clone)]
struct Confirmation {
    token: String,
    config: GuardConfig,
    home: Option<PathBuf>,
}

struct Session {
    state: AppState,
    dispatcher: EffectDispatcher,
    capabilities: Capabilities,
    hello: bool,
    closed: bool,
    last_id: u64,
    next_operation: u64,
    token_sequence: u64,
    token_prefix: String,
    consent: Option<Confirmation>,
    repair: Option<Confirmation>,
    pending: Option<Pending>,
}

impl Session {
    fn fail_pending(&mut self, code: &str, message: &str) -> Option<Value> {
        let error = BridgeError::new(code, message);
        self.pending.take().map(|pending| match pending.kind {
            PendingKind::Launch { operation_id } => operation_failure(operation_id, error),
            PendingKind::Response { id, .. } => protocol::failure(Some(id), error),
        })
    }

    fn new(state: AppState, dispatcher: EffectDispatcher) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            state,
            dispatcher,
            capabilities: Capabilities::default(),
            hello: false,
            closed: false,
            last_id: 0,
            next_operation: 0,
            token_sequence: 0,
            token_prefix: format!("{}-{nonce:x}", std::process::id()),
            consent: None,
            repair: None,
            pending: None,
        }
    }

    fn invalidate(&mut self) {
        self.consent = None;
        self.repair = None;
    }

    fn action(&mut self, intent: UserIntent) -> Result<bool, BridgeError> {
        let mut candidate = self.state.clone();
        let effects = reduce(&mut candidate, AppAction::Intent(intent));
        self.commit(candidate, effects)
    }

    fn commit(
        &mut self,
        candidate: AppState,
        effects: Vec<AppEffect>,
    ) -> Result<bool, BridgeError> {
        for effect in &effects {
            self.capabilities.authorize(effect).map_err(|_| {
                BridgeError::new("CAPABILITY_DENIED", "This operation is unavailable.")
            })?;
        }
        self.state = candidate;
        let dispatched = !effects.is_empty();
        for effect in effects {
            self.dispatcher.dispatch(effect, &self.state);
        }
        Ok(dispatched)
    }

    fn idle(&self) -> Result<(), BridgeError> {
        if self.pending.is_some() || self.state.foreground.is_some() {
            return Err(BridgeError::new(
                "BRIDGE_BUSY",
                "Another foreground operation is active.",
            ));
        }
        Ok(())
    }

    fn unchanged(&mut self) -> Result<(), BridgeError> {
        let disk = GuardConfig::load(&self.state.config_path);
        let unchanged = match self.state.config_readiness {
            ConfigReadiness::Ready => disk.is_ok_and(|config| config == self.state.config),
            ConfigReadiness::RepairRequired => disk.is_err(),
        };
        if !unchanged {
            self.invalidate();
            return Err(BridgeError::new(
                "CONFIG_CHANGED",
                "Configuration changed. Refresh and confirm again.",
            ));
        }
        Ok(())
    }

    fn response_pending(&mut self, id: u64, kind: ResponseKind) {
        self.pending = Some(Pending {
            kind: PendingKind::Response { id, kind },
            deadline: Instant::now() + REQUEST_BUDGET,
        });
    }

    fn request(&mut self, request: Request) -> Result<Vec<Value>, BridgeError> {
        let id = request.id;
        if id <= self.last_id {
            return Err(BridgeError::new(
                "BRIDGE_ID_REUSED",
                "Request ids must strictly increase within a session.",
            ));
        }
        self.last_id = id;
        if matches!(request.method, Method::Shutdown) {
            self.invalidate();
            self.action(UserIntent::Quit)?;
            self.closed = true;
            return Ok(vec![protocol::success(id, json!({"shutting_down": true}))]);
        }
        if matches!(request.method, Method::Hello) {
            self.hello = true;
            return Ok(vec![protocol::success(
                id,
                json!({
                    "protocol_version": protocol::SCHEMA, "engine_version": crate::build_info::VERSION,
                    "engine_commit": crate::build_info::COMMIT,
                    "capabilities": ["snapshot", "set_proxy", "backend_proxy_consent", "launch", "repair", "cancel"]
                }),
            )]);
        }
        if !self.hello {
            return Err(BridgeError::new(
                "BRIDGE_HELLO_REQUIRED",
                "Send hello before business requests.",
            ));
        }
        match request.method {
            Method::Snapshot => {
                if self.pending.is_some() {
                    return Ok(vec![protocol::success(id, self.snapshot())]);
                }
                self.invalidate();
                self.state = crate::tui_state(&self.state.config_path);
                self.action(UserIntent::Dismiss)?;
                self.action(UserIntent::Refresh)?;
                self.response_pending(id, ResponseKind::Snapshot);
                Ok(vec![])
            }
            Method::SetProxy { host, port } => {
                self.idle()?;
                self.unchanged()?;
                let mut candidate = self.state.config.clone();
                candidate.proxy.host = host.clone();
                candidate.proxy.port = port;
                candidate.validate().map_err(|_| {
                    BridgeError::new(
                        "CONFIG_INVALID",
                        "Use a loopback HTTP/Mixed proxy host and a port between 1 and 65535.",
                    )
                })?;
                self.invalidate();
                self.action(UserIntent::EditProxy)?;
                self.action(UserIntent::UpdateProxyField {
                    field: ProxyField::Host,
                    value: host,
                })?;
                self.action(UserIntent::UpdateProxyField {
                    field: ProxyField::Port,
                    value: port.to_string(),
                })?;
                if !self.action(UserIntent::SaveProxy)? {
                    return Err(BridgeError::new(
                        "CONFIG_INVALID",
                        "Proxy configuration could not be saved.",
                    ));
                }
                self.response_pending(id, ResponseKind::Proxy);
                Ok(vec![])
            }
            Method::Consent {
                enabled,
                confirmation_token,
            } => {
                self.idle()?;
                self.unchanged()?;
                let confirmation = self.consent.take();
                self.invalidate();
                let confirmation = confirmation
                    .filter(|c| {
                        c.token == confirmation_token
                            && c.config == self.state.config
                            && enabled != self.state.config.codex.manage_codex_proxy_env
                    })
                    .ok_or_else(confirmation_required)?;
                let (candidate, effects) = consent_candidate(&self.state);
                if !effects.iter().any(|effect| matches!(effect, AppEffect::UpdateBackendProxyConsent { enable, home } if *enable == enabled && Some(home) == confirmation.home.as_ref())) { return Err(confirmation_required()); }
                self.commit(candidate, effects)?;
                self.response_pending(id, ResponseKind::Consent);
                Ok(vec![])
            }
            Method::Launch {
                repair,
                confirmation_token,
            } => {
                self.idle()?;
                self.unchanged()?;
                if self.state.config_readiness != ConfigReadiness::Ready {
                    return Err(BridgeError::new(
                        "CONFIG_INVALID",
                        "Repair the proxy configuration before launch.",
                    ));
                }
                if let Some(error) = elevation_error(elevation()) {
                    return Err(error);
                }
                if repair {
                    let confirmation = self.repair.take();
                    self.invalidate();
                    if !confirmation.is_some_and(|c| {
                        Some(c.token) == confirmation_token && c.config == self.state.config
                    }) {
                        return Err(confirmation_required());
                    }
                    self.action(UserIntent::Dismiss)?;
                    self.action(UserIntent::RequestDaemonRepairLaunch)?;
                    if !self.action(UserIntent::ConfirmDaemonRepairLaunch)? {
                        return Err(BridgeError::engine(&self.state.status_message));
                    }
                } else {
                    if confirmation_token.is_some() {
                        return Err(BridgeError::new(
                            "BRIDGE_PARAMS_INVALID",
                            "Normal launch does not accept a confirmation token.",
                        ));
                    }
                    self.invalidate();
                    self.action(UserIntent::Dismiss)?;
                    if !self.action(UserIntent::Launch)? {
                        return Err(BridgeError::engine(&self.state.status_message));
                    }
                }
                self.next_operation += 1;
                let operation_id = self.next_operation;
                self.pending = Some(Pending {
                    kind: PendingKind::Launch { operation_id },
                    deadline: Instant::now() + LAUNCH_BUDGET,
                });
                Ok(vec![
                    protocol::success(id, json!({"operation_id": operation_id})),
                    json!({"schema": 1, "event": "operation_state", "operation_id": operation_id, "state": "running", "message": if repair { "Preparing the proxy configuration and repair launch…" } else { "Launching Desktop…" }}),
                ])
            }
            Method::Cancel { operation_id } => {
                if !matches!(self.pending.as_ref().map(|p| &p.kind), Some(PendingKind::Launch { operation_id: current }) if *current == operation_id)
                {
                    return Err(BridgeError::new(
                        "OPERATION_NOT_FOUND",
                        "No matching launch operation is active.",
                    ));
                }
                self.dispatcher.cancel_foreground();
                Ok(vec![
                    protocol::success(id, json!({"cancellation_requested": true})),
                    json!({"schema": 1, "event": "operation_state", "operation_id": operation_id, "state": "cancelling", "message": "Cancellation requested; waiting for the engine outcome…"}),
                ])
            }
            Method::Hello | Method::Shutdown => unreachable!(),
        }
    }

    fn complete(&mut self, result: TaskResult) -> Vec<Value> {
        let Some(pending) = self.pending.take() else {
            return vec![];
        };
        let outcome: Result<Value, BridgeError> = match (&pending.kind, &result) {
            (PendingKind::Launch { .. }, TaskResult::LaunchCompleted(result)) => result
                .as_ref()
                .map(|(_, receipt)| {
                    let mut value = serde_json::to_value(receipt).expect("receipt is serializable");
                    for key in ["package_name", "package_version", "architecture"] {
                        if let Some(text) = value["desktop"][key].as_str() {
                            value["desktop"][key] = json!(public_identifier(text));
                        }
                    }
                    value
                })
                .map_err(|e| BridgeError::engine(e)),
            (
                PendingKind::Response {
                    kind: ResponseKind::Snapshot,
                    ..
                },
                TaskResult::LocalStateRefreshed { .. },
            ) => Ok(Value::Null),
            (
                PendingKind::Response {
                    kind: ResponseKind::Proxy,
                    ..
                },
                TaskResult::ConfigSaved(result),
            )
            | (
                PendingKind::Response {
                    kind: ResponseKind::Consent,
                    ..
                },
                TaskResult::BackendProxyConsentUpdated(result),
            ) => result
                .as_ref()
                .map(|_| Value::Null)
                .map_err(|e| BridgeError::engine(e)),
            _ => {
                self.pending = Some(pending);
                return vec![];
            }
        };
        let mut candidate = self.state.clone();
        let effects = reduce(&mut candidate, AppAction::TaskComplete(Box::new(result)));
        if let Err(error) = self.commit(candidate, effects) {
            self.closed = true;
            return vec![protocol::failure(None, error)];
        }
        if outcome.is_err() && self.state.proxy_editor.is_some() {
            let _ = self.action(UserIntent::CancelProxyEdit);
        }
        // A mutation changes intent/configuration, not the observed file
        // state. Reuse the normal refresh effect before returning the result
        // so an existing current block is never mislabeled Pending/Stale.
        if outcome.is_ok()
            && let PendingKind::Response {
                id,
                kind: ResponseKind::Proxy | ResponseKind::Consent,
            } = pending.kind
        {
            let refresh = self
                .action(UserIntent::Dismiss)
                .and_then(|_| self.action(UserIntent::Refresh));
            match refresh {
                Ok(true) => {
                    self.pending = Some(Pending {
                        kind: PendingKind::Response {
                            id,
                            kind: ResponseKind::Snapshot,
                        },
                        deadline: pending.deadline,
                    });
                    return vec![];
                }
                _ => {
                    self.closed = true;
                    return vec![protocol::failure(
                        Some(id),
                        BridgeError::new(
                            "ENGINE_TASK_FAILED",
                            "Configuration was saved, but the state could not be refreshed.",
                        ),
                    )];
                }
            }
        }
        self.issue_confirmations();
        match pending.kind {
            PendingKind::Launch { operation_id } => vec![match outcome {
                Ok(result) => {
                    json!({"schema": 1, "event": "operation_finished", "operation_id": operation_id, "ok": true, "result": result})
                }
                Err(error) => operation_failure(operation_id, error),
            }],
            PendingKind::Response { id, .. } => vec![match outcome {
                Ok(_) => protocol::success(id, self.snapshot()),
                Err(error) => protocol::failure(Some(id), error),
            }],
        }
    }

    fn confirmation(&mut self, home: Option<PathBuf>) -> Confirmation {
        self.token_sequence += 1;
        Confirmation {
            token: format!("{}-{}", self.token_prefix, self.token_sequence),
            config: self.state.config.clone(),
            home,
        }
    }

    fn issue_confirmations(&mut self) {
        self.invalidate();
        if self.state.config_readiness != ConfigReadiness::Ready || self.pending.is_some() {
            return;
        }
        let (_, effects) = consent_candidate(&self.state);
        if let Some(home) = effects.into_iter().find_map(|effect| match effect {
            AppEffect::UpdateBackendProxyConsent { home, .. } => Some(home),
            _ => None,
        }) {
            self.consent = Some(self.confirmation(Some(home)));
        }
        let mut candidate = self.state.clone();
        reduce(&mut candidate, AppAction::Intent(UserIntent::Dismiss));
        reduce(
            &mut candidate,
            AppAction::Intent(UserIntent::RequestDaemonRepairLaunch),
        );
        if candidate.daemon_repair_prompt && elevation() == "not_elevated" {
            self.repair = Some(self.confirmation(None));
        }
    }

    fn snapshot(&self) -> Value {
        let ready = self.state.config_readiness == ConfigReadiness::Ready;
        let busy = self.pending.is_some() || self.state.foreground.is_some();
        let elevation = elevation();
        let (desktop, method, found) = match &self.state.desktop_app {
            DesktopAppDiscovery::Found(info) => {
                let (method, application_id, executable, runtime_kind) = match &info.target_kind {
                    DesktopTargetKind::RegisteredPackage(package) => (
                        "appmodel_activation",
                        Some(public_identifier(&package.application_id)),
                        safe_relative_executable(&package.manifest_executable),
                        serde_json::to_value(package.runtime_kind).unwrap_or(Value::Null),
                    ),
                    DesktopTargetKind::UnpackagedExecutable => {
                        ("native_process", None, None, Value::Null)
                    }
                };
                (
                    json!({"state":"found", "product":info.product, "display_name":info.product.display_name(), "package_version":public_identifier(&info.package_version), "architecture":public_identifier(&info.architecture), "application_id":application_id, "manifest_executable":executable, "runtime_kind":runtime_kind}),
                    method,
                    true,
                )
            }
            DesktopAppDiscovery::Unknown => (json!({"state":"unknown"}), "unknown", false),
            DesktopAppDiscovery::Searching => (json!({"state":"searching"}), "unknown", false),
            DesktopAppDiscovery::NotFound(_) => (json!({"state":"not_found"}), "unknown", false),
        };
        let (process, pid) = match self.state.desktop_process {
            DesktopProcessState::Unknown => ("unknown", None),
            DesktopProcessState::Stopped => ("stopped", None),
            DesktopProcessState::Running { pid } => ("running", Some(pid)),
        };
        let coverage = match self.state.backend_proxy_state {
            BackendProxyRuntimeState::Unknown => "unknown",
            BackendProxyRuntimeState::NotApplicable => "not_applicable",
            BackendProxyRuntimeState::NotAuthorized => "not_authorized",
            BackendProxyRuntimeState::Pending => "pending",
            BackendProxyRuntimeState::Current => "current",
            BackendProxyRuntimeState::Stale => "stale",
            BackendProxyRuntimeState::Conflict { .. } => "conflict",
            BackendProxyRuntimeState::Invalid => "invalid",
            BackendProxyRuntimeState::Unavailable => "unavailable",
        };
        let enabled = self.state.config.codex.manage_codex_proxy_env;
        let can_launch =
            ready && !busy && found && process == "stopped" && elevation == "not_elevated";
        let error = if !ready {
            Some(BridgeError::new(
                "CONFIG_INVALID",
                "Repair the proxy configuration before launch.",
            ))
        } else if let Some(error) = elevation_error(elevation) {
            Some(error)
        } else {
            self.state
                .error_message
                .as_deref()
                .map(BridgeError::engine)
                .or_else(|| match &self.state.desktop_app {
                    DesktopAppDiscovery::NotFound(message) => Some(BridgeError::engine(message)),
                    _ => None,
                })
        };
        let candidate_home = self.consent.as_ref().and_then(|c| c.home.as_ref());
        json!({
            "config_readiness": if ready { "ready" } else { "repair_required" },
            "proxy": { "url": self.state.config.proxy_url(), "host": self.state.config.proxy.host, "port":self.state.config.proxy.port },
            "launch":{"method":method}, "desktop":desktop, "process":{"state":process,"pid":pid},
            "coverage":{"state":coverage,"enabled":enabled,"authorized_home":if enabled { Some(&self.state.config.codex.proxy_env_home) } else { None },"candidate_home":candidate_home},
            "busy":busy, "elevation":elevation,"error":error,
            "actions":{"can_launch":can_launch,"can_edit_proxy":!busy,"can_authorize_backend_proxy":!busy && !enabled && self.consent.is_some(),"can_revoke_backend_proxy":!busy && enabled && self.consent.is_some(),"can_repair":can_launch && self.repair.is_some(),"can_refresh":!busy,"can_cancel":matches!(self.pending.as_ref().map(|p| &p.kind),Some(PendingKind::Launch { .. }))},
            "confirmations":{"backend_proxy":self.consent.as_ref().map(|c|json!({"token":c.token,"enabled":!enabled,"home":c.home})),"repair":self.repair.as_ref().map(|c|json!({"token":c.token}))}
        })
    }
}

fn consent_candidate(state: &AppState) -> (AppState, Vec<AppEffect>) {
    let mut candidate = state.clone();
    reduce(&mut candidate, AppAction::Intent(UserIntent::Dismiss));
    reduce(
        &mut candidate,
        AppAction::Intent(UserIntent::RequestBackendProxyConsent),
    );
    let effects = reduce(
        &mut candidate,
        AppAction::Intent(UserIntent::ConfirmBackendProxyConsent),
    );
    (candidate, effects)
}

fn elevation() -> &'static str {
    match proxy_guard_windows::query_elevation() {
        Ok(proxy_guard_windows::ElevationState::NotElevated) => "not_elevated",
        Ok(proxy_guard_windows::ElevationState::Elevated) => "elevated",
        Err(_) => "unknown",
    }
}

fn elevation_error(state: &str) -> Option<BridgeError> {
    match state {
        "not_elevated" => None,
        "elevated" => Some(BridgeError::new(
            "ELEVATED_LAUNCH_UNSUPPORTED",
            "Restart Guard without administrator privileges.",
        )),
        _ => Some(BridgeError::new(
            "ELEVATION_QUERY_FAILED",
            "Guard could not verify its elevation; launch is blocked.",
        )),
    }
}

fn confirmation_required() -> BridgeError {
    BridgeError::new(
        "CONFIRMATION_REQUIRED",
        "Refresh and explicitly confirm the displayed scope again.",
    )
}

fn operation_failure(operation_id: u64, error: BridgeError) -> Value {
    json!({"schema": 1, "event": "operation_finished", "operation_id":operation_id, "ok":false, "error":error})
}

fn public_text(text: &str) -> String {
    redact_text(text)
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}

fn public_identifier(text: &str) -> String {
    if text.is_empty()
        || text.len() > 256
        || !text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return "unknown".into();
    }
    text.into()
}

fn safe_relative_executable(text: &str) -> Option<String> {
    if text.contains(':')
        || text.starts_with(['/', '\\'])
        || text.split(['/', '\\']).any(|part| part == "..")
    {
        return None;
    }
    Some(public_text(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let (tx, _rx) = mpsc::channel(4);
        Session::new(
            AppState::new(GuardConfig::default(), "unused.toml".into()),
            EffectDispatcher::new(tx, CancellationToken::new()),
        )
    }

    #[test]
    fn hello_shutdown_and_monotonic_ids() {
        let mut session = session();
        assert_eq!(
            session
                .request(Request {
                    id: 1,
                    method: Method::Hello
                })
                .unwrap()[0]["result"]["protocol_version"],
            1
        );
        assert_eq!(
            session
                .request(Request {
                    id: 1,
                    method: Method::Hello
                })
                .unwrap_err()
                .code,
            "BRIDGE_ID_REUSED"
        );
        assert_eq!(
            session
                .request(Request {
                    id: 2,
                    method: Method::Shutdown
                })
                .unwrap()[0]["ok"],
            true
        );
        assert!(session.closed);
    }

    #[test]
    fn invalid_configuration_stays_editable_and_cannot_get_confirmations() {
        let mut session = session();
        session.state.config_readiness = ConfigReadiness::RepairRequired;
        session.issue_confirmations();
        let snapshot = session.snapshot();
        assert_eq!(snapshot["config_readiness"], "repair_required");
        assert_eq!(snapshot["actions"]["can_edit_proxy"], true);
        assert_eq!(snapshot["actions"]["can_launch"], false);
        assert!(snapshot["confirmations"]["backend_proxy"].is_null());
        assert!(snapshot["confirmations"]["repair"].is_null());
    }

    #[test]
    fn foreground_gate_and_cancel_id_are_enforced() {
        let mut session = session();
        session.hello = true;
        session.pending = Some(Pending {
            kind: PendingKind::Launch { operation_id: 5 },
            deadline: Instant::now() + LAUNCH_BUDGET,
        });
        assert_eq!(session.idle().unwrap_err().code, "BRIDGE_BUSY");
        assert_eq!(
            session
                .request(Request {
                    id: 1,
                    method: Method::Cancel { operation_id: 4 }
                })
                .unwrap_err()
                .code,
            "OPERATION_NOT_FOUND"
        );
        assert_eq!(
            session
                .request(Request {
                    id: 2,
                    method: Method::Cancel { operation_id: 5 }
                })
                .unwrap()[0]["ok"],
            true
        );
        assert!(
            session.pending.is_some(),
            "Cancellation must await a real terminal result"
        );
        assert!(session.snapshot()["confirmations"]["backend_proxy"].is_null());
    }

    #[test]
    fn consent_scope_is_derived_by_the_reducer_and_tokens_rotate() {
        let mut session = session();
        session.issue_confirmations();
        let first = session.consent.clone().unwrap();
        assert_eq!(first.home, GuardConfig::default_codex_home());
        assert_eq!(first.config, session.state.config);
        let (_, effects) = consent_candidate(&session.state);
        assert!(
            matches!(&effects[0],AppEffect::UpdateBackendProxyConsent { enable:true, home } if Some(home) == first.home.as_ref())
        );
        session.issue_confirmations();
        assert_ne!(first.token, session.consent.as_ref().unwrap().token);
        session.invalidate();
        assert!(session.consent.is_none());
    }

    #[test]
    fn snapshot_omits_installation_and_arbitrary_paths() {
        let session = session();
        let wire = session.snapshot().to_string();
        assert!(!wire.contains("config_path"));
        assert!(!wire.contains("install_location"));
        assert!(safe_relative_executable("C:\\WindowsApps\\app.exe").is_none());
        assert!(safe_relative_executable("../secret").is_none());
        assert_eq!(
            safe_relative_executable("app/ChatGPT.exe"),
            Some("app/ChatGPT.exe".into())
        );
    }

    #[test]
    fn elevation_errors_distinguish_query_failure_from_elevation() {
        assert!(elevation_error("not_elevated").is_none());
        assert_eq!(
            elevation_error("elevated").unwrap().code,
            "ELEVATED_LAUNCH_UNSUPPORTED"
        );
        assert_eq!(
            elevation_error("unknown").unwrap().code,
            "ELEVATION_QUERY_FAILED"
        );
    }

    #[tokio::test]
    async fn consent_result_waits_for_real_refresh_and_preserves_current_observation() {
        // A deterministic task-result test. This current-thread runtime never
        // yields to the queued discovery task; results below represent the
        // dispatcher responses without touching any real Home or application.
        let mut session = session();
        session.state.config.codex.manage_codex_proxy_env = true;
        session.state.config.codex.proxy_env_home = PathBuf::from(r"C:\bridge-test-home");
        session.state.foreground =
            Some(proxy_guard_core::ForegroundOperation::UpdateBackendProxyConsent);
        session.response_pending(2, ResponseKind::Consent);
        let config = session.state.config.clone();
        assert!(
            session
                .complete(TaskResult::BackendProxyConsentUpdated(Ok(config)))
                .is_empty()
        );
        assert_eq!(
            session.state.foreground,
            Some(proxy_guard_core::ForegroundOperation::Refresh)
        );
        assert!(
            session.consent.is_none(),
            "no new confirmation before the fresh observation"
        );
        let messages = session.complete(TaskResult::LocalStateRefreshed {
            desktop_app: Err("CODEX_NOT_INSTALLED: test fixture".into()),
            process: DesktopProcessState::Unknown,
            backend_proxy: BackendProxyRuntimeState::Current,
        });
        assert_eq!(messages[0]["id"], 2);
        assert_eq!(
            messages[0]["result"]["coverage"]["state"], "current",
            "fresh observation supersedes the provisional Pending state"
        );
        if elevation() == "not_elevated" {
            assert_eq!(
                messages[0]["result"]["error"]["code"],
                "CODEX_NOT_INSTALLED"
            );
        }
    }
}
