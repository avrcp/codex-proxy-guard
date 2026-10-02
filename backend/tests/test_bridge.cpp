#include "bridge.h"
#include <QJsonDocument>
#include <QProcess>
#include <QTemporaryDir>
#include <QtTest>
using namespace cpg;
namespace {
QByteArray request(int id, const QString &method, QJsonObject params = {}) {
    return QJsonDocument(QJsonObject{{"schema", 1}, {"id", id}, {"method", method}, {"params", params}}).toJson(QJsonDocument::Compact);
}
QString code(const QList<QJsonObject> &messages) { return messages.first().value("error").toObject().value("code").toString(); }
Desktop noDesktop(const Config &, const Cancellation &) { throw Error("APPX_NOT_FOUND", "No Desktop fixture."); }
QList<QJsonObject> complete(BridgeSession &session) {
    QElapsedTimer timer; timer.start();
    while (timer.elapsed() < 5000) { const auto messages = session.poll(); if (!messages.isEmpty()) return messages; QThread::msleep(1); }
    return {};
}
}
class BridgeTests : public QObject {
    Q_OBJECT
private slots:
    void protocolGate() {
        QTemporaryDir dir; BridgeSession s(dir.filePath("guard.toml"));
        QCOMPARE(code(s.request(request(1, "snapshot"))), "BRIDGE_HELLO_REQUIRED");
        QCOMPARE(s.request(request(2, "hello")).first().value("result").toObject().value("protocol_version").toInt(), 1);
        QCOMPARE(code(s.request(request(2, "hello"))), "BRIDGE_ID_REUSED");
        QCOMPARE(code(s.request(request(3, "hello", {{"secret", "value"}}))), "BRIDGE_PARAMS_INVALID");
        QVERIFY(!s.request(request(4, "shutdown")).isEmpty()); QVERIFY(s.closed());
        QVERIFY(!QFile::exists(dir.filePath("guard.toml")));
    }
    void rejectsUntrustedProtocol() {
        QTemporaryDir dir; BridgeSession s(dir.filePath("guard.toml"));
        const QList<QByteArray> invalid{
            R"({"schema":1,"id":1,"id":2,"method":"hello","params":{}})",
            R"({"schema":1,"id":1.5,"method":"hello","params":{}})",
            R"({"schema":1,"id":9007199254740992,"method":"hello","params":{}})",
            R"({"schema":1,"id":1,"method":"hello","params":{},"extra":true})",
            QByteArray(33000, 'x'), QByteArray("\xff", 1)};
        for (const auto &frame : invalid) QVERIFY(!s.request(frame).first().value("ok").toBool());
    }
    void noUnconfirmedMutations() {
        QTemporaryDir dir; const auto path = dir.filePath("guard.toml");
        initializeConfig(path, {}, false);
        const auto original = Config::load(path);
        BridgeSession s(path); s.request(request(1, "hello"));
        QCOMPARE(code(s.request(request(2, "set_backend_proxy_consent", {{"enabled", true}, {"confirmation_token", "guessed"}}))), "CONFIG_INVALID");
        QCOMPARE(code(s.request(request(3, "start_launch", {{"repair", true}, {"confirmation_token", "guessed"}}))), "CONFIG_INVALID");
        QVERIFY(Config::load(path) == original);
    }
    void realPipeShutdownWithoutEof() {
        QTemporaryDir dir; QProcess process;
        process.start(TARGET_ENGINE_PATH, {"--config", dir.filePath("guard.toml"), "bridge"});
        QVERIFY(process.waitForStarted(5000));
        process.write(request(1, "hello") + '\n' + request(2, "shutdown") + '\n');
        QVERIFY(process.waitForFinished(5000)); QCOMPARE(process.exitCode(), 0);
        const auto lines = process.readAllStandardOutput().trimmed().split('\n'); QCOMPARE(lines.size(), 2);
        QVERIFY(QJsonDocument::fromJson(lines[0]).object().value("ok").toBool());
        QVERIFY(QJsonDocument::fromJson(lines[1]).object().value("result").toObject().value("shutting_down").toBool());
        QVERIFY(!QFile::exists(dir.filePath("guard.toml")));
    }
    void singleUseConsentAndStaleConfig() {
        QTemporaryDir dir; const auto path = dir.filePath("guard.toml");
        Config config; config.home = dir.path(); initializeConfig(path, config, false);
        BridgeSession s(path, noDesktop, [] { return QString("not_elevated"); });
        s.request(request(1, "hello")); QVERIFY(s.request(request(2, "snapshot")).isEmpty());
        auto messages = complete(s); QVERIFY(!messages.isEmpty());
        auto snapshot = messages.first().value("result").toObject();
        const auto proposal = snapshot.value("confirmations").toObject().value("backend_proxy").toObject();
        QCOMPARE(proposal.value("home").toString(), dir.path());
        const auto token = proposal.value("token").toString(); QVERIFY(!token.isEmpty());
        QVERIFY(s.request(request(3, "set_backend_proxy_consent", {{"enabled", true}, {"confirmation_token", token}})).isEmpty());
        QCOMPARE(code(s.request(request(4, "set_proxy", {{"host", "127.0.0.1"}, {"port", 8080}}))), "BRIDGE_BUSY");
        QVERIFY(!complete(s).isEmpty()); QVERIFY(Config::load(path).manageBackend);
        QCOMPARE(code(s.request(request(5, "set_backend_proxy_consent", {{"enabled", false}, {"confirmation_token", token}}))), "CONFIRMATION_REQUIRED");
        QVERIFY(!QFile::exists(dir.filePath(".env")));
        s.request(request(6, "snapshot")); messages = complete(s); QVERIFY(!messages.isEmpty());
        const auto revokeToken = messages.first().value("result").toObject().value("confirmations").toObject().value("backend_proxy").toObject().value("token").toString();
        QVERIFY(!revokeToken.isEmpty());
        auto current = Config::load(path); updateProxy(path, current, false, "localhost", 8080);
        QCOMPARE(code(s.request(request(7, "set_backend_proxy_consent", {{"enabled", false}, {"confirmation_token", revokeToken}}))), "CONFIG_CHANGED");
        QVERIFY(Config::load(path).manageBackend);
    }
    void malformedConfigCanBeRepairedButCannotLaunch() {
        QTemporaryDir dir; const auto path = dir.filePath("guard.toml");
        QFile file(path); QVERIFY(file.open(QIODevice::WriteOnly)); file.write("malformed !"); file.close();
        BridgeSession s(path, noDesktop); s.request(request(1, "hello")); s.request(request(2, "snapshot"));
        auto messages = complete(s); QVERIFY(!messages.isEmpty());
        QCOMPARE(messages.first().value("result").toObject().value("config_readiness").toString(), "repair_required");
        QCOMPARE(code(s.request(request(3, "start_launch", {{"repair", false}}))), "CONFIG_INVALID");
        s.request(request(4, "set_proxy", {{"host", "127.0.0.1"}, {"port", 8080}}));
        messages = complete(s); QVERIFY(!messages.isEmpty()); QVERIFY(messages.first().value("ok").toBool());
        QCOMPARE(Config::load(path).port, 8080);
    }
    void cliRejectsRemovedAndConflictingOptions() {
        for (const auto &args : QList<QStringList>{{"daemon-stop"}, {"launch", "--auto-stop-daemon"}, {"launch", "--refresh-codex-daemon", "--activation-only"}, {"bridge", "--force"}}) {
            QProcess process; process.start(TARGET_ENGINE_PATH, args); QVERIFY(process.waitForFinished(5000)); QVERIFY(process.exitCode() != 0);
        }
    }
};
QTEST_GUILESS_MAIN(BridgeTests)
#include "test_bridge.moc"
