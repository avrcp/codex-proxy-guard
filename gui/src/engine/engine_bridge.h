#pragma once

#include "protocol.h"

#include <QElapsedTimer>
#include <QHash>
#include <QObject>
#include <QProcess>
#include <QTimer>

namespace guard {

class IEngineClient : public QObject {
    Q_OBJECT
public:
    using QObject::QObject;
    virtual void start() = 0;
    virtual quint64 request(const QString &method, const QJsonObject &params = {}) = 0;
    virtual void shutdown() = 0;

signals:
    void ready();
    void response(quint64 id, const QString &method, const QJsonObject &result);
    void requestFailed(quint64 id, const QString &method, const QString &code,
                       const QString &message, bool retryable);
    void event(const QJsonObject &object);
    void failed(const QString &code, const QString &message);
    void stopped();
};

class EngineBridge final : public IEngineClient {
    Q_OBJECT
public:
    explicit EngineBridge(QObject *parent = nullptr);
    // Test seam only: production always uses the fixed, adjacent bundled engine.
    EngineBridge(const QString &absoluteTestEnginePath, QObject *parent);
    ~EngineBridge() override;
    void start() override;
    quint64 request(const QString &method, const QJsonObject &params = {}) override;
    void shutdown() override;

private:
    struct Pending { QString method; qint64 deadline; };
    quint64 send(const QString &method, const QJsonObject &params);
    void readOutput();
    void readDiagnostics();
    void receive(const QJsonObject &message);
    void fail(const QString &code, const QString &message);
    void checkDeadlines();
    void stopProcess();
    void notifyStopped();

    const QString enginePath_;
    QProcess process_;
    ProtocolDecoder decoder_;
    QByteArray diagnosticRing_;
    QHash<quint64, Pending> pending_;
    QElapsedTimer elapsed_;
    QTimer timer_;
    quint64 nextId_ = 1;
    quint64 operationId_ = 0;
    qint64 operationDeadline_ = 0;
    qint64 startDeadline_ = 0;
    qint64 stopDeadline_ = 0;
    bool ready_ = false;
    bool stopping_ = false;
    bool failed_ = false;
    bool killed_ = false;
    bool stoppedNotified_ = false;
};

} // namespace guard
