#include "engine_bridge.h"

#include <QCoreApplication>
#include <QDir>
#include <QFileInfo>
#include <QJsonArray>

namespace guard {
namespace {
constexpr qint64 RequestTimeoutMs = 90000;
constexpr qint64 OperationTimeoutMs = 900000;
constexpr qint64 ShutdownTimeoutMs = 20000;
constexpr qsizetype DiagnosticLimit = 64 * 1024;
constexpr qsizetype ReadChunkBytes = 16 * 1024;
}

EngineBridge::EngineBridge(QObject *parent)
    : EngineBridge(QDir(QCoreApplication::applicationDirPath())
                       .absoluteFilePath(QStringLiteral("engine/codex-proxy-guard.exe")), parent)
{
}

EngineBridge::EngineBridge(const QString &absoluteTestEnginePath, QObject *parent)
    : IEngineClient(parent), enginePath_(absoluteTestEnginePath)
{
    process_.setProcessChannelMode(QProcess::SeparateChannels);
#ifdef Q_OS_WIN
    process_.setCreateProcessArgumentsModifier([](QProcess::CreateProcessArguments *args) {
        args->flags |= 0x08000000; // CREATE_NO_WINDOW; never create a shell or console.
    });
#endif
    timer_.setInterval(250);
    connect(&timer_, &QTimer::timeout, this, &EngineBridge::checkDeadlines);
    connect(&process_, &QProcess::started, this, [this] {
        startDeadline_ = 0;
        if (stopping_) {
            send(QStringLiteral("shutdown"), {});
            process_.closeWriteChannel();
        } else {
            send(QStringLiteral("hello"), {});
        }
    });
    connect(&process_, &QProcess::readyReadStandardOutput, this, &EngineBridge::readOutput);
    connect(&process_, &QProcess::readyReadStandardError, this, &EngineBridge::readDiagnostics);
    connect(&process_, &QProcess::errorOccurred, this, [this](QProcess::ProcessError error) {
        if (error == QProcess::FailedToStart) {
            fail(QStringLiteral("ENGINE_UNAVAILABLE"), QStringLiteral("The bundled engine could not start."));
            timer_.stop();
            notifyStopped();
        } else if (!stopping_ && error != QProcess::Crashed) {
            fail(QStringLiteral("ENGINE_IO_FAILED"), QStringLiteral("Communication with the engine failed."));
        }
    });
    connect(&process_, qOverload<int, QProcess::ExitStatus>(&QProcess::finished), this,
            [this](int, QProcess::ExitStatus) {
        readOutput();
        readDiagnostics();
        QString error;
        if (!stopping_ && !failed_) {
            if (!decoder_.finish(&error)) fail(QStringLiteral("BRIDGE_PROTOCOL_INVALID"), error);
            else fail(QStringLiteral("ENGINE_UNAVAILABLE"),
                      QStringLiteral("The engine disconnected. Any submitted launch outcome is unknown; do not retry automatically."));
        }
        ready_ = false;
        pending_.clear();
        operationId_ = 0;
        timer_.stop();
        diagnosticRing_.clear();
        notifyStopped();
    });
}

EngineBridge::~EngineBridge()
{
    // Normal window close waits asynchronously for stopped(). This is only the
    // final application teardown fallback, and touches our bridge child alone.
    if (process_.state() != QProcess::NotRunning) {
        shutdown();
        if (!process_.waitForFinished(static_cast<int>(ShutdownTimeoutMs))) {
            process_.kill();
            process_.waitForFinished(2000);
        }
    }
}

void EngineBridge::start()
{
    if (process_.state() != QProcess::NotRunning) {
        if (stopping_) emit failed(QStringLiteral("ENGINE_STOPPING"),
                                   QStringLiteral("The engine is still shutting down. Restart after cleanup completes."));
        return;
    }
    ready_ = false;
    stopping_ = false;
    failed_ = false;
    killed_ = false;
    stoppedNotified_ = false;
    nextId_ = 1;
    operationId_ = 0;
    operationDeadline_ = 0;
    stopDeadline_ = 0;
    decoder_.reset();
    pending_.clear();
    diagnosticRing_.clear();
    elapsed_.start();
    if (!QFileInfo(enginePath_).isAbsolute() || !QFileInfo(enginePath_).isFile()) {
        fail(QStringLiteral("ENGINE_NOT_FOUND"), QStringLiteral("The bundled engine is missing. Restore the portable package."));
        return;
    }
    process_.setProgram(enginePath_);
    process_.setArguments({QStringLiteral("bridge")});
    process_.setWorkingDirectory(QFileInfo(enginePath_).absolutePath());
    startDeadline_ = elapsed_.elapsed() + 10000;
    timer_.start();
    process_.start();
}

quint64 EngineBridge::request(const QString &method, const QJsonObject &params)
{
    if (!ready_ || stopping_ || failed_) return 0;
    // A fixed method allowlist keeps the GUI transport from becoming a command runner.
    static const QStringList allowed{QStringLiteral("snapshot"), QStringLiteral("set_proxy"),
        QStringLiteral("set_backend_proxy_consent"), QStringLiteral("start_launch"),
        QStringLiteral("cancel_operation")};
    if (!allowed.contains(method)) return 0;
    if (operationId_ != 0 && method != "cancel_operation") return 0;
    // A completion event may beat the cancellation acknowledgement. Permit its
    // read-only refresh without delaying cleanup or starting a second operation.
    if (!pending_.isEmpty()) {
        const bool finishingCancellation = operationId_ == 0 && method == "snapshot"
            && pending_.size() == 1 && pending_.cbegin()->method == "cancel_operation";
        if (!finishingCancellation) return 0;
    }
    return send(method, params);
}

quint64 EngineBridge::send(const QString &method, const QJsonObject &params)
{
    if (process_.state() != QProcess::Running || nextId_ > MaxProtocolId) return 0;
    const quint64 id = nextId_++;
    const auto bytes = encodeRequest(id, method, params);
    if (bytes.isEmpty() || process_.bytesToWrite() + bytes.size() > MaxRequestBytes + 1) {
        fail(QStringLiteral("BRIDGE_REQUEST_TOO_LARGE"), QStringLiteral("The engine request exceeds the protocol boundary."));
        return 0;
    }
    pending_.insert(id, {method, elapsed_.elapsed() + (method == "hello" ? 10000 : RequestTimeoutMs)});
    if (process_.write(bytes) != bytes.size()) {
        pending_.remove(id);
        fail(QStringLiteral("ENGINE_IO_FAILED"), QStringLiteral("The engine request could not be delivered."));
        return 0;
    }
    return id;
}

void EngineBridge::readOutput()
{
    process_.setReadChannel(QProcess::StandardOutput);
    while (process_.bytesAvailable() > 0) {
        const auto bytes = process_.read(ReadChunkBytes);
        if (failed_) continue; // Drain, never display raw protocol or diagnostics.
        QVector<QJsonObject> messages;
        QString error;
        if (!decoder_.feed(bytes, &messages, &error)) {
            fail(QStringLiteral("BRIDGE_PROTOCOL_INVALID"), error);
            continue;
        }
        for (const auto &message : messages) {
            receive(message);
            if (failed_) break;
        }
    }
}

void EngineBridge::readDiagnostics()
{
    process_.setReadChannel(QProcess::StandardError);
    while (process_.bytesAvailable() > 0) {
        diagnosticRing_.append(process_.read(ReadChunkBytes));
        if (diagnosticRing_.size() > DiagnosticLimit)
            diagnosticRing_.remove(0, diagnosticRing_.size() - DiagnosticLimit);
    }
    process_.setReadChannel(QProcess::StandardOutput);
}

void EngineBridge::receive(const QJsonObject &message)
{
    quint64 id = 0;
    if (message.contains("event")) {
        protocolId(message.value("operation_id"), &id);
        if (id != operationId_ || operationId_ == 0) {
            fail(QStringLiteral("BRIDGE_PROTOCOL_INVALID"), QStringLiteral("The engine returned an unexpected operation."));
            return;
        }
        if (message.value("event").toString() == "operation_finished") {
            operationId_ = 0;
            operationDeadline_ = 0;
        }
        if (!stopping_) emit event(message);
        return;
    }
    protocolId(message.value("id"), &id);
    const auto found = pending_.find(id);
    if (found == pending_.end()) {
        fail(QStringLiteral("BRIDGE_PROTOCOL_INVALID"), QStringLiteral("The engine returned an unexpected request identity."));
        return;
    }
    const QString method = found->method;
    pending_.erase(found);
    if (!message.value("ok").toBool()) {
        const auto error = message.value("error").toObject();
        if (method == "hello") {
            fail(QStringLiteral("BRIDGE_VERSION_MISMATCH"), QStringLiteral("Engine / GUI version mismatch."));
        } else if (!stopping_) {
            emit requestFailed(id, method, error.value("code").toString(),
                               error.value("message").toString(), error.value("retryable").toBool(false));
        }
        return;
    }
    const auto result = message.value("result").toObject();
    if (method == "hello") {
        const auto version = result.value("protocol_version");
        const auto capabilities = result.value("capabilities");
        if (!version.isDouble() || version.toDouble() != ProtocolVersion || !capabilities.isArray()) {
            fail(QStringLiteral("BRIDGE_VERSION_MISMATCH"), QStringLiteral("Engine / GUI version mismatch."));
            return;
        }
        const auto capabilityList = capabilities.toArray();
        for (const auto *required : {"snapshot", "set_proxy", "backend_proxy_consent", "launch", "repair", "cancel"}) {
            if (!capabilityList.contains(QString::fromLatin1(required))) {
                fail(QStringLiteral("BRIDGE_VERSION_MISMATCH"), QStringLiteral("The engine lacks required GUI capabilities."));
                return;
            }
        }
        if (!stopping_) { ready_ = true; emit ready(); }
    } else if (method == "start_launch") {
        if (operationId_ != 0 || !protocolId(result.value("operation_id"), &operationId_)) {
            fail(QStringLiteral("BRIDGE_PROTOCOL_INVALID"), QStringLiteral("The engine returned an invalid launch identity."));
            return;
        }
        operationDeadline_ = elapsed_.elapsed() + OperationTimeoutMs;
        if (!stopping_) emit response(id, method, result);
    } else if (!stopping_) {
        emit response(id, method, result);
    }
}

void EngineBridge::fail(const QString &code, const QString &message)
{
    if (failed_) return;
    failed_ = true;
    ready_ = false;
    emit failed(code, message);
    shutdown();
}

void EngineBridge::shutdown()
{
    if (stopping_) return;
    stopping_ = true;
    ready_ = false;
    if (process_.state() == QProcess::NotRunning) { notifyStopped(); return; }
    stopDeadline_ = elapsed_.elapsed() + ShutdownTimeoutMs;
    // shutdown itself cancels the operation and runs the engine's bounded cleanup.
    if (process_.state() == QProcess::Running) {
        send(QStringLiteral("shutdown"), {});
        process_.closeWriteChannel();
    }
    timer_.start();
}

void EngineBridge::checkDeadlines()
{
    const qint64 now = elapsed_.elapsed();
    if (stopping_) {
        if (stopDeadline_ != 0 && now >= stopDeadline_ && !killed_) stopProcess();
        return;
    }
    if (startDeadline_ != 0 && now >= startDeadline_) {
        fail(QStringLiteral("ENGINE_TIMEOUT"), QStringLiteral("The engine did not start within its time limit."));
        return;
    }
    for (auto it = pending_.cbegin(); it != pending_.cend(); ++it) {
        if (now >= it->deadline) {
            fail(QStringLiteral("ENGINE_TIMEOUT"), QStringLiteral("The engine stopped responding. A submitted launch is not retried automatically."));
            return;
        }
    }
    if (operationId_ != 0 && now >= operationDeadline_)
        fail(QStringLiteral("ENGINE_TIMEOUT"), QStringLiteral("The launch time limit expired; cancelling through the engine. Its outcome may be unknown."));
}

void EngineBridge::stopProcess()
{
    killed_ = true;
    failed_ = true;
    emit failed(QStringLiteral("ENGINE_SHUTDOWN_TIMEOUT"),
                QStringLiteral("The engine did not finish cleanup. Only the bridge child is being stopped; any submitted launch outcome is unknown. Check Desktop before retrying."));
    // Only this QProcess-owned bridge child is killed after cooperative shutdown
    // exceeded its deadline. Desktop and daemon handles are never obtained here.
    process_.kill();
}

void EngineBridge::notifyStopped()
{
    if (stoppedNotified_) return;
    stoppedNotified_ = true;
    emit stopped();
}

} // namespace guard
