#ifndef _CRT_RAND_S
#define _CRT_RAND_S
#endif
#include "session.h"
#include <cstdio>
#include <cstdlib>

namespace cpg {
namespace {
std::string randomToken()
{
    unsigned int values[4] = {};
    for (unsigned int &value : values)
        if (rand_s(&value) != 0)
            throw Error("SESSION_RANDOM_UNAVAILABLE", "Cannot generate a confirmation token.");
    char buffer[33] = {};
    std::snprintf(buffer, sizeof buffer, "%08x%08x%08x%08x", values[0], values[1], values[2], values[3]);
    return buffer;
}
template<typename T> Json nullable(const std::optional<T> &value)
{
    return value ? Json(*value) : Json(nullptr);
}
Json pathJson(const std::optional<std::filesystem::path> &value)
{
    return value ? Json(wideToUtf8(value->wstring())) : Json(nullptr);
}
std::string textField(const Json &value, const char *name)
{
    const auto item = value.find(name);
    return item != value.end() && item->is_string() ? item->get<std::string>() : std::string();
}
LaunchReceipt typedReceipt(const Json &value)
{
    LaunchReceipt result;
    const auto pid = value.find("pid");
    if (pid != value.end() && pid->is_number_integer()) result.pid = pid->get<std::uint32_t>();
    result.launchMethod = textField(value, "launch_method");
    result.activationState = textField(value, "activation_state");
    result.instance = textField(value, "instance");
    result.proxyDelivery = textField(value, "proxy_delivery");
    result.backendProxyConfig = textField(value, "backend_proxy_config");
    result.daemonPreparation = textField(value, "daemon_preparation");
    const auto endpoint = value.find("proxy_endpoint");
    if (endpoint != value.end() && endpoint->is_string()) result.proxyEndpoint = endpoint->get<std::string>();
    const auto elevationValue = value.find("target_elevation");
    if (elevationValue != value.end() && elevationValue->is_boolean()) result.targetElevation = elevationValue->get<bool>();
    return result;
}
Error taskError() { return Error("ENGINE_TASK_FAILED", "The engine task failed; refresh before retrying."); }
Error timeoutError() { return Error("SESSION_TIMEOUT", "The operation exceeded its session budget and has settled; known completion facts are retained. Refresh before another action."); }
}

Json SessionView::publicJson() const
{
    return Json{
        {"config_readiness", ready ? "ready" : "repair_required"},
        {"proxy", {{"url", proxy.url}, {"host", proxy.host}, {"port", proxy.port}}},
        {"launch", {{"method", launchMethod}}},
        {"desktop", {{"state", desktop.state}, {"product", desktop.product},
            {"display_name", desktop.displayName}, {"package_version", desktop.packageVersion},
            {"architecture", desktop.architecture}, {"application_id", nullable(desktop.applicationId)},
            {"manifest_executable", nullable(desktop.manifestExecutable)}, {"runtime_kind", nullable(desktop.runtimeKind)}}},
        {"process", {{"state", process.state}, {"pid", nullable(process.pid)}}},
        {"coverage", {{"state", coverage.state}, {"enabled", coverage.enabled},
            {"authorized_home", pathJson(coverage.authorizedHome)}, {"candidate_home", pathJson(coverage.candidateHome)}}},
        {"busy", busy}, {"elevation", elevation}, {"error", error ? error->json() : Json(nullptr)},
        {"actions", {{"can_launch", actions.canLaunch}, {"can_edit_proxy", actions.canEditProxy},
            {"can_authorize_backend_proxy", actions.canAuthorizeBackendProxy},
            {"can_revoke_backend_proxy", actions.canRevokeBackendProxy}, {"can_repair", actions.canRepair},
            {"can_refresh", actions.canRefresh}, {"can_cancel", actions.canCancel}, {"blocked_reason", actions.blockedReason}}},
        {"confirmations", {{"backend_proxy", consent ? Json{{"token", consent->token},
            {"enabled", consent->enabled}, {"home", wideToUtf8(consent->home.wstring())}, {"revision", consent->revision}} : Json(nullptr)},
            {"repair", repair ? Json{{"token", repair->token}, {"revision", repair->revision}} : Json(nullptr)}}}};
}

EngineSession::EngineSession(std::filesystem::path path,
    std::function<Desktop(const Config &, const Cancellation &)> discovery,
    std::function<std::string()> queryElevation, SessionBudgets budgets, SessionServices services)
    : path_(std::move(path)), discovery_(std::move(discovery)), queryElevation_(std::move(queryElevation)),
      budgets_(budgets), services_(std::move(services))
{
    if (path_.empty() || !path_.is_absolute())
        throw Error("CONFIG_PATH_INVALID", "The session requires a resolved absolute configuration path.");
    if (!services_.launch) services_.launch = launchPipeline;
}
EngineSession::~EngineSession() { cancel(); if (task_.valid()) task_.wait(); }
void EngineSession::cancel() { if (pending_) { cancellation_.cancel(); invalidate(); } }
void EngineSession::invalidate() { consent_.reset(); repair_.reset(); }

EngineSession::Result EngineSession::refresh(const Cancellation &cancel) const
{
    Result result;
    result.snapshot = true;
    try {
        cancel.check();
        result.config = Config::loadOrCreate(path_);
        result.ready = true;
        cancel.check();
        result.desktop = discovery_(result.config, cancel);
        result.process = services_.process(*result.desktop);
        result.coverage = result.desktop->registered ? inspectProxyEnv(result.config) : "not_applicable";
    } catch (const Error &error) { result.error = error; }
    catch (...) { result.error = taskError(); }
    return result;
}
EngineSession::Result EngineSession::committedRefresh(Config config, const Cancellation &cancel) const
{
    Result result = refresh(cancel);
    MutationOutcome mutation;
    mutation.committed = true;
    mutation.committedConfig = config;
    mutation.refreshError = result.error;
    // A committed write is an authoritative fact even when cancellation occurs
    // before the subsequent read, or a new reader cannot acquire the file.
    if (!result.ready) { result.config = std::move(config); result.ready = true; }
    result.mutation = std::move(mutation);
    return result;
}
void EngineSession::confirmations()
{
    invalidate();
    if (!state_.ready || pending_) return;
    confirmedConfig_ = state_.config;
    ++revision_;
    try {
        const auto home = services_.proposalHome(state_.config);
        if (!home.empty() && home.is_absolute())
            consent_ = ConsentProposal{randomToken(), !state_.config.manageBackend, home, revision_};
    } catch (const Error &) { }
    if (state_.desktop && state_.process.state == "stopped" && queryElevation_() == "not_elevated"
        && (!state_.desktop->registered || state_.config.manageBackend))
        repair_ = RepairProposal{randomToken(), revision_};
}
SessionView EngineSession::view() const
{
    SessionView result;
    result.ready = state_.ready;
    result.busy = pending_;
    result.proxy = {state_.config.proxyUrl(), state_.config.host, state_.config.port};
    result.elevation = queryElevation_();
    result.error = state_.error;
    result.process.state = state_.process.state;
    if (state_.process.pid) result.process.pid = state_.process.pid;
    if (state_.desktop) {
        const auto &desktop = *state_.desktop;
        result.desktop.state = "found";
        result.desktop.product = publicText(desktop.product);
        result.desktop.displayName = publicText(desktop.displayName);
        result.desktop.packageVersion = publicText(desktop.packageVersion);
        result.desktop.architecture = publicText(desktop.architecture);
        result.launchMethod = desktop.registered ? "appmodel_activation" : "native_process";
        if (desktop.registered) {
            result.desktop.applicationId = publicText(desktop.applicationId);
            result.desktop.manifestExecutable = publicText(desktop.manifestExecutable);
            result.desktop.runtimeKind = publicText(desktop.runtimeKind);
        }
    } else if (state_.error && (state_.error->code == "CODEX_NOT_INSTALLED" || state_.error->code == "APPX_NOT_FOUND")) {
        result.desktop.state = "not_found";
    }
    result.coverage.state = state_.coverage;
    result.coverage.enabled = state_.config.manageBackend;
    if (state_.config.manageBackend) result.coverage.authorizedHome = state_.config.home;
    if (consent_) result.coverage.candidateHome = consent_->home;
    result.consent = consent_;
    result.repair = repair_;
    auto &actions = result.actions;
    actions.canEditProxy = actions.canRefresh = !pending_;
    actions.canCancel = pending_;
    actions.canAuthorizeBackendProxy = !pending_ && consent_ && consent_->enabled;
    actions.canRevokeBackendProxy = !pending_ && consent_ && !consent_->enabled;
    if (pending_) actions.blockedReason = "Another foreground operation is active.";
    else if (!state_.ready) actions.blockedReason = "Repair the proxy configuration before launch.";
    else if (result.elevation != "not_elevated") actions.blockedReason = "Run Guard as a normal user; elevation must be verifiable.";
    else if (!state_.desktop) actions.blockedReason = state_.error ? state_.error->message : "Refresh to discover Desktop.";
    else if (state_.process.state == "running") actions.blockedReason = "Desktop is already running. Close it before another launch.";
    else if (state_.process.state != "stopped") actions.blockedReason = "Desktop process state is unknown; refresh before launching.";
    else actions.canLaunch = true;
    actions.canRepair = actions.canLaunch && repair_.has_value();
    if (!state_.ready && !result.error) result.error = Error("CONFIG_INVALID", actions.blockedReason);
    else if (result.elevation != "not_elevated") result.error = Error("ELEVATION_NOT_ALLOWED", "Run Guard as a normal user; elevation must be verifiable.");
    return result;
}
void EngineSession::dispatch(bool launch, std::function<Result()> effect)
{
    // Construct the task before committing busy state; async allocation failure
    // cannot leave the session permanently busy with no valid future.
    auto task = std::async(std::launch::async, [effect = std::move(effect)] {
        try { return effect(); }
        catch (const Error &error) { Result result; result.error = error; return result; }
        catch (...) { Result result; result.error = taskError(); return result; }
    });
    ++operation_;
    task_ = std::move(task);
    launching_ = launch;
    pending_ = true;
    foreground_ = ForegroundState::Running;
    mutationError_.reset();
    dispatchedAt_ = std::chrono::steady_clock::now();
}
void EngineSession::requestRefresh()
{
    if (pending_) return;
    invalidate();
    cancellation_ = Cancellation();
    dispatch(false, [this, cancel = cancellation_] { return refresh(cancel); });
}
void EngineSession::requestSetProxy(const std::string &host, int port)
{
    if (pending_) throw Error("SESSION_BUSY", "Another foreground operation is active.");
    const auto expected = state_.config;
    Config candidate = expected;
    candidate.host = host;
    candidate.port = port;
    candidate.validate();
    const bool invalid = !state_.ready;
    invalidate();
    cancellation_ = Cancellation();
    dispatch(false, [this, expected, invalid, host, port, cancel = cancellation_] {
        try {
            cancel.check();
            return committedRefresh(updateProxy(path_, expected, invalid, host, port), cancel);
        } catch (const Error &error) {
            Result result; result.mutation.emplace(); result.mutation->mutationError = error; return result;
        } catch (...) {
            Result result; result.mutation.emplace(); result.mutation->mutationError = taskError(); return result;
        }
    });
}
void EngineSession::requestSetBackendConsent(const ConsentProposal &proposal)
{
    if (pending_) throw Error("SESSION_BUSY", "Another foreground operation is active.");
    const bool authorized = state_.ready && consent_ && proposal == *consent_
        && state_.config == confirmedConfig_;
    // Consume the displayed proposal (including guessed submissions) before
    // any filesystem access can throw. A failed read/lease never makes the
    // same user confirmation reusable.
    invalidate();
    if (!authorized) throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this action again.");
    const auto expected = state_.config;
    if (!(Config::load(path_) == expected))
        throw Error("CONFIG_CHANGED", "Configuration changed; refresh and confirm again.");
    cancellation_ = Cancellation();
    dispatch(false, [this, expected, proposal, cancel = cancellation_] {
        try {
            cancel.check();
            return committedRefresh(updateConsent(path_, expected, proposal.enabled, proposal.home), cancel);
        } catch (const Error &error) {
            Result result; result.mutation.emplace(); result.mutation->mutationError = error; return result;
        } catch (...) {
            Result result; result.mutation.emplace(); result.mutation->mutationError = taskError(); return result;
        }
    });
}
void EngineSession::requestSetBackendConsent(bool enabled, const std::string &token)
{
    ConsentProposal proposal = consent_.value_or(ConsentProposal{});
    proposal.enabled = enabled;
    proposal.token = token;
    requestSetBackendConsent(proposal);
}
void EngineSession::requestLaunch(const RepairProposal &proposal)
{
    if (pending_) throw Error("SESSION_BUSY", "Another foreground operation is active.");
    if (!repair_ || proposal != *repair_) {
        invalidate(); throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this repair launch again.");
    }
    requestLaunch(true, proposal.token);
}
void EngineSession::requestLaunch(bool repair, const std::string &token)
{
    if (pending_) throw Error("SESSION_BUSY", "Another foreground operation is active.");
    const bool authorized = state_.ready && repair_ && token == repair_->token
        && state_.config == confirmedConfig_;
    // Authorization is single-use even if subsequent config reads, leases,
    // elevation queries, or dispatch setup fail.
    invalidate();
    if (repair && !authorized) throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this repair launch again.");
    if (!repair && !token.empty()) throw Error("SESSION_PARAMS_INVALID", "Normal launch does not accept a confirmation token.");
    if (!state_.ready) throw Error("CONFIG_INVALID", "Refresh and repair the configuration first.");
    const auto expected = state_.config;
    if (!(Config::load(path_) == expected))
        throw Error("CONFIG_CHANGED", "Configuration changed; refresh and confirm again.");
    if (queryElevation_() != "not_elevated") throw Error("ELEVATION_NOT_ALLOWED", "Launch requires verifiable normal-user elevation.");
    cancellation_ = Cancellation();
    dispatch(true, [this, expected, repair, cancel = cancellation_] {
        Result result;
        LaunchOutcome outcome;
        LaunchFacts facts;
        try {
            result.value = services_.launch(expected, path_, {repair, false}, cancel, &facts);
            outcome.receipt = typedReceipt(result.value);
        } catch (const Error &error) { outcome.error = error; }
        catch (...) { outcome.error = taskError(); }
        outcome.activationState = facts.activationState;
        outcome.daemonPreparation = facts.daemonPreparation;
        outcome.backendProxyConfig = facts.backendProxyConfig;
        result.launch = std::move(outcome);
        return result;
    });
}
EngineSession::Event EngineSession::poll()
{
    if (!pending_) return Event::None;
    if (task_.wait_for(std::chrono::milliseconds(0)) != std::future_status::ready) {
        const auto budget = launching_ ? budgets_.launch : budgets_.mutation;
        if (std::chrono::steady_clock::now() - dispatchedAt_ > budget && foreground_ == ForegroundState::Running) {
            cancellation_.cancel(); invalidate(); foreground_ = ForegroundState::CancellingAfterTimeout;
        }
        return Event::None;
    }
    auto result = task_.get();
    const bool exceeded = foreground_ == ForegroundState::CancellingAfterTimeout;
    pending_ = false;
    launching_ = false;
    foreground_ = ForegroundState::Idle;
    if (result.launch) {
        result.launch->operationId = operation_;
        result.launch->exceededBudget = exceeded;
        launchOutcome_ = std::move(result.launch);
        operationResult_ = std::move(result.value);
        invalidate();
        state_.process = {};
        // The operation result remains separate from the next read-only view.
        return Event::OperationFinished;
    }
    if (result.mutation) {
        result.mutation->operationId = operation_;
        result.mutation->exceededBudget = exceeded;
        mutationOutcome_ = std::move(result.mutation);
        if (mutationOutcome_->committed) {
            state_ = std::move(result);
            confirmations();
        } else {
            mutationError_ = mutationOutcome_->mutationError;
            invalidate();
        }
        return Event::MutationFinished;
    }
    if (result.snapshot) state_ = std::move(result);
    else if (result.error) state_.error = result.error;
    if (exceeded) {
        invalidate();
        mutationError_ = timeoutError();
        return Event::MutationFailed;
    }
    confirmations();
    return Event::SnapshotArrived;
}
} // namespace cpg
