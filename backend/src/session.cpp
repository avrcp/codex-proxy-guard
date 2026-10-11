#ifndef _CRT_RAND_S
#define _CRT_RAND_S
#endif
#include "session.h"
#include "cpg/build_info.h"

#include <cstdio>
#include <cstdlib>

namespace cpg {
namespace {

std::string randomToken()
{
    // 128 bits from the CSPRNG-backed rand_s, formatted as a UUID-shaped hex
    // token. Single-use confirmation nonce; not a security boundary by itself.
    unsigned int values[4] = {};
    for (unsigned int &value : values) {
        if (rand_s(&value) != 0)
            throw Error("SESSION_RANDOM_UNAVAILABLE", "Cannot generate a confirmation token.");
    }
    char buffer[33] = {};
    std::snprintf(buffer, sizeof buffer, "%08x%08x%08x%08x",
                  values[0], values[1], values[2], values[3]);
    return buffer;
}

const Json &at(const Json &object, const char *key)
{
    static const Json empty = Json::object();
    const auto entry = object.find(key);
    return entry == object.end() ? empty : *entry;
}

} // namespace

EngineSession::EngineSession(std::filesystem::path path,
    std::function<Desktop(const Config &, const Cancellation &)> discovery,
    std::function<std::string()> queryElevation)
    : path_(std::move(path)), discovery_(std::move(discovery)),
      queryElevation_(std::move(queryElevation)) {}

EngineSession::~EngineSession()
{
    cancel();
    if (task_.valid()) task_.wait();
}

void EngineSession::cancel()
{
    cancellation_.cancel();
    invalidate();
}

void EngineSession::invalidate()
{
    consentToken_.clear();
    repairToken_.clear();
    confirmedHome_.clear();
}

EngineSession::Result EngineSession::refresh(const Cancellation &cancel) const
{
    Result result;
    result.snapshot = true;
    try { result.config = Config::loadOrCreate(path_); result.ready = true; }
    catch (const Error &e) { result.error = e; }
    cancel.check();
    if (result.ready) {
        try {
            result.desktop = discovery_(result.config, cancel);
            result.process = desktopProcessState(*result.desktop);
            result.coverage = result.desktop->registered ? inspectProxyEnv(result.config) : "not_applicable";
        } catch (const Error &e) { result.error = e; }
    }
    return result;
}

void EngineSession::confirmations()
{
    invalidate();
    if (!state_.ready || pending_) return;
    confirmedConfig_ = state_.config;
    try {
        confirmedHome_ = consentHome(state_.config);
        if (!confirmedHome_.empty()) consentToken_ = randomToken();
    } catch (const Error &) { }
    if (state_.desktop && state_.process.state == "stopped" && queryElevation_() == "not_elevated"
        && (!state_.desktop->registered || state_.config.manageBackend))
        repairToken_ = randomToken();
}

Json EngineSession::snapshot() const
{
    const std::string level = queryElevation_();
    const bool enabled = state_.config.manageBackend;
    const bool canLaunch = state_.ready && !pending_ && state_.desktop.has_value()
        && state_.process.state == "stopped" && level == "not_elevated";
    Json desktop{{"state", "unknown"}};
    if (state_.desktop) {
        desktop = state_.desktop->publicJson();
        desktop["state"] = "found";
    } else if (state_.error) desktop["state"] = "not_found";
    Json error = Json(nullptr);
    if (!state_.ready)
        error = Error("CONFIG_INVALID", "Repair the proxy configuration before launch.").json();
    else if (level != "not_elevated")
        error = Error("ELEVATION_NOT_ALLOWED", "Run Guard as a normal user; elevation must be verifiable.").json();
    else if (state_.error) error = state_.error->json();
    return Json{
        {"config_readiness", state_.ready ? "ready" : "repair_required"},
        {"proxy", Json{{"url", state_.config.proxyUrl()}, {"host", state_.config.host}, {"port", state_.config.port}}},
        {"launch", Json{{"method", state_.desktop ? (state_.desktop->registered ? "appmodel_activation" : "native_process") : "unknown"}}},
        {"desktop", desktop},
        {"process", Json{{"state", state_.process.state}, {"pid", state_.process.pid ? Json(static_cast<std::int64_t>(state_.process.pid)) : Json(nullptr)}}},
        {"coverage", Json{{"state", state_.coverage}, {"enabled", enabled},
            {"authorized_home", enabled ? Json(wideToUtf8(state_.config.home.wstring())) : Json(nullptr)},
            {"candidate_home", confirmedHome_.empty() ? Json(nullptr) : Json(wideToUtf8(confirmedHome_.wstring()))}}},
        {"busy", pending_}, {"elevation", level}, {"error", error},
        {"actions", Json{{"can_launch", canLaunch}, {"can_edit_proxy", !pending_},
            {"can_authorize_backend_proxy", !pending_ && !enabled && !consentToken_.empty()},
            {"can_revoke_backend_proxy", !pending_ && enabled && !consentToken_.empty()},
            {"can_repair", canLaunch && !repairToken_.empty()}, {"can_refresh", !pending_},
            {"can_cancel", pending_ && launching_}}},
        {"confirmations", Json{
            {"backend_proxy", consentToken_.empty() ? Json(nullptr)
                : Json{{"token", consentToken_}, {"enabled", !enabled}, {"home", wideToUtf8(confirmedHome_.wstring())}}},
            {"repair", repairToken_.empty() ? Json(nullptr) : Json{{"token", repairToken_}}}}}};
}

void EngineSession::dispatch(bool launch, std::function<Result()> effect)
{
    // Candidate and authorization were checked before committing a foreground effect.
    launching_ = launch;
    pending_ = true;
    dispatchedAt_ = std::chrono::steady_clock::now();
    task_ = std::async(std::launch::async, [effect = std::move(effect)] {
        try { return effect(); }
        catch (const Error &e) { Result result; result.error = e; return result; }
        catch (...) {
            Result result;
            result.error = Error("ENGINE_TASK_FAILED", "The engine task failed; refresh before retrying.");
            return result;
        }
    });
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
    if (pending_) throw Error("BRIDGE_BUSY", "Another foreground operation is active.");
    if (port < 1 || port > 65535) throw Error("CONFIG_INVALID", "Proxy port must be between 1 and 65535.");
    const auto expected = state_.config;
    Config candidate = expected;
    candidate.host = host;
    candidate.port = port;
    candidate.validate();
    invalidate();
    cancellation_ = Cancellation();
    const bool invalid = !state_.ready;
    dispatch(false, [this, expected, invalid, host, port, cancel = cancellation_] {
        cancel.check();
        updateProxy(path_, expected, invalid, host, port);
        return refresh(cancel);
    });
}

void EngineSession::requestSetBackendConsent(bool enabled, const std::string &token)
{
    if (pending_) throw Error("BRIDGE_BUSY", "Another foreground operation is active.");
    if (token.empty() || token.size() > 128)
        throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this action again.");
    if (!state_.ready) throw Error("CONFIG_INVALID", "Refresh and repair the configuration first.");
    const auto expected = state_.config;
    if (!(Config::load(path_) == expected)) {
        invalidate();
        throw Error("CONFIG_CHANGED", "Configuration changed; refresh and confirm again.");
    }
    const bool authorized = token == consentToken_ && !consentToken_.empty()
        && expected == confirmedConfig_ && enabled != expected.manageBackend;
    const auto home = confirmedHome_;
    invalidate();
    if (!authorized) throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this action again.");
    cancellation_ = Cancellation();
    dispatch(false, [this, expected, enabled, home, cancel = cancellation_] {
        cancel.check();
        updateConsent(path_, expected, enabled, home);
        return refresh(cancel);
    });
}

void EngineSession::requestLaunch(bool repair, const std::string &token)
{
    if (pending_) throw Error("BRIDGE_BUSY", "Another foreground operation is active.");
    if (!state_.ready) throw Error("CONFIG_INVALID", "Refresh and repair the configuration first.");
    const auto expected = state_.config;
    if (!(Config::load(path_) == expected)) {
        invalidate();
        throw Error("CONFIG_CHANGED", "Configuration changed; refresh and confirm again.");
    }
    if (repair) {
        const bool authorized = token == repairToken_ && !repairToken_.empty()
            && expected == confirmedConfig_;
        invalidate();
        if (!authorized) throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this repair launch again.");
    } else {
        if (!token.empty())
            throw Error("BRIDGE_PARAMS_INVALID", "Normal launch does not accept a confirmation token.");
        invalidate();
    }
    requireNonElevated();
    ++operation_;
    cancellation_ = Cancellation();
    dispatch(true, [this, expected, repair, cancel = cancellation_] {
        Result result;
        result.value = launchPipeline(expected, path_, {repair, false}, cancel);
        return result;
    });
}

EngineSession::Event EngineSession::poll()
{
    mutationFailure_ = false;
    mutationError_.reset();
    if (!pending_) return Event::None;
    if (task_.wait_for(std::chrono::milliseconds(0)) != std::future_status::ready) {
        const auto elapsed = std::chrono::steady_clock::now() - dispatchedAt_;
        if (elapsed > (launching_ ? std::chrono::seconds(840) : std::chrono::seconds(60))) {
            cancel();
            mutationError_ = Error("SESSION_TIMEOUT", "The foreground operation exceeded its budget and was cancelled; refresh.");
            mutationFailure_ = true;
            pending_ = false;
            return Event::MutationFailed;
        }
        return Event::None;
    }
    auto result = task_.get();
    pending_ = false;
    if (launching_) {
        invalidate();
        // Previous process observations are stale after any submitted launch.
        state_.process = {};
        state_.error = result.error;
        state_.value = result.value;
        return Event::OperationFinished;
    }
    // Discovery failure is part of a usable snapshot; mutation exceptions
    // never commit the mutated state.
    if (!result.snapshot && result.error) {
        invalidate();
        mutationError_ = result.error;
        mutationFailure_ = true;
        return Event::MutationFailed;
    }
    state_ = std::move(result);
    confirmations();
    return Event::SnapshotArrived;
}

} // namespace cpg
