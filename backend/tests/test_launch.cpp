#include "launch_internal.h"
#include <QDir>
#include <QFile>
#include <QTemporaryDir>
#include <QThread>
#include <QtTest>
#include <cstdio>
#include <thread>

using namespace cpg;
namespace {
QJsonObject cleanWorker() {
    return {{"version", 1}, {"activation_returned", true}, {"pid", 42},
            {"activation_hresult", QJsonValue::Null}, {"failure", QJsonValue::Null},
            {"early_exit_code", QJsonValue::Null}, {"observation_failure", QJsonValue::Null},
            {"instance", "created"}, {"package_identity", "matched"}, {"aumid", "matched"},
            {"elevation", false}};
}
struct Fixture {
    Config config;
    Cancellation cancel;
    detail::LaunchServices services;
    Desktop desktop;
    QStringList calls;
    QJsonObject worker = cleanWorker();
    Fixture() {
        desktop.registered = true;
        desktop.executable = "C:/fixture/Desktop.exe";
        desktop.packageFullName = "OpenAI.Codex_1.0_x64__test";
        desktop.packageFamilyName = "OpenAI.Codex_test";
        desktop.applicationId = "App";
        desktop.manifestExecutable = "app/Desktop.exe";
        desktop.aumid = "OpenAI.Codex_test!App";
        desktop.runtimeKind = "full_trust_desktop";
        services.requireElevation = [&] { calls.append("elevation"); };
        services.discover = [&] { calls.append("discover"); return desktop; };
        services.process = [&](const Desktop &) { calls.append("process"); return ProcessState{"stopped", 0}; };
        services.prepare = [&] { calls.append("prepare"); };
        services.resolve = [&] { calls.append("resolve"); return CodexCli{"C:/fixture/codex.exe", "C:/fixture/home"}; };
        services.stop = [&](const CodexCli &) { calls.append("stop"); return QString("stopped"); };
        services.markSubmitted = [&] { calls.append("mark"); };
        services.activate = [&](const Desktop &, const QString &arguments) {
            calls.append(arguments.isEmpty() ? "activate_empty" : "activate"); return worker;
        };
        services.native = [&](const Desktop &, const QProcessEnvironment &) {
            calls.append("native"); return QJsonObject{{"pid", 43}};
        };
    }
    void authorize() { config.manageBackend = true; config.home = "C:/fixture/home"; }
    QJsonObject run(LaunchOptions options = {}) { return detail::launchWith(config, options, cancel, services); }
};
QString failure(const std::function<void()> &action) {
    try { action(); } catch (const Error &error) { return error.code; }
    return {};
}
void writeFixture(const QString &path) {
    if (!QDir().mkpath(QFileInfo(path).absolutePath())) qFatal("Cannot create fixture directory");
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly) || file.write("fixture, never executed") < 0)
        qFatal("Cannot write fixture");
}
class HelperMode final {
public:
    explicit HelperMode(const QByteArray &value) : previous_(qgetenv("CPG_LAUNCH_TEST_HELPER")),
        existed_(qEnvironmentVariableIsSet("CPG_LAUNCH_TEST_HELPER")) { qputenv("CPG_LAUNCH_TEST_HELPER", value); }
    ~HelperMode() {
        if (existed_) qputenv("CPG_LAUNCH_TEST_HELPER", previous_);
        else qunsetenv("CPG_LAUNCH_TEST_HELPER");
    }
private:
    QByteArray previous_;
    bool existed_;
};
}

