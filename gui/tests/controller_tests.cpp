#include "controller/launcher_controller.h"
#include "ui/main_window.h"
#include "ui/theme.h"
#include <QDir>
#include <QStyleHints>
#include <QJsonArray>
#include <QPushButton>
#include <QSignalSpy>
#include <QtTest>

using namespace guard;
class FakeEngineClient final : public IEngineClient {
public:
    using IEngineClient::IEngineClient;
    struct Call { QString method; QJsonObject params; };
    QList<Call> calls;
    bool shutdownRequested = false;
    void start() override { emit ready(); }
    quint64 request(const QString &method, const QJsonObject &params = {}) override {
        calls.append({method, params}); return static_cast<quint64>(calls.size());
    }
    void shutdown() override { shutdownRequested = true; emit stopped(); }
    void reply(const QString &method, const QJsonObject &result) { emit response(1, method, result); }
};
static QJsonObject readySnapshot() {
    return {{"config_readiness", "ready"}, {"elevation", "not_elevated"},
        {"desktop", QJsonObject{{"state", "found"}, {"display_name", "ChatGPT Desktop"}, {"package_version", "26.1"}}},
        {"launch", QJsonObject{{"method", "appmodel_activation"}}},
        {"process", QJsonObject{{"state", "stopped"}}},
        {"coverage", QJsonObject{{"state", "not_authorized"}}},
        {"actions", QJsonObject{{"can_launch", true}, {"can_edit_proxy", true}, {"can_authorize_backend_proxy", true}, {"can_repair", true}}},
        {"confirmations", QJsonObject{{"backend_proxy", QJsonObject{{"token", "b1"}, {"enabled", true}, {"home", "C:/scope"}}}, {"repair", QJsonObject{{"token", "r1"}}}}}};
}
class ControllerTests : public QObject {
    Q_OBJECT
private slots:
    void busyBlocksMutationsAndCancelWaitsForResult() {
        FakeEngineClient engine; LauncherController c(&engine); c.start();
        QCOMPARE(engine.calls.last().method, "snapshot");
        QVERIFY(c.busy()); QVERIFY(!c.allows("can_launch"));
        engine.reply("snapshot", readySnapshot());
        QVERIFY(c.allows("can_launch"));
        c.launch(); c.launch(); c.refresh(); c.setProxy("127.0.0.1", 7890);
        QCOMPARE(engine.calls.size(), 2);
        engine.reply("start_launch", {{"operation_id", 7}});
        QVERIFY(c.operating()); c.cancel();
        QCOMPARE(engine.calls.last().method, "cancel_operation");
        QCOMPARE(engine.calls.last().params.value("operation_id").toInt(), 7);
        engine.reply("cancel_operation", {});
        QVERIFY(c.busy());
        emit engine.event({{"event", "operation_finished"}, {"operation_id", 9}, {"ok", true}});
        QVERIFY(c.operating());
        emit engine.event({{"event", "operation_finished"}, {"operation_id", 7}, {"ok", false},
            {"error", QJsonObject{{"code", "APPX_ACTIVATION_OUTCOME_UNKNOWN"}, {"message", "Outcome unknown; do not retry automatically."}}}});
        QCOMPARE(engine.calls.last().method, "snapshot");
        engine.reply("snapshot", readySnapshot());
        QVERIFY(!c.busy()); QCOMPARE(c.errorCode(), "APPX_ACTIVATION_OUTCOME_UNKNOWN");
        QCOMPARE(engine.calls.size(), 4); // Never retries the launch.
    }
    void confirmationsAreExplicitAndNotHomeParameters() {
        FakeEngineClient engine; LauncherController c(&engine); c.start(); engine.reply("snapshot", readySnapshot());
        c.setBackendConsent(true, {}); c.launch(true, {});
        QCOMPARE(engine.calls.size(), 1);
        c.setBackendConsent(true, "b1");
        QCOMPARE(engine.calls.last().params.value("confirmation_token").toString(), "b1");
        QVERIFY(!engine.calls.last().params.contains("home"));
        engine.reply("set_backend_proxy_consent", readySnapshot());
        c.launch(true, "r1");
        QCOMPARE(engine.calls.last().params.value("confirmation_token").toString(), "r1");
        QVERIFY(engine.calls.last().params.value("repair").toBool());
    }
    void terminalResultWinsLateCancelNotFound() {
        FakeEngineClient engine; LauncherController c(&engine); c.start(); engine.reply("snapshot", readySnapshot());
        c.launch(); engine.reply("start_launch", {{"operation_id", 7}}); c.cancel();
        emit engine.event({{"event", "operation_finished"}, {"operation_id", 7}, {"ok", true},
            {"result", QJsonObject{{"launch_method", "appmodel_activation"}, {"pid", 123}}}});
        emit engine.requestFailed(3, "cancel_operation", "OPERATION_NOT_FOUND", "Already finished", false);
        engine.reply("snapshot", readySnapshot());
        QVERIFY(c.errorCode().isEmpty()); QVERIFY(!c.busy());
        QCOMPARE(c.receipt().value("pid").toInt(), 123);
        QVERIFY(c.message().contains("activated"));
    }
    void disconnectBlocksAndRestartRefreshes() {
        FakeEngineClient engine; LauncherController c(&engine); c.start(); engine.reply("snapshot", readySnapshot());
        emit engine.failed("BRIDGE_EXITED", "Engine unavailable. Outcome may be unknown.");
        QVERIFY(!c.connected()); QVERIFY(!c.allows("can_launch"));
        c.launch(); QCOMPARE(engine.calls.size(), 1);
        c.start(); QCOMPARE(engine.calls.last().method, "snapshot");
        engine.reply("snapshot", readySnapshot()); QVERIFY(c.allows("can_launch"));
    }
    void gracefulCloseIgnoresLateResponses() {
        FakeEngineClient engine; LauncherController c(&engine); c.start(); engine.reply("snapshot", readySnapshot());
        c.launch(); engine.reply("start_launch", {{"operation_id", 1}});
        QSignalSpy closed(&c, &LauncherController::closed); c.close();
        QVERIFY(engine.shutdownRequested); QCOMPARE(closed.count(), 1);
        engine.reply("snapshot", readySnapshot());
        QVERIFY(!c.allows("can_launch"));
    }
    void invalidConfigurationCanBeEdited() {
        FakeEngineClient engine; LauncherController c(&engine); c.start();
        auto s = readySnapshot(); s.insert("config_readiness", "repair_required");
        engine.reply("snapshot", s);
        QVERIFY(!c.allows("can_launch")); QVERIFY(c.allows("can_edit_proxy"));
        c.setProxy("remote.example", 443);
        QCOMPARE(engine.calls.last().method, "set_proxy"); // Engine is final validator.
        emit engine.requestFailed(2, "set_proxy", "CONFIG_INVALID", "Loopback required", false);
        QVERIFY(!c.busy()); QVERIFY(!c.allows("can_launch"));
    }
    void errorActionsAreAllowlisted() {
        QCOMPARE(errorAction("CONFIG_INVALID"), ErrorAction::ProxySettings);
        QCOMPARE(errorAction("CODEX_ALREADY_RUNNING"), ErrorAction::Refresh);
        QCOMPARE(errorAction("BACKEND_PROXY_REQUIRED_FOR_REPAIR"), ErrorAction::BackendProxy);
        QCOMPARE(errorAction("BACKEND_PROXY_CONFIG_CONFLICT"), ErrorAction::None);
        QCOMPARE(errorAction("future_error"), ErrorAction::None);
    }
    void enterRespectsFocusedButton() {
        FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
        c.start(); engine.reply("snapshot", readySnapshot()); window.show(); window.activateWindow();
        QPushButton *refresh = nullptr;
        for (auto *button : window.findChildren<QPushButton *>()) {
            if (button->text() == "Refresh") refresh = button;
        }
        QVERIFY(refresh); refresh->setFocus(); QCoreApplication::processEvents();
        QTest::keyClick(&window, Qt::Key_Return);
        QCoreApplication::processEvents();
        QCOMPARE(engine.calls.size(), 2);
        QCOMPARE(engine.calls.last().method, "snapshot");
    }
    void idleWindowCloseDoesNotReenterCloseEvent() {
        FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
        window.show(); QCoreApplication::processEvents();
        QVERIFY(window.isVisible()); window.close();
        QTRY_VERIFY(!window.isVisible());
        QVERIFY(engine.shutdownRequested);
    }
    void windowReflectsEngineState() {
        FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
        auto *launch = window.findChild<QPushButton *>("launch"); QVERIFY(launch); QVERIFY(!launch->isEnabled());
        c.start(); engine.reply("snapshot", readySnapshot()); QVERIFY(launch->isEnabled());
        c.launch(); QVERIFY(!launch->isEnabled());
        QVERIFY(launch->text().contains("Working"));
    }
    void renderReviewFixtures() {
        const auto output = qEnvironmentVariable("CPG_SCREENSHOT_DIR");
        if (output.isEmpty()) QSKIP("Optional visual review capture");
        QDir().mkpath(output);
        FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
        c.start(); auto s = readySnapshot();
        s.insert("proxy", QJsonObject{{"url", "http://127.0.0.1:10808"}});
        auto desktop = s.value("desktop").toObject();
        desktop.insert("package_version", "26.924.2738.0");
        desktop.insert("manifest_executable", "app/ChatGPT.exe"); s.insert("desktop", desktop);
        engine.reply("snapshot", s);
        qInfo() << "Render device pixel ratio:" << window.devicePixelRatioF();
        for (const auto scheme : {Qt::ColorScheme::Light, Qt::ColorScheme::Dark}) {
            QGuiApplication::styleHints()->setColorScheme(scheme);
            applyTheme(*qobject_cast<QApplication *>(QCoreApplication::instance()));
            for (const auto size : {QSize(520,430), QSize(480,400)}) {
                window.resize(size); window.show(); QCoreApplication::processEvents();
                const auto file = QString("/%1-%2.png").arg(scheme == Qt::ColorScheme::Dark ? "dark" : "light").arg(size.width());
                QVERIFY(window.grab().save(output + file));
                QVERIFY(window.width() <= size.width());
            }
        }
    }
};
QTEST_MAIN(ControllerTests)
#include "controller_tests.moc"
