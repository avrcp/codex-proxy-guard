#ifndef _CRT_RAND_S
#define _CRT_RAND_S
#endif
#include "bridge.h"
#include "cpg/build_info.h"
#include "cpg/win_handle.h"

#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <thread>
#include <windows.h>

namespace cpg {
namespace {

std::string randomToken()
{
    // 128 bits from the CSPRNG-backed rand_s, formatted as a UUID-shaped hex
    // token. Single-use confirmation nonce; not a security boundary by itself.
    unsigned int values[4] = {};
    for (unsigned int &value : values) {
        if (rand_s(&value) != 0) throw Error("BRIDGE_RANDOM_UNAVAILABLE", "Cannot generate a confirmation token.");
    }
    char buffer[33] = {};
    std::snprintf(buffer, sizeof buffer, "%08x%08x%08x%08x",
                  values[0], values[1], values[2], values[3]);
    return buffer;
}

Json success(std::int64_t id, const Json &result)
{
    return Json{{"schema", 1}, {"id", id}, {"ok", true}, {"result", result}};
}
Json failure(std::int64_t id, const Error &error)
{
    return Json{{"schema", 1}, {"id", id > 0 ? Json(id) : Json(nullptr)},
                {"ok", false}, {"error", error.json()}};
}

const Json &at(const Json &object, const char *key)
{
    static const Json nullValue = Json();
    const auto entry = object.find(key);
    return entry == object.end() ? nullValue : *entry;
}

void keys(const Json &object, std::initializer_list<const char *> allowed,
          std::initializer_list<const char *> required)
{
    for (auto entry = object.begin(); entry != object.end(); ++entry) {
        bool known = false;
        for (const char *candidate : allowed)
            if (entry.key() == candidate) { known = true; break; }
        if (!known) throw Error("BRIDGE_PARAMS_INVALID", "Unexpected request field.");
    }
    for (const char *key : required)
        if (!object.contains(key)) throw Error("BRIDGE_PARAMS_INVALID", "Missing request field.");
}

std::int64_t integer(const Json &value)
{
    if (value.is_number_float()) {
        const double n = value.get<double>();
        if (!std::isfinite(n) || n < 1 || n > 9007199254740991.0 || std::floor(n) != n)
            throw Error("BRIDGE_PARAMS_INVALID", "Expected a positive safe integer.");
        return static_cast<std::int64_t>(n);
    }
    if (value.is_number_unsigned()) {
        const std::uint64_t n = value.get<std::uint64_t>();
        if (n < 1 || n > 9007199254740991ull)
            throw Error("BRIDGE_PARAMS_INVALID", "Expected a positive safe integer.");
        return static_cast<std::int64_t>(n);
    }
    if (value.is_number_integer()) {
        const std::int64_t n = value.get<std::int64_t>();
        if (n < 1 || n > 9007199254740991)
            throw Error("BRIDGE_PARAMS_INVALID", "Expected a positive safe integer.");
        return n;
    }
    throw Error("BRIDGE_PARAMS_INVALID", "Expected a positive safe integer.");
}

std::string token(const Json &params)
{
    const Json &value = at(params, "confirmation_token");
    if (!value.is_string()) throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this action again.");
    const std::string result = value.get<std::string>();
    if (result.empty() || result.size() > 128)
        throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this action again.");
    return result;
}

bool boolean(const Json &params, const char *key)
{
    const Json &value = at(params, key);
    if (!value.is_boolean()) throw Error("BRIDGE_PARAMS_INVALID", "Expected a boolean.");
    return value.get<bool>();
}

// A blocked reader must not pin the engine. The writer owns no business state,
// and synchronous Windows pipe I/O can be cancelled on the owning thread.
void writeFrame(const Json &value)
{
    struct WriteState { std::string bytes; bool ok = false; };
    const auto state = std::make_shared<WriteState>();
    state->bytes = jsonDump(value) + '\n';
    if (state->bytes.size() > 128 * 1024) throw Error("BRIDGE_OUTPUT_INVALID", "Engine output exceeds its limit.");
    std::thread writer([state] {
        DWORD written = 0;
        state->ok = WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), state->bytes.data(),
                              static_cast<DWORD>(state->bytes.size()), &written, nullptr)
            && written == static_cast<DWORD>(state->bytes.size());
    });
    const HANDLE handle = static_cast<HANDLE>(writer.native_handle());
    bool done = WaitForSingleObject(handle, 2000) == WAIT_OBJECT_0;
    // Cancellation can race with a not-yet-entered WriteFile. Retry until the
    // thread acknowledges it; never unbounded-join an unresponsive pipe writer.
    for (int attempt = 0; !done && attempt < 100; ++attempt) {
        CancelSynchronousIo(handle);
        done = WaitForSingleObject(handle, 10) == WAIT_OBJECT_0;
    }
    if (!done) { writer.detach(); throw Error("BRIDGE_IO_FAILED", "Bridge output stalled."); }
    writer.join();
    if (!state->ok) throw Error("BRIDGE_IO_FAILED", "Bridge output closed or stalled.");
}
}

