#pragma once
#include <QByteArray>
#include <QJsonObject>
#include <QString>
#include <QStringList>
#include <atomic>
#include <memory>
#include <stdexcept>

namespace cpg {
struct Error final : std::runtime_error {
    QString code;
    QString message;
    Error(QString code, QString message);
    QJsonObject json() const;
};
struct Cancellation {
    std::shared_ptr<std::atomic_bool> flag = std::make_shared<std::atomic_bool>(false);
    bool cancelled() const { return flag->load(); }
    void cancel() const { flag->store(true); }
    void check() const;
};
struct Config {
    int version = 2;
    QString scheme = "http";
    QString host = "127.0.0.1";
    int port = 10808;
    QStringList noProxy{"localhost", "127.0.0.1", "::1"};
    QString executableOverride;
    QString cliOverride;
    bool refuseIfRunning = true;
    bool manageBackend = false;
    QString home;
    QString alternateScreen = "auto";
    bool operator==(const Config &) const = default;
    void validate() const;
    QString proxyUrl() const;
    QString noProxyValue() const;
    QByteArray toml() const;
    static Config parse(const QByteArray &bytes);
    static Config load(const QString &path);
    static Config loadOrCreate(const QString &path);
    static QString defaultPath();
    static QString defaultHome();
};
// Opaque held Windows handle; a launch lease excludes all config mutations.
class ConfigLease {
public:
    ConfigLease(const QString &path, const Config &expected);
    ~ConfigLease();
    ConfigLease(const ConfigLease &) = delete;
    ConfigLease &operator=(const ConfigLease &) = delete;
private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
void initializeConfig(const QString &path, const Config &config, bool force);
Config updateProxy(const QString &path, const Config &expected, bool invalid,
                   const QString &host, int port);
Config updateConsent(const QString &path, const Config &expected, bool enable,
                     const QString &confirmedHome);
QString consentHome(const Config &config);
QString inspectProxyEnv(const Config &config); // unknown/not_authorized/pending/current/stale/conflict/invalid/unavailable/unsupported
void prepareProxyEnv(const Config &config);
void revokeProxyEnv(const QString &home);
bool strictUtf8(const QByteArray &bytes);
QJsonObject parseJsonObject(const QByteArray &bytes, qsizetype limit, const QString &errorCode);
QString publicText(const QString &text, int limit = 160);
}