class LaunchTests final : public QObject {
    Q_OBJECT
private slots:
    void proxyArguments() {
        Config config;
        QCOMPARE(proxyPlan(config).arguments,
                 "--proxy-server=\"http://127.0.0.1:10808\" --proxy-bypass-list=\"localhost;127.0.0.1;[::1]\"");
        config.host = "::1";
        QCOMPARE(proxyPlan(config).endpoint, "http://[::1]:10808");
        for (const QString &entry : {QString("*"), QString("example.com"), QString("8.8.8.8"), QString("<local>")}) {
            config.noProxy = {entry};
            QVERIFY(!failure([&] { proxyPlan(config); }).isEmpty());
        }
    }
    void environmentContract() {
        Config config;
        const auto environment = proxyEnvironment(config);
        QCOMPARE(environment.value("HTTP_PROXY"), config.proxyUrl());
        QCOMPARE(environment.value("HTTPS_PROXY"), config.proxyUrl());
        QCOMPARE(environment.value("NO_PROXY"), config.noProxyValue());
        QVERIFY(!environment.contains("ALL_PROXY"));
    }
    void daemonStatusStrict() {
        QCOMPARE(detail::daemonStopStatus("{\"status\":\"stopped\"}"), "stopped");
        QCOMPARE(detail::daemonStopStatus(" {\"status\":\"notRunning\",\"extra\":true}\n"), "not_needed");
        for (const QByteArray &output : {QByteArray(), QByteArray("not json"), QByteArray("[]"),
             QByteArray("{\"status\":false}"), QByteArray("{\"status\":\"Stopped\"}"),
             QByteArray("{\"status\":\"running\"}"), QByteArray("{\"status\":\"not_running\"}"),
             QByteArray("{\"status\":\"stopped\"}{}"), QByteArray("{\"status\":\"stopped\",\"status\":\"stopped\"}"),
             QByteArray("{\"status\":\"stopped\",\"x\":\"\xff\"}"), QByteArray(65537, ' ')})
            QVERIFY2(detail::daemonStopStatus(output).isEmpty(), output.constData());
    }
    void normalNeverTouchesCli() {
        Fixture f;
        f.config.cliOverride = "Z:/does-not-exist/codex.exe";
        const auto receipt = f.run();
        QCOMPARE(receipt.value("daemon_preparation").toString(), "skipped");
        QCOMPARE(receipt.value("backend_proxy_config").toString(), "not_authorized");
        QCOMPARE(receipt.value("proxy_delivery").toString(), "activation_arguments");
        QVERIFY(!f.calls.contains("resolve"));
        QVERIFY(!f.calls.contains("stop"));
        QVERIFY(!f.calls.contains("prepare"));
        QCOMPARE(f.calls.count("activate"), 1);
        QVERIFY(!f.calls.contains("native"));
    }
    void repairPreparesThenStopsOnce() {
        Fixture f;
        f.authorize();
        const auto receipt = f.run({true, false});
        QCOMPARE(receipt.value("backend_proxy_config").toString(), "prepared");
        QCOMPARE(receipt.value("daemon_preparation").toString(), "stopped");
        QVERIFY(f.calls.indexOf("prepare") < f.calls.indexOf("resolve"));
        QVERIFY(f.calls.indexOf("resolve") < f.calls.indexOf("stop"));
        QVERIFY(f.calls.indexOf("stop") < f.calls.indexOf("activate"));
        QCOMPARE(f.calls.count("stop"), 1);
        QCOMPARE(f.calls.count("discover"), 3);
        QCOMPARE(f.calls.count("process"), 3);
    }
    void authorizedNormalPreparesWithoutDaemon() {
        Fixture f;
        f.authorize();
        const auto receipt = f.run();
        QCOMPARE(receipt.value("proxy_delivery").toString(), "activation_arguments_and_home_config");
        QCOMPARE(f.calls.count("prepare"), 1);
        QVERIFY(!f.calls.contains("resolve"));
        QVERIFY(!f.calls.contains("stop"));
    }
    void invalidIdentityFailsBeforePreparation() {
        Fixture f;
        f.authorize();
        f.desktop.aumid = "other!App";
        QCOMPARE(failure([&] { f.run({true, false}); }), "APPX_METADATA_INCOMPLETE");
        QVERIFY(!f.calls.contains("prepare"));
        QVERIFY(!f.calls.contains("resolve"));
        QVERIFY(!f.calls.contains("mark"));
    }
    void repairRequiresConsent() {
        Fixture f;
        QCOMPARE(failure([&] { f.run({true, false}); }), "BACKEND_PROXY_REQUIRED_FOR_REPAIR");
        QVERIFY(!f.calls.contains("resolve"));
        QVERIFY(!f.calls.contains("stop"));
    }
    void envFailureDoesNotInterruptDaemon() {
        Fixture f;
        f.authorize();
        f.services.prepare = [] { throw Error("BACKEND_PROXY_CONFLICT", "fixture conflict"); };
        QCOMPARE(failure([&] { f.run({true, false}); }), "BACKEND_PROXY_CONFLICT");
        QVERIFY(!f.calls.contains("resolve"));
        QVERIFY(!f.calls.contains("stop"));
        QVERIFY(!f.calls.contains("activate"));
    }
    void failedStopDoesNotLaunch() {
        Fixture f;
        f.authorize();
        f.services.stop = [](const CodexCli &) -> QString { throw Error("CODEX_DAEMON_STOP_FAILED", "fixture"); };
        QCOMPARE(failure([&] { f.run({true, false}); }), "CODEX_DAEMON_STOP_FAILED");
        QVERIFY(!f.calls.contains("mark"));
        QVERIFY(!f.calls.contains("activate"));
    }
    void cancellationAfterStopDoesNotLaunch() {
        Fixture f;
        f.authorize();
        f.services.stop = [&](const CodexCli &) { f.cancel.cancel(); return QString("stopped"); };
        QCOMPARE(failure([&] { f.run({true, false}); }), "LAUNCH_CANCELLED");
        QVERIFY(!f.calls.contains("mark"));
    }
    void changedPackageAfterStopDoesNotLaunch() {
        Fixture f;
        f.authorize();
        f.services.stop = [&](const CodexCli &) { f.desktop.packageFullName += "_changed"; return QString("stopped"); };
        QCOMPARE(failure([&] { f.run({true, false}); }), "APPX_PACKAGE_CHANGED");
        QVERIFY(!f.calls.contains("activate"));
    }
    void runningAndUnknownRefused() {
        for (const QString &state : {QString("running"), QString("unknown")}) {
            Fixture f;
            f.config.refuseIfRunning = false;
            f.services.process = [&](const Desktop &) { return ProcessState{state, 9}; };
            QVERIFY(!failure([&] { f.run(); }).isEmpty());
            QVERIFY(!f.calls.contains("prepare"));
            QVERIFY(!f.calls.contains("mark"));
        }
    }
    void runningAfterStopRefused() {
        Fixture f;
        f.authorize();
        bool stopped = false;
        f.services.stop = [&](const CodexCli &) { stopped = true; return QString("stopped"); };
        f.services.process = [&](const Desktop &) { return ProcessState{stopped ? "running" : "stopped", 0}; };
        QCOMPARE(failure([&] { f.run({true, false}); }), "CODEX_ALREADY_RUNNING");
        QVERIFY(!f.calls.contains("mark"));
    }
    void elevatedFailsBeforeDiscovery() {
        Fixture f;
        f.services.requireElevation = [] { throw Error("GUARD_ELEVATED", "fixture"); };
        QCOMPARE(failure([&] { f.run(); }), "GUARD_ELEVATED");
        QVERIFY(f.calls.isEmpty());
    }
    void activationOnlyIsExplicitlyUnproxied() {
        Fixture f;
        f.authorize();
        const auto receipt = f.run({false, true});
        QCOMPARE(receipt.value("proxy_delivery").toString(), "not_established");
        QCOMPARE(receipt.value("backend_proxy_config").toString(), "not_applicable");
        QVERIFY(receipt.value("proxy_endpoint").isNull());
        QVERIFY(f.calls.contains("activate_empty"));
        QVERIFY(!f.calls.contains("prepare"));
        QVERIFY(!f.calls.contains("resolve"));
        QCOMPARE(failure([&] { f.run({true, true}); }), "INVALID_LAUNCH_OPTIONS");
    }
    void nativeUsesOnlyEnvironment() {
        Fixture f;
        f.desktop.registered = false;
        QString injectedEndpoint;
        f.services.native = [&](const Desktop &, const QProcessEnvironment &env) {
            injectedEndpoint = env.value("HTTP_PROXY");
            f.calls.append("native");
            return QJsonObject{{"pid", 43}};
        };
        const auto receipt = f.run();
        QCOMPARE(injectedEndpoint, f.config.proxyUrl());
        QCOMPARE(receipt.value("proxy_delivery").toString(), "process_environment");
        QVERIFY(f.calls.contains("native"));
        QVERIFY(!f.calls.contains("activate"));
        QVERIFY(!f.calls.contains("prepare"));
        QVERIFY(!f.calls.contains("stop"));
        QCOMPARE(failure([&] { f.run({false, true}); }), "ACTIVATION_ONLY_UNSUPPORTED");
    }
    void activationFailuresNeverRetry() {
        for (const QString &field : {QString("package_identity"), QString("aumid")}) {
            Fixture f;
            f.worker.insert(field, "mismatch");
            QVERIFY(!failure([&] { f.run(); }).isEmpty());
            QCOMPARE(f.calls.count("activate"), 1);
            QVERIFY(!f.calls.contains("native"));
        }
        auto worker = cleanWorker();
        worker.insert("activation_returned", false);
        QCOMPARE(failure([&] { detail::activationReceipt(worker); }), "APPX_ACTIVATION_OUTCOME_UNKNOWN");
        worker = cleanWorker();
        worker.insert("early_exit_code", 0);
        QCOMPARE(failure([&] { detail::activationReceipt(worker); }), "APPX_TARGET_EXITED_EARLY");
    }
    void resolverControlledPaths() {
        QTemporaryDir root;
        QVERIFY(root.isValid());
        const QString cwd = root.filePath("cwd");
        const QString trusted = root.filePath("trusted path");
        writeFixture(cwd + "/codex.exe");
        writeFixture(trusted + "/codex.exe");
        const QString path = ";.;relative;" + cwd + ';' + trusted;
        const auto cli = detail::resolveCliFrom({}, root.path(), path, {}, cwd);
        QCOMPARE(cli.executable, QFileInfo(trusted + "/codex.exe").canonicalFilePath());
        QCOMPARE(failure([&] { detail::resolveCliFrom({}, root.path(), cwd, {}, cwd); }), "CODEX_CLI_UNAVAILABLE");
        QCOMPARE(failure([&] { detail::resolveCliFrom(root.filePath("missing"), root.path(), path, {}, cwd); }), "CODEX_HOME_INVALID");
        QCOMPARE(failure([&] { detail::resolveCliFrom({}, root.path(), path, "codex.exe", cwd); }), "CODEX_CLI_OVERRIDE_INVALID");
        const QString shim = root.filePath("codex.cmd");
        writeFixture(shim);
        QCOMPARE(failure([&] { detail::resolveCliFrom({}, root.path(), path, shim, cwd); }), "CODEX_CLI_OVERRIDE_INVALID");
    }
    void resolverHomeOrderAndPinning() {
        QTemporaryDir root;
        QVERIFY(root.isValid());
        const QString home = root.filePath("home");
        const QString standalone = home + "/packages/standalone/current/codex.exe";
        const QString daemon = home + "/packages/app-server-daemon/current/bin/codex.exe";
        writeFixture(standalone);
        QCOMPARE(detail::resolveCliFrom("home", {}, {}, {}, root.path()).executable, QFileInfo(standalone).canonicalFilePath());
        writeFixture(daemon);
        const auto cli = detail::resolveCliFrom("home", {}, {}, {}, root.path());
        QCOMPARE(cli.executable, QFileInfo(daemon).canonicalFilePath());
        QCOMPARE(cli.home, QFileInfo(home).canonicalFilePath());
    }
    void publicStopUsesBoundedOwnedFixture() {
        QTemporaryDir root;
        QVERIFY(root.isValid());
        const QString executable = root.filePath("codex.exe");
        QVERIFY(QFile::copy(QCoreApplication::applicationFilePath(), executable));
        const CodexCli cli{executable, root.path()};
        for (const auto &mode : {QByteArray("stopped"), QByteArray("notRunning")}) {
            HelperMode scope(mode);
            QCOMPARE(stopCodexDaemon(cli, Cancellation{}, 5000), mode == "stopped" ? "stopped" : "not_needed");
        }
        for (const auto &mode : {QByteArray("oversize"), QByteArray("invalid"), QByteArray("exit")}) {
            HelperMode scope(mode);
            QCOMPARE(failure([&] { stopCodexDaemon(cli, Cancellation{}, 5000); }), "CODEX_DAEMON_STOP_FAILED");
        }
        {
            HelperMode scope("unsupported");
            QCOMPARE(failure([&] { stopCodexDaemon(cli, Cancellation{}, 5000); }), "CODEX_DAEMON_UNSUPPORTED");
        }
        {
            HelperMode scope("wait");
            QCOMPARE(failure([&] { stopCodexDaemon(cli, Cancellation{}, 100); }), "CODEX_DAEMON_STOP_TIMEOUT");
        }
        {
            HelperMode scope("wait");
            Cancellation cancellation;
            std::thread canceller([&] { QThread::msleep(100); cancellation.cancel(); });
            const QString code = failure([&] { stopCodexDaemon(cli, cancellation, 5000); });
            canceller.join();
            QCOMPARE(code, "LAUNCH_CANCELLED");
        }
    }
};
int main(int argc, char **argv) {
    QCoreApplication application(argc, argv);
    const QStringList arguments = application.arguments();
    const QByteArray mode = qgetenv("CPG_LAUNCH_TEST_HELPER");
    if (QFileInfo(application.applicationFilePath()).baseName() == "codex" && !mode.isEmpty()) {
        // This branch exists only in the test executable copied to a temporary
        // codex.exe. A real Codex CLI is never run by these fixtures.
        if (arguments.mid(1) != QStringList{"app-server", "daemon", "stop"}) return 91;
        QByteArray output;
        int status = 0;
        if (mode == "wait") QThread::msleep(10000);
        else if (mode == "oversize") output = QByteArray(65537, 'x');
        else if (mode == "invalid") output = "{\"status\":\"running\"}";
        else if (mode == "unsupported") { output = "unknown command: daemon"; status = 2; }
        else if (mode == "exit") status = 3;
        else output = "{\"status\":\"" + mode + "\"}";
        if (!output.isEmpty()) std::fwrite(output.constData(), 1, static_cast<size_t>(output.size()), stdout);
        std::fflush(stdout);
        return status;
    }
    LaunchTests tests;
    return QTest::qExec(&tests, argc, argv);
}
#include "test_launch.moc"
