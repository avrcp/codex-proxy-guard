#include "launcher_controller.h"
#include "engine/snapshot.h"

namespace guard {
ErrorAction errorAction(const QString &code) {
    if (code == "CONFIG_INVALID") return ErrorAction::ProxySettings;
    if (code == "CODEX_ALREADY_RUNNING" || code == "CONFIG_CHANGED" || code == "CONFIG_LOCK_FAILED") return ErrorAction::Refresh;
    if (code == "BACKEND_PROXY_REQUIRED_FOR_REPAIR") return ErrorAction::BackendProxy;
    return ErrorAction::None;
}

LauncherController::LauncherController(IEngineClient *engine, QObject *parent)
    : QObject(parent), engine_(engine) {
    connect(engine_, &IEngineClient::ready, this, [this] {
        connected_ = true;
        pending_ = false;
        refresh();
    });
    connect(engine_, &IEngineClient::response, this,
        [this](quint64, const QString &method, const QJsonObject &result) {
        if (closing_) return;
        if (method == "cancel_operation" || method == "shutdown") return;
        pending_ = false;
        if (method == "start_launch") {
            operation_ = static_cast<quint64>(result.value("operation_id").toDouble());
            if (!operation_) { report("BRIDGE_PROTOCOL_INVALID", "Missing operation identity."); return; }
            message_ = "Operation running…";
        } else if (method == "snapshot" || method == "set_proxy" || method == "set_backend_proxy_consent") {
            snapshot_ = result;
            if (receipt_.isEmpty()) message_ = "Local state refreshed";
            const auto error = result.value("error").toObject();
            if (!error.isEmpty()) {
                errorCode_ = error.value("code").toString();
                errorMessage_ = error.value("message").toString();
            }
        }
        emit changed();
    });
    connect(engine_, &IEngineClient::event, this, [this](const QJsonObject &event) {
        if (closing_) return;
        const auto id = static_cast<quint64>(event.value("operation_id").toDouble());
        if (!operation_ || id != operation_) return;
        if (event.value("event") == "operation_state") {
            message_ = event.value("message").toString();
            emit changed();
        } else if (event.value("event") == "operation_finished") {
            operation_ = 0;
            if (!event.value("ok").toBool()) {
                const auto error = event.value("error").toObject();
                report(error.value("code").toString(), error.value("message").toString());
            } else {
                receipt_ = event.value("result").toObject();
                message_ = receipt_.value("launch_method") == "appmodel_activation"
                    ? "Application activated. Network coverage is not verified."
                    : "Process created. Network coverage is not verified.";
            }
            // Keep operation errors visible through the automatic refresh.
            send("snapshot");
        }
    });
    connect(engine_, &IEngineClient::requestFailed, this,
        [this](quint64, const QString &method, const QString &code, const QString &message, bool) {
        if (closing_) return;
        // Completion may legally win the race with a queued cancellation.
        // Its terminal result is authoritative; an obsolete cancel is not a failure.
        if (method == "cancel_operation" && !operation_ && code == "OPERATION_NOT_FOUND") return;
        if (method != "cancel_operation") pending_ = false;
        report(code, message);
    });
    connect(engine_, &IEngineClient::failed, this, [this](const QString &code, const QString &message) {
        connected_ = false;
        pending_ = false;
        operation_ = 0;
        report(code, message);
    });
    connect(engine_, &IEngineClient::stopped, this, [this] {
        connected_ = false;
        pending_ = false;
        operation_ = 0;
        engineStopped_ = true;
        if (closing_) completeCloseIfStopped();
        else emit changed();
    });
}
bool LauncherController::allows(const QString &action) const {
    if (!connected_ || busy() || closing_) return false;
    const auto s = Snapshot::fromJson(snapshot_);
    if (action == "can_launch") return s.canLaunch;
    if (action == "can_edit_proxy") return s.canEditProxy;
    if (action == "can_authorize_backend_proxy") return s.canAuthorizeBackendProxy || s.canRevokeBackendProxy;
    if (action == "can_repair") return s.canRepair;
    return false;
}
void LauncherController::start() {
    if (busy() || connected_ || closing_) return;
    snapshot_ = {};
    receipt_ = {};
    errorCode_.clear(); errorMessage_.clear();
    pending_ = true;
    message_ = "Connecting to engine…";
    emit changed();
    // start() may synchronously report failure and the terminal stop; the
    // previous stop fact must not survive into the new engine lifetime.
    engineStopped_ = false;
    engine_->start();
}
void LauncherController::send(const QString &method, const QJsonObject &params) {
    if (!connected_ || busy() || closing_) return;
    pending_ = true;
    emit changed();
    if (!engine_->request(method, params)) {
        pending_ = false;
        if (errorCode_.isEmpty()) report("BRIDGE_BUSY", "The engine cannot accept this request yet. Refresh when it becomes available.");
        else emit changed();
    }
}
void LauncherController::refresh() {
    if (!connected_ || busy() || closing_) return;
    errorCode_.clear(); errorMessage_.clear();
    receipt_ = {};
    send("snapshot");
}
void LauncherController::setProxy(const QString &host, int port) {
    if (!allows("can_edit_proxy")) return;
    errorCode_.clear(); errorMessage_.clear();
    receipt_ = {};
    send("set_proxy", {{"host", host}, {"port", port}});
}
void LauncherController::setBackendConsent(bool enabled, const QString &token) {
    if (!allows("can_authorize_backend_proxy") || token.isEmpty()) return;
    errorCode_.clear(); errorMessage_.clear();
    receipt_ = {};
    send("set_backend_proxy_consent", {{"enabled", enabled}, {"confirmation_token", token}});
}
void LauncherController::launch(bool repair, const QString &token) {
    if (!allows(repair ? "can_repair" : "can_launch") || (repair && token.isEmpty())) return;
    errorCode_.clear(); errorMessage_.clear();
    receipt_ = {};
    QJsonObject params{{"repair", repair}};
    if (repair) params.insert("confirmation_token", token);
    send("start_launch", params);
}
void LauncherController::cancel() {
    if (operation_ && connected_ && !closing_) {
        message_ = "Cancelling; waiting for the engine result…";
        engine_->request("cancel_operation", {{"operation_id", static_cast<double>(operation_)}});
        emit changed();
    }
}
void LauncherController::close() {
    if (closing_) return;
    closing_ = true;
    message_ = "Closing safely…";
    emit changed();
    engine_->shutdown();
    // The engine may already be in its terminal stopped state (start failure,
    // crash cleanup finished earlier). No further stopped notification will
    // arrive, so the close must complete from the recorded terminal fact.
    completeCloseIfStopped();
}
void LauncherController::completeCloseIfStopped() {
    if (!closing_ || !engineStopped_ || closeCompletionQueued_) return;
    closeCompletionQueued_ = true;
    QMetaObject::invokeMethod(this, [this] {
        emit closed();
    }, Qt::QueuedConnection);
}
void LauncherController::report(const QString &code, const QString &message) {
    errorCode_ = code; errorMessage_ = message;
    message_ = "Action needs attention";
    emit changed();
}
}
