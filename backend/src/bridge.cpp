#include "bridge.h"
#include <QJsonArray>
#include <QJsonDocument>
#include <QUuid>
#include <QThread>
#include <cmath>
#include <thread>
#include <windows.h>

namespace cpg {
namespace {
QJsonObject success(qint64 id, const QJsonObject &result) {
    return {{"schema", 1}, {"id", id}, {"ok", true}, {"result", result}};
}
QJsonObject failure(qint64 id, const Error &error) {
    return {{"schema", 1}, {"id", id > 0 ? QJsonValue(id) : QJsonValue()},
            {"ok", false}, {"error", error.json()}};
}
void keys(const QJsonObject &object, const QStringList &allowed, const QStringList &required) {
    for (auto it = object.begin(); it != object.end(); ++it)
        if (!allowed.contains(it.key())) throw Error("BRIDGE_PARAMS_INVALID", "Unexpected request field.");
    for (const auto &key : required)
        if (!object.contains(key)) throw Error("BRIDGE_PARAMS_INVALID", "Missing request field.");
}
qint64 integer(const QJsonValue &value) {
    const double n = value.toDouble(-1);
    if (!value.isDouble() || !std::isfinite(n) || n < 1 || n > 9007199254740991.0 || std::floor(n) != n)
        throw Error("BRIDGE_PARAMS_INVALID", "Expected a positive safe integer.");
    return static_cast<qint64>(n);
}
QString token(const QJsonObject &params) {
    const auto value = params.value("confirmation_token");
    if (!value.isString() || value.toString().isEmpty() || value.toString().size() > 128)
        throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this action again.");
    return value.toString();
}
bool boolean(const QJsonObject &params, const QString &key) {
    if (!params.value(key).isBool()) throw Error("BRIDGE_PARAMS_INVALID", "Expected a boolean.");
    return params.value(key).toBool();
}
// A blocked reader must not pin the engine. The writer owns no business state,
// and synchronous Windows pipe I/O can be cancelled on the owning thread.
void writeFrame(const QJsonObject &value) {
    QByteArray bytes = QJsonDocument(value).toJson(QJsonDocument::Compact) + '\n';
    if (bytes.size() > 128 * 1024) throw Error("BRIDGE_OUTPUT_INVALID", "Engine output exceeds its limit.");
    bool ok = false;
    std::thread writer([&] {
        DWORD written = 0;
        ok = WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), bytes.constData(), static_cast<DWORD>(bytes.size()), &written, nullptr)
             && written == static_cast<DWORD>(bytes.size());
    });
    const HANDLE handle = static_cast<HANDLE>(writer.native_handle());
    if (WaitForSingleObject(handle, 2000) != WAIT_OBJECT_0) CancelSynchronousIo(handle);
    writer.join();
    if (!ok) throw Error("BRIDGE_IO_FAILED", "Bridge output closed or stalled.");
}
}

