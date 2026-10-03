#pragma once
#include "engine/engine_bridge.h"
#include <QJsonObject>
#include <QObject>

namespace guard {
enum class ErrorAction { None, ProxySettings, Refresh, BackendProxy };
ErrorAction errorAction(const QString &code);

class LauncherController final : public QObject {
    Q_OBJECT
public:
    explicit LauncherController(IEngineClient *engine, QObject *parent = nullptr);
    const QJsonObject &snapshot() const { return snapshot_; }
    const QJsonObject &receipt() const { return receipt_; }
    bool connected() const { return connected_; }
    bool busy() const { return pending_ || operation_ != 0; }
    bool operating() const { return operation_ != 0; }
    bool closing() const { return closing_; }
    QString message() const { return message_; }
    QString errorCode() const { return errorCode_; }
    QString errorMessage() const { return errorMessage_; }
    bool allows(const QString &action) const;
    void start();
    void refresh();
    void setProxy(const QString &host, int port);
    void setBackendConsent(bool enabled, const QString &token);
    void launch(bool repair = false, const QString &token = {});
    void cancel();
    void close();
signals:
    void changed();
    void closed();
private:
    void send(const QString &method, const QJsonObject &params = {});
    void report(const QString &code, const QString &message);
    void completeCloseIfStopped();
    IEngineClient *engine_;
    QJsonObject snapshot_;
    QJsonObject receipt_;
    bool connected_ = false;
    bool pending_ = false;
    bool closing_ = false;
    bool engineStopped_ = true;
    bool closeCompletionQueued_ = false;
    quint64 operation_ = 0;
    QString message_ = QStringLiteral("Connecting to engine…");
    QString errorCode_;
    QString errorMessage_;
};
}