BridgeSession::BridgeSession(std::filesystem::path path,
    std::function<Desktop(const Config &, const Cancellation &)> discovery,
    std::function<std::string()> queryElevation)
    : path_(std::move(path)), discovery_(std::move(discovery)),
      queryElevation_(std::move(queryElevation)) {}
BridgeSession::~BridgeSession() { cancel(); if (task_.valid()) task_.wait(); }
void BridgeSession::cancel() { cancellation_.cancel(); invalidate(); }
void BridgeSession::invalidate() { consentToken_.clear(); repairToken_.clear(); confirmedHome_.clear(); }

BridgeSession::Result BridgeSession::refresh(const Cancellation &cancel) const {
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

void BridgeSession::confirmations() {
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

Json BridgeSession::snapshot() const {
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
    if (!state_.ready) error = Error("CONFIG_INVALID", "Repair the proxy configuration before launch.").json();
    else if (level != "not_elevated") error = Error("ELEVATION_NOT_ALLOWED", "Run Guard as a normal user; elevation must be verifiable.").json();
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

void BridgeSession::dispatch(std::int64_t id, bool launch, std::function<Result()> effect) {
    // Candidate and authorization were checked before committing a foreground effect.
    pendingId_ = id;
    launching_ = launch;
    pending_ = true;
    timer_ = std::chrono::steady_clock::now();
    task_ = std::async(std::launch::async, [effect = std::move(effect)] {
        try { return effect(); }
        catch (const Error &e) { Result result; result.error = e; return result; }
        catch (...) { Result result; result.error = Error("ENGINE_TASK_FAILED", "The engine task failed; refresh before retrying."); return result; }
    });
}

std::vector<Json> BridgeSession::request(const std::string &frame) {
    std::int64_t id = 0;
    try {
        const Json request = parseStrictJsonObject(frame, 32 * 1024, "BRIDGE_REQUEST_INVALID");
        keys(request, {"schema", "id", "method", "params"}, {"schema", "id", "method", "params"});
        id = integer(at(request, "id"));
        if (at(request, "schema") != Json(1)) throw Error("BRIDGE_SCHEMA_UNSUPPORTED", "Unsupported bridge schema.");
        if (!at(request, "method").is_string() || !at(request, "params").is_object())
            throw Error("BRIDGE_REQUEST_INVALID", "Malformed bridge request.");
        const std::string method = at(request, "method").get<std::string>();
        const Json &params = at(request, "params");
        if (id <= lastId_) throw Error("BRIDGE_ID_REUSED", "Request ids must strictly increase.");
        lastId_ = id;
        if (method == "shutdown") { keys(params, {}, {}); cancel(); closed_ = true; return {success(id, Json{{"shutting_down", true}})}; }
        if (method == "hello") {
            keys(params, {}, {});
            hello_ = true;
            return {success(id, Json{{"protocol_version", 1}, {"engine_version", productVersion()},
                                     {"engine_commit", buildCommit()},
                                     {"capabilities", Json::array({"snapshot", "set_proxy", "backend_proxy_consent", "launch", "repair", "cancel"})}})};
        }
        if (!hello_) throw Error("BRIDGE_HELLO_REQUIRED", "Send hello before business requests.");
        if (method == "cancel_operation") {
            keys(params, {"operation_id"}, {"operation_id"});
            if (!pending_ || !launching_ || integer(at(params, "operation_id")) != operation_)
                throw Error("OPERATION_NOT_FOUND", "No matching launch is active.");
            cancel();
            return {success(id, Json{{"cancellation_requested", true}}),
                    Json{{"schema", 1}, {"event", "operation_state"}, {"operation_id", operation_},
                         {"state", "cancelling"},
                         {"message", "Cancellation requested; waiting for the engine outcome."}}};
        }
        if (method == "snapshot") {
            keys(params, {}, {});
            if (pending_) return {success(id, snapshot())};
            invalidate();
            cancellation_ = Cancellation();
            dispatch(id, false, [this, cancel = cancellation_] { return refresh(cancel); });
            return {};
        }
        if (method != "set_proxy" && method != "set_backend_proxy_consent" && method != "start_launch")
            throw Error("BRIDGE_METHOD_UNKNOWN", "Unknown bridge method.");
        if (pending_) throw Error("BRIDGE_BUSY", "Another foreground operation is active.");
        cancellation_ = Cancellation();
        const auto expected = state_.config;
        if (method == "set_proxy") {
            keys(params, {"host", "port"}, {"host", "port"});
            if (!at(params, "host").is_string()) throw Error("BRIDGE_PARAMS_INVALID", "Expected a proxy host.");
            const std::string host = at(params, "host").get<std::string>();
            const auto port = integer(at(params, "port"));
            if (port > 65535) throw Error("CONFIG_INVALID", "Proxy port must be between 1 and 65535.");
            Config candidate = expected;
            candidate.host = host;
            candidate.port = static_cast<int>(port);
            candidate.validate();
            invalidate();
            dispatch(id, false, [this, expected, invalid = !state_.ready, host, port, cancel = cancellation_] {
                cancel.check();
                updateProxy(path_, expected, invalid, host, static_cast<int>(port));
                return refresh(cancel);
            });
            return {};
        }
        if (!state_.ready) throw Error("CONFIG_INVALID", "Refresh and repair the configuration first.");
        if (!(Config::load(path_) == expected)) { invalidate(); throw Error("CONFIG_CHANGED", "Configuration changed; refresh and confirm again."); }
        if (method == "set_backend_proxy_consent") {
            keys(params, {"enabled", "confirmation_token"}, {"enabled", "confirmation_token"});
            const bool enabled = boolean(params, "enabled");
            const std::string supplied = token(params);
            const bool authorized = supplied == consentToken_ && !consentToken_.empty()
                && expected == confirmedConfig_ && enabled != expected.manageBackend;
            const auto home = confirmedHome_;
            invalidate();
            if (!authorized) throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this action again.");
            dispatch(id, false, [this, expected, enabled, home, cancel = cancellation_] {
                cancel.check();
                updateConsent(path_, expected, enabled, home);
                return refresh(cancel);
            });
            return {};
        }
        keys(params, {"repair", "confirmation_token"}, {"repair"});
        const bool repair = boolean(params, "repair");
        if (repair) {
            const bool authorized = token(params) == repairToken_ && !repairToken_.empty()
                && expected == confirmedConfig_;
            invalidate();
            if (!authorized) throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this repair launch again.");
        } else {
            if (params.contains("confirmation_token"))
                throw Error("BRIDGE_PARAMS_INVALID", "Normal launch does not accept a confirmation token.");
            invalidate();
        }
        requireNonElevated();
        ++operation_;
        dispatch(id, true, [this, expected, repair, cancel = cancellation_] {
            Result result;
            result.value = launchPipeline(expected, path_, {repair, false}, cancel);
            return result;
        });
        return {success(id, Json{{"operation_id", operation_}}),
                Json{{"schema", 1}, {"event", "operation_state"}, {"operation_id", operation_},
                     {"state", "running"},
                     {"message", repair ? "Preparing the proxy configuration and repair launch." : "Launching Desktop."}}};
    } catch (const Error &e) { return {failure(id, e)}; }
    catch (...) { return {failure(id, Error("ENGINE_TASK_FAILED", "The engine request failed."))}; }
}

std::vector<Json> BridgeSession::poll() {
    if (!pending_) return {};
    if (task_.wait_for(std::chrono::milliseconds(0)) != std::future_status::ready) {
        const auto elapsed = std::chrono::steady_clock::now() - timer_;
        if (elapsed > (launching_ ? std::chrono::seconds(840) : std::chrono::seconds(60))) {
            cancel();
            closed_ = true;
        }
        return {};
    }
    auto result = task_.get();
    pending_ = false;
    if (launching_) {
        invalidate();
        // Previous process observations are stale after any submitted launch.
        state_.process = {};
        state_.error = result.error;
        Json event{{"schema", 1}, {"event", "operation_finished"}, {"operation_id", operation_},
                   {"ok", !result.error.has_value()}};
        event[result.error ? "error" : "result"] = result.error ? result.error->json() : result.value;
        return {event};
    }
    // Discovery failure is part of a usable snapshot; mutation exceptions have
    // no ready configuration and are returned as errors without committing it.
    if (!result.snapshot && result.error) {
        invalidate();
        return {failure(pendingId_, *result.error)};
    }
    state_ = std::move(result);
    confirmations();
    return {success(pendingId_, snapshot())};
}

int runBridge(const std::filesystem::path &configPath) {
    BridgeSession session(configPath);
    std::string input;
    const HANDLE pipe = GetStdHandle(STD_INPUT_HANDLE);
    if (GetFileType(pipe) != FILE_TYPE_PIPE) return 2;
    try {
        while (!session.closed()) {
            for (const auto &message : session.poll()) writeFrame(message);
            DWORD available = 0;
            if (!PeekNamedPipe(pipe, nullptr, 0, nullptr, &available, nullptr)) break;
            if (available) {
                char buffer[4096];
                DWORD read = 0;
                if (!ReadFile(pipe, buffer, available < sizeof buffer ? available : static_cast<DWORD>(sizeof buffer), &read, nullptr) || !read) break;
                input.append(buffer, read);
                std::size_t newline = std::string::npos;
                while ((newline = input.find('\n')) != std::string::npos && !session.closed()) {
                    const std::string frame = input.substr(0, newline);
                    input.erase(0, newline + 1);
                    for (const auto &message : session.request(frame)) writeFrame(message);
                }
                if (input.size() > 32 * 1024) {
                    writeFrame(failure(0, Error("BRIDGE_REQUEST_TOO_LARGE", "Request exceeds its limit.")));
                    break;
                }
            } else Sleep(5);
        }
    } catch (const Error &) { session.cancel(); return 2; }
    session.cancel();
    return 0;
}

} // namespace cpg