BridgeSession::BridgeSession(QString path,
    std::function<Desktop(const Config &, const Cancellation &)> discovery,
    std::function<QString()> queryElevation)
    : path_(std::move(path)), discovery_(std::move(discovery)), queryElevation_(std::move(queryElevation)) {}
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
        if (!confirmedHome_.isEmpty()) consentToken_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
    } catch (const Error &) { }
    if (state_.desktop && state_.process.state == "stopped" && queryElevation_() == "not_elevated"
        && (!state_.desktop->registered || state_.config.manageBackend))
        repairToken_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
}
QJsonObject BridgeSession::snapshot() const {
    const QString level = queryElevation_();
    const bool enabled = state_.config.manageBackend;
    const bool canLaunch = state_.ready && !pending_ && state_.desktop.has_value()
        && state_.process.state == "stopped" && level == "not_elevated";
    QJsonObject desktop{{"state", "unknown"}};
    if (state_.desktop) { desktop = state_.desktop->publicJson(); desktop["state"] = "found"; }
    else if (state_.error) desktop["state"] = "not_found";
    QJsonValue error;
    if (!state_.ready) error = Error("CONFIG_INVALID", "Repair the proxy configuration before launch.").json();
    else if (level != "not_elevated") error = Error("ELEVATION_NOT_ALLOWED", "Run Guard as a normal user; elevation must be verifiable.").json();
    else if (state_.error) error = state_.error->json();
    return {
        {"config_readiness", state_.ready ? "ready" : "repair_required"},
        {"proxy", QJsonObject{{"url", state_.config.proxyUrl()}, {"host", state_.config.host}, {"port", state_.config.port}}},
        {"launch", QJsonObject{{"method", state_.desktop ? (state_.desktop->registered ? "appmodel_activation" : "native_process") : "unknown"}}},
        {"desktop", desktop}, {"process", QJsonObject{{"state", state_.process.state}, {"pid", state_.process.pid ? QJsonValue(static_cast<qint64>(state_.process.pid)) : QJsonValue()}}},
        {"coverage", QJsonObject{{"state", state_.coverage}, {"enabled", enabled}, {"authorized_home", enabled ? QJsonValue(state_.config.home) : QJsonValue()}, {"candidate_home", confirmedHome_.isEmpty() ? QJsonValue() : QJsonValue(confirmedHome_)}}},
        {"busy", pending_}, {"elevation", level}, {"error", error},
        {"actions", QJsonObject{{"can_launch", canLaunch}, {"can_edit_proxy", !pending_}, {"can_authorize_backend_proxy", !pending_ && !enabled && !consentToken_.isEmpty()}, {"can_revoke_backend_proxy", !pending_ && enabled && !consentToken_.isEmpty()}, {"can_repair", canLaunch && !repairToken_.isEmpty()}, {"can_refresh", !pending_}, {"can_cancel", pending_ && launching_}}},
        {"confirmations", QJsonObject{{"backend_proxy", consentToken_.isEmpty() ? QJsonValue() : QJsonValue(QJsonObject{{"token", consentToken_}, {"enabled", !enabled}, {"home", confirmedHome_}})}, {"repair", repairToken_.isEmpty() ? QJsonValue() : QJsonValue(QJsonObject{{"token", repairToken_}})}}}
    };
}
void BridgeSession::dispatch(qint64 id, bool launch, std::function<Result()> effect) {
    // Candidate and authorization were checked before committing a foreground effect.
    pendingId_ = id; launching_ = launch; pending_ = true; timer_.start();
    task_ = std::async(std::launch::async, [effect = std::move(effect)] {
        try { return effect(); }
        catch (const Error &e) { Result result; result.error = e; return result; }
        catch (...) { Result result; result.error = Error("ENGINE_TASK_FAILED", "The engine task failed; refresh before retrying."); return result; }
    });
}
QList<QJsonObject> BridgeSession::request(const QByteArray &frame) {
    qint64 id = 0;
    try {
        const auto request = parseJsonObject(frame, 32 * 1024, "BRIDGE_REQUEST_INVALID");
        keys(request, {"schema", "id", "method", "params"}, {"schema", "id", "method", "params"});
        id = integer(request.value("id"));
        if (request.value("schema") != QJsonValue(1)) throw Error("BRIDGE_SCHEMA_UNSUPPORTED", "Unsupported bridge schema.");
        if (!request.value("method").isString() || !request.value("params").isObject()) throw Error("BRIDGE_REQUEST_INVALID", "Malformed bridge request.");
        const auto method = request.value("method").toString();
        const auto params = request.value("params").toObject();
        if (id <= lastId_) throw Error("BRIDGE_ID_REUSED", "Request ids must strictly increase.");
        lastId_ = id;
        if (method == "shutdown") { keys(params, {}, {}); cancel(); closed_ = true; return {success(id, {{"shutting_down", true}})}; }
        if (method == "hello") {
            keys(params, {}, {}); hello_ = true;
            return {success(id, {{"protocol_version", 1}, {"engine_version", CPG_PRODUCT_VERSION}, {"engine_commit", CPG_BUILD_COMMIT}, {"capabilities", QJsonArray{"snapshot", "set_proxy", "backend_proxy_consent", "launch", "repair", "cancel"}}})};
        }
        if (!hello_) throw Error("BRIDGE_HELLO_REQUIRED", "Send hello before business requests.");
        if (method == "cancel_operation") {
            keys(params, {"operation_id"}, {"operation_id"});
            if (!pending_ || !launching_ || integer(params.value("operation_id")) != operation_) throw Error("OPERATION_NOT_FOUND", "No matching launch is active.");
            cancel();
            return {success(id, {{"cancellation_requested", true}}), {{"schema", 1}, {"event", "operation_state"}, {"operation_id", operation_}, {"state", "cancelling"}, {"message", "Cancellation requested; waiting for the engine outcome."}}};
        }
        if (method == "snapshot") {
            keys(params, {}, {});
            if (pending_) return {success(id, snapshot())};
            invalidate(); cancellation_ = Cancellation();
            dispatch(id, false, [this, cancel = cancellation_] { return refresh(cancel); }); return {};
        }
        if (method != "set_proxy" && method != "set_backend_proxy_consent" && method != "start_launch") throw Error("BRIDGE_METHOD_UNKNOWN", "Unknown bridge method.");
        if (pending_) throw Error("BRIDGE_BUSY", "Another foreground operation is active.");
        cancellation_ = Cancellation();
        const auto expected = state_.config;
        if (method == "set_proxy") {
            keys(params, {"host", "port"}, {"host", "port"});
            if (!params.value("host").isString()) throw Error("BRIDGE_PARAMS_INVALID", "Expected a proxy host.");
            const auto host = params.value("host").toString();
            const auto port = integer(params.value("port"));
            if (port > 65535) throw Error("CONFIG_INVALID", "Proxy port must be between 1 and 65535.");
            Config candidate = expected; candidate.host = host; candidate.port = static_cast<int>(port); candidate.validate();
            invalidate();
            dispatch(id, false, [this, expected, invalid = !state_.ready, host, port, cancel = cancellation_] {
                cancel.check(); updateProxy(path_, expected, invalid, host, static_cast<int>(port)); return refresh(cancel);
            }); return {};
        }
        if (!state_.ready) throw Error("CONFIG_INVALID", "Refresh and repair the configuration first.");
        if (!(Config::load(path_) == expected)) { invalidate(); throw Error("CONFIG_CHANGED", "Configuration changed; refresh and confirm again."); }
        if (method == "set_backend_proxy_consent") {
            keys(params, {"enabled", "confirmation_token"}, {"enabled", "confirmation_token"});
            const bool enabled = boolean(params, "enabled");
            const auto supplied = token(params);
            const bool authorized = supplied == consentToken_ && !consentToken_.isEmpty() && expected == confirmedConfig_ && enabled != expected.manageBackend;
            const auto home = confirmedHome_; invalidate();
            if (!authorized) throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this action again.");
            dispatch(id, false, [this, expected, enabled, home, cancel = cancellation_] {
                cancel.check(); updateConsent(path_, expected, enabled, home); return refresh(cancel);
            }); return {};
        }
        keys(params, {"repair", "confirmation_token"}, {"repair"});
        const bool repair = boolean(params, "repair");
        if (repair) {
            const bool authorized = token(params) == repairToken_ && !repairToken_.isEmpty() && expected == confirmedConfig_;
            invalidate();
            if (!authorized) throw Error("CONFIRMATION_REQUIRED", "Refresh and confirm this repair launch again.");
        } else {
            if (params.contains("confirmation_token")) throw Error("BRIDGE_PARAMS_INVALID", "Normal launch does not accept a confirmation token.");
            invalidate();
        }
        requireNonElevated();
        ++operation_;
        dispatch(id, true, [this, expected, repair, cancel = cancellation_] {
            Result result; result.value = launchPipeline(expected, path_, {repair, false}, cancel); return result;
        });
        return {success(id, {{"operation_id", operation_}}), {{"schema", 1}, {"event", "operation_state"}, {"operation_id", operation_}, {"state", "running"}, {"message", repair ? "Preparing the proxy configuration and repair launch." : "Launching Desktop."}}};
    } catch (const Error &e) { return {failure(id, e)}; }
    catch (...) { return {failure(id, Error("ENGINE_TASK_FAILED", "The engine request failed."))}; }
}
QList<QJsonObject> BridgeSession::poll() {
    if (!pending_) return {};
    if (task_.wait_for(std::chrono::milliseconds(0)) != std::future_status::ready) {
        if (timer_.elapsed() > (launching_ ? 840000 : 60000)) { cancel(); closed_ = true; }
        return {};
    }
    auto result = task_.get(); pending_ = false;
    if (launching_) {
        invalidate();
        // Previous process observations are stale after any submitted launch.
        state_.process = {}; state_.error = result.error;
        QJsonObject event{{"schema", 1}, {"event", "operation_finished"}, {"operation_id", operation_}, {"ok", !result.error.has_value()}};
        event[result.error ? "error" : "result"] = result.error ? result.error->json() : result.value;
        return {event};
    }
    // Discovery failure is part of a usable snapshot; mutation exceptions have
    // no ready configuration and are returned as errors without committing it.
    if (!result.snapshot && result.error) {
        invalidate(); return {failure(pendingId_, *result.error)};
    }
    state_ = std::move(result); confirmations();
    return {success(pendingId_, snapshot())};
}
int runBridge(const QString &configPath) {
    BridgeSession session(configPath);
    QByteArray input;
    const HANDLE pipe = GetStdHandle(STD_INPUT_HANDLE);
    if (GetFileType(pipe) != FILE_TYPE_PIPE) return 2;
    try {
        while (!session.closed()) {
            for (const auto &message : session.poll()) writeFrame(message);
            DWORD available = 0;
            if (!PeekNamedPipe(pipe, nullptr, 0, nullptr, &available, nullptr)) break;
            if (available) {
                char buffer[4096]; DWORD read = 0;
                if (!ReadFile(pipe, buffer, qMin<DWORD>(available, sizeof buffer), &read, nullptr) || !read) break;
                input.append(buffer, static_cast<qsizetype>(read));
                qsizetype newline = -1;
                while ((newline = input.indexOf('\n')) >= 0 && !session.closed()) {
                    const auto frame = input.left(newline); input.remove(0, newline + 1);
                    for (const auto &message : session.request(frame)) writeFrame(message);
                }
                if (input.size() > 32 * 1024) { writeFrame(failure(0, Error("BRIDGE_REQUEST_TOO_LARGE", "Request exceeds its limit."))); break; }
            } else QThread::msleep(5);
        }
    } catch (const Error &) { session.cancel(); return 2; }
    session.cancel(); return 0;
}
}
