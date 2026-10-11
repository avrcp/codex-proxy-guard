#include "bridge.h"
#include "cpg/build_info.h"
#include "cpg/win_handle.h"

#include <cmath>
#include <cstdio>
#include <memory>
#include <thread>
#include <windows.h>

namespace cpg {
namespace {

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
    if (state->bytes.size() > 128 * 1024)
        throw Error("BRIDGE_OUTPUT_INVALID", "Engine output exceeds its limit.");
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
    : session_(std::make_unique<EngineSession>(std::move(path), std::move(discovery),
                                                std::move(queryElevation))) {}
BridgeSession::~BridgeSession() = default;
void BridgeSession::cancel() { session_->cancel(); }

std::vector<Json> BridgeSession::request(const std::string &frame)
{
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
        if (method == "shutdown") {
            keys(params, {}, {});
            cancel();
            closed_ = true;
            return {success(id, Json{{"shutting_down", true}})};
        }
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
            if (!session_->busy() || !session_->launching()
                || integer(at(params, "operation_id")) != session_->operationId())
                throw Error("OPERATION_NOT_FOUND", "No matching launch is active.");
            cancel();
            return {success(id, Json{{"cancellation_requested", true}}),
                    Json{{"schema", 1}, {"event", "operation_state"}, {"operation_id", session_->operationId()},
                         {"state", "cancelling"},
                         {"message", "Cancellation requested; waiting for the engine outcome."}}};
        }
        if (method == "snapshot") {
            keys(params, {}, {});
            if (session_->busy()) return {success(id, session_->snapshot())};
            session_->requestRefresh();
            pendingId_ = id;
            return {};
        }
        if (method == "set_proxy") {
            keys(params, {"host", "port"}, {"host", "port"});
            if (!at(params, "host").is_string()) throw Error("BRIDGE_PARAMS_INVALID", "Expected a proxy host.");
            const std::int64_t portValue = integer(at(params, "port"));
            if (portValue > 65535)
                throw Error("CONFIG_INVALID", "Proxy port must be between 1 and 65535.");
            session_->requestSetProxy(at(params, "host").get<std::string>(),
                                      static_cast<int>(portValue));
            pendingId_ = id;
            return {};
        }
        if (method == "set_backend_proxy_consent") {
            keys(params, {"enabled", "confirmation_token"}, {"enabled", "confirmation_token"});
            session_->requestSetBackendConsent(boolean(params, "enabled"), token(params));
            pendingId_ = id;
            return {};
        }
        if (method == "start_launch") {
            keys(params, {"repair", "confirmation_token"}, {"repair"});
            session_->requestLaunch(boolean(params, "repair"), at(params, "confirmation_token").is_string()
                ? token(params) : std::string());
            pendingId_ = id;
            return {success(id, Json{{"operation_id", session_->operationId()}}),
                    Json{{"schema", 1}, {"event", "operation_state"}, {"operation_id", session_->operationId()},
                         {"state", "running"},
                         {"message", boolean(params, "repair")
                             ? "Preparing the proxy configuration and repair launch." : "Launching Desktop."}}};
        }
        throw Error("BRIDGE_METHOD_UNKNOWN", "Unknown bridge method.");
    } catch (const Error &e) { return {failure(id, e)}; }
    catch (...) { return {failure(id, Error("ENGINE_TASK_FAILED", "The engine request failed."))}; }
}

std::vector<Json> BridgeSession::poll()
{
    const auto event = session_->poll();
    switch (event) {
    case EngineSession::Event::SnapshotArrived:
        return {success(pendingId_, session_->snapshot())};
    case EngineSession::Event::OperationFinished: {
        const auto error = session_->operationError();
        Json message{{"schema", 1}, {"event", "operation_finished"},
                     {"operation_id", session_->operationId()}, {"ok", !error.has_value()}};
        message[error ? "error" : "result"] = error ? error->json() : session_->operationResult();
        return {message};
    }
    case EngineSession::Event::MutationFailed:
        return {failure(pendingId_, session_->mutationError().value_or(Error("ENGINE_TASK_FAILED", "The engine request failed.")))};
    case EngineSession::Event::None:
        break;
    }
    return {};
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
