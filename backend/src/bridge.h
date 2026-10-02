#pragma once
#include "launch.h"
#include <future>
#include <optional>
#include <QElapsedTimer>

namespace cpg {
// Candidate validation/authorization is synchronous; exactly one committed
// effect executes off-thread. Results are applied only by the session owner.
class BridgeSession {
public:
    explicit BridgeSession(QString path,
        std::function<Desktop(const Config &, const Cancellation &)> discovery = discoverDesktop,
        std::function<QString()> queryElevation = elevation);
    ~BridgeSession();
    QList<QJsonObject> request(const QByteArray &frame);
    QList<QJsonObject> poll();
    void cancel();
    bool closed() const { return closed_; }
    bool busy() const { return pending_; }
private:
    struct Result {
        Config config;
        bool ready = false;
        bool snapshot = false;
        std::optional<Desktop> desktop;
        ProcessState process;
        QString coverage = "unknown";
        QJsonObject value;
        std::optional<Error> error;
    };
    QString path_;
    std::function<Desktop(const Config &, const Cancellation &)> discovery_;
    std::function<QString()> queryElevation_;
    Result state_;
    bool hello_ = false, closed_ = false, pending_ = false, launching_ = false;
    qint64 lastId_ = 0, pendingId_ = 0, operation_ = 0;
    QString consentToken_, repairToken_, confirmedHome_;
    Config confirmedConfig_;
    Cancellation cancellation_;
    std::future<Result> task_;
    QElapsedTimer timer_;
    void invalidate();
    void confirmations();
    QJsonObject snapshot() const;
    void dispatch(qint64 id, bool launch, std::function<Result()> effect);
    Result refresh(const Cancellation &cancel) const;
};
int runBridge(const QString &configPath);
}
