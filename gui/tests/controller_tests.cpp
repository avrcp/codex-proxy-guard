#include "controller/launcher_controller.h"
#include "ui/main_window.h"
#include "ui/proxy_settings_dialog.h"
#include "ui/theme.h"
#include "ui/widgets/info_row.h"
#include <QDir>
#include <QLabel>
#include <QLineEdit>
#include <QSpinBox>
#include <QStyleHints>
#include <QJsonArray>
#include <QPushButton>
#include <QShortcut>
#include <QSignalSpy>
#include <QtTest>
#include <algorithm>
#include <cmath>

using namespace guard;
class FakeEngineClient final : public IEngineClient {
public:
    using IEngineClient::IEngineClient;
    struct Call { QString method; QJsonObject params; };
    QList<Call> calls;
    bool shutdownRequested = false;
    bool stoppedOnce = false;
    bool deferShutdownStop = false;
    // The real bridge resets its one-shot stop notification on start().
    void start() override { stoppedOnce = false; emit ready(); }
    quint64 request(const QString &method, const QJsonObject &params = {}) override {
        calls.append({method, params}); return static_cast<quint64>(calls.size());
    }
    void shutdown() override {
        shutdownRequested = true;
        if (!deferShutdownStop) notifyStopped();
    }
    void failAndStop(const QString &code, const QString &message) {
        emit failed(code, message);
        notifyStopped();
    }
    void reply(const QString &method, const QJsonObject &result) { emit response(1, method, result); }
private:
    void notifyStopped() {
        if (stoppedOnce) return;
        stoppedOnce = true;
        emit stopped();
    }
};
static QString enginePath(const QString &relative) {
    return QDir(QCoreApplication::applicationDirPath()).absoluteFilePath(relative);
}
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
        QVERIFY(engine.shutdownRequested); QCOMPARE(closed.count(), 0);
        engine.reply("snapshot", readySnapshot());
        QTRY_COMPARE(closed.count(), 1);
        QVERIFY(!c.allows("can_launch"));
    }
    void realEngineStartFailureThenCloseCompletes() {
        // Portable package without the engine binary: start fails and the
        // engine reaches its terminal stopped state long before the user
        // closes the window. The close must still complete exactly once.
        EngineBridge engine(enginePath(QStringLiteral("missing/fake-engine-absent.exe")), {}, nullptr, true);
        LauncherController c(&engine);
        QSignalSpy failed(&engine, &IEngineClient::failed);
        QSignalSpy stopped(&engine, &IEngineClient::stopped);
        QSignalSpy closed(&c, &LauncherController::closed);
        c.start();
        QTRY_COMPARE(failed.count(), 1);
        QTRY_COMPARE(stopped.count(), 1);
        QVERIFY(!c.connected());
        QCOMPARE(closed.count(), 0);
        c.close();
        QTRY_COMPARE(closed.count(), 1);
    }
    void realEngineCrashThenCloseCompletes() {
        EngineBridge engine(enginePath(QStringLiteral("fake_engine.exe")), {}, nullptr, true);
        LauncherController c(&engine);
        QSignalSpy stopped(&engine, &IEngineClient::stopped);
        QSignalSpy closed(&c, &LauncherController::closed);
        c.start();
        QTRY_VERIFY(c.connected());
        QTRY_VERIFY(!c.busy());
        c.setProxy("crash", 7890); // The fake engine exits; failure and stop are reported.
        QTRY_COMPARE(stopped.count(), 1);
        QVERIFY(!c.connected());
        c.close();
        QTRY_COMPARE(closed.count(), 1);
        QCOMPARE(closed.count(), 1);
    }
    void realEngineRestartAfterCrashThenCloseWaitsForNewStop() {
        EngineBridge engine(enginePath(QStringLiteral("fake_engine.exe")), {}, nullptr, true);
        LauncherController c(&engine);
        QSignalSpy ready(&engine, &IEngineClient::ready);
        QSignalSpy stopped(&engine, &IEngineClient::stopped);
        QSignalSpy closed(&c, &LauncherController::closed);
        c.start(); QTRY_VERIFY(c.connected()); QTRY_VERIFY(!c.busy());
        c.setProxy("crash", 7890);
        QTRY_COMPARE(stopped.count(), 1);
        c.start(); // Explicit restart must clear the previous stop fact.
        QTRY_COMPARE(ready.count(), 2);
        QTRY_VERIFY(c.connected()); QTRY_VERIFY(!c.busy());
        c.close();
        QTRY_COMPARE(stopped.count(), 2);
        QTRY_COMPARE(closed.count(), 1);
    }
    void closeBeforeStopWaitsForTerminalStop() {
        FakeEngineClient engine; LauncherController c(&engine); c.start(); engine.reply("snapshot", readySnapshot());
        engine.deferShutdownStop = true; // Engine still cleaning up; no stop notification yet.
        QSignalSpy closed(&c, &LauncherController::closed);
        emit engine.failed("ENGINE_IO_FAILED", "Communication failed; cleanup running.");
        QVERIFY(!c.connected()); // failed alone is not stopped: the child may still be cleaning up.
        c.close();
        QCoreApplication::processEvents();
        QCOMPARE(closed.count(), 0);
        engine.failAndStop("ENGINE_IO_FAILED", "Cleanup finished.");
        QTRY_COMPARE(closed.count(), 1);
    }
    void closeWithoutStartCompletesOnce() {
        FakeEngineClient engine; LauncherController c(&engine);
        QSignalSpy closed(&c, &LauncherController::closed);
        c.close(); c.close();
        QVERIFY(engine.shutdownRequested);
        QCOMPARE(engine.calls.size(), 0);
        QTRY_COMPARE(closed.count(), 1);
        QCoreApplication::processEvents();
        QCOMPARE(closed.count(), 1);
    }
    void duplicateCloseAndLateStopEmitClosedOnce() {
        FakeEngineClient engine; LauncherController c(&engine); c.start(); engine.reply("snapshot", readySnapshot());
        QSignalSpy closed(&c, &LauncherController::closed);
        c.close();
        engine.failAndStop("ENGINE_UNAVAILABLE", "Stopped during close.");
        c.close(); c.close();
        QTRY_COMPARE(closed.count(), 1);
        QCoreApplication::processEvents();
        QCOMPARE(closed.count(), 1);
    }
    void stoppedEngineWindowCloseCompletes() {
        FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
        window.show(); QCoreApplication::processEvents();
        engine.failAndStop("ENGINE_NOT_FOUND", "The bundled engine is missing. Restore the portable package.");
        window.close(); // The window X must actually close even though no stop notification will follow.
        QTRY_VERIFY(!window.isVisible());
    }
    void instanceObservationSurfacesWarnings() {
        FakeEngineClient engine; LauncherController c(&engine); c.start(); engine.reply("snapshot", readySnapshot());
        const auto finish = [&engine](int id, const QString &instance) {
            emit engine.event({{"event", "operation_finished"}, {"operation_id", id}, {"ok", true},
                {"result", QJsonObject{{"launch_method", "appmodel_activation"}, {"pid", 123}, {"instance", instance},
                    {"proxy_delivery", "activation_arguments_and_home_config"}, {"backend_proxy_config", "prepared"},
                    {"daemon_preparation", "skipped"}}}});
        };
        c.launch(); engine.reply("start_launch", {{"operation_id", 3}});
        finish(3, "reused");
        QVERIFY(c.errorCode().isEmpty()); // Observation warning, not a fabricated failure.
        QVERIFY(c.message().contains("existing Desktop instance"));
        QVERIFY(c.message().contains("not verified"));
        QCOMPARE(c.receipt().value("instance").toString(), "reused");
        engine.reply("snapshot", readySnapshot()); // The automatic post-completion refresh must not swallow it.
        QVERIFY(c.message().contains("existing Desktop instance"));
        QVERIFY(MainWindow::receiptSummary(c.receipt()).contains("Reused an existing instance"));
        QVERIFY(MainWindow::receiptSummary(c.receipt()).contains("how configuration was submitted"));

        c.launch(); engine.reply("start_launch", {{"operation_id", 4}});
        finish(4, "unknown");
        QVERIFY(c.errorCode().isEmpty());
        QVERIFY(c.message().contains("could not be confirmed"));
        QVERIFY(MainWindow::receiptSummary(c.receipt()).contains("Unknown; whether a new instance"));

        c.launch(); engine.reply("start_launch", {{"operation_id", 5}});
        finish(5, "created");
        QVERIFY(c.message().contains("Application activated"));
        QVERIFY(!c.message().contains("existing Desktop instance"));
        QVERIFY(MainWindow::receiptSummary(c.receipt()).contains("Created (new instance observed)"));
    }
    void proxySaveFailureKeepsDialogAndInput() {
        FakeEngineClient engine; LauncherController c(&engine); c.start();
        auto snapshot = readySnapshot();
        snapshot.insert("proxy", QJsonObject{{"host", "127.0.0.1"}, {"port", 10808}});
        engine.reply("snapshot", snapshot);
        ProxySettingsDialog dialog(&c, nullptr);
        dialog.show(); QCoreApplication::processEvents();
        auto *host = dialog.findChild<QLineEdit *>("proxyHost");
        auto *port = dialog.findChild<QSpinBox *>("proxyPort");
        QPushButton *save = nullptr;
        for (auto *button : dialog.findChildren<QPushButton *>())
            if (button->text() == "Save") save = button;
        QVERIFY(host && port && save);
        host->setText("192.0.2.1"); port->setValue(7890);
        save->click();
        QCOMPARE(engine.calls.last().method, "set_proxy"); // Engine is the final validator.
        QVERIFY(dialog.isVisible());
        QTest::keyClick(&dialog, Qt::Key_Escape); // Saving blocks Esc/X/Cancel until a bounded result.
        QCoreApplication::processEvents();
        QVERIFY(dialog.isVisible());
        for (const auto &code : {QString("CONFIG_INVALID"), QString("CONFIG_CHANGED"), QString("CONFIG_LOCK_FAILED")}) {
            emit engine.requestFailed(2, "set_proxy", code, "The engine rejected this save.", false);
            QVERIFY(dialog.isVisible()); // The dialog stays open…
            QCOMPARE(host->text(), "192.0.2.1"); QCOMPARE(port->value(), 7890); // …with the user's input.
            // The human message leads in the status text; the machine code is
            // reachable on the same label's tooltip (UI/UX finding #4).
            bool shown = false;
            for (auto *label : dialog.findChildren<QLabel *>())
                shown = shown || (label->text().contains("engine rejected this save")
                                  && label->toolTip().contains(code));
            QVERIFY2(shown, "The rejection must be visible inside the dialog.");
            save->click(); // Retry is an explicit user action from the restored editing state.
            QCOMPARE(engine.calls.last().method, "set_proxy");
        }
        // A confirmed save closes the dialog exactly once.
        emit engine.response(3, "set_proxy", snapshot);
        QTRY_VERIFY(!dialog.isVisible());
        QCOMPARE(engine.calls.size(), 5); // snapshot + four saves, no silent extra submissions.
    }
    void proxySaveOutcomeUnknownOnEngineFailure() {
        FakeEngineClient engine; LauncherController c(&engine); c.start();
        auto snapshot = readySnapshot();
        snapshot.insert("proxy", QJsonObject{{"host", "127.0.0.1"}, {"port", 10808}});
        engine.reply("snapshot", snapshot);
        ProxySettingsDialog dialog(&c, nullptr);
        dialog.show(); QCoreApplication::processEvents();
        auto *host = dialog.findChild<QLineEdit *>("proxyHost");
        QPushButton *save = nullptr;
        for (auto *button : dialog.findChildren<QPushButton *>())
            if (button->text() == "Save") save = button;
        QVERIFY(host && save);
        const int callsBefore = engine.calls.size();
        save->click();
        engine.failAndStop("ENGINE_IO_FAILED", "Engine died during the save.");
        QCoreApplication::processEvents();
        QVERIFY(dialog.isVisible()); // Not closed, not resubmitted.
        QCOMPARE(engine.calls.size(), callsBefore + 1);
        QCOMPARE(host->text(), "127.0.0.1");
        bool unconfirmed = false;
        for (auto *label : dialog.findChildren<QLabel *>())
            unconfirmed = unconfirmed || (label->text().contains("unconfirmed") && label->text().contains("verify"));
        QVERIFY(unconfirmed);
        QTest::keyClick(&dialog, Qt::Key_Escape); // The unknown state allows closing again.
        QTRY_VERIFY(!dialog.isVisible());
    }
    void proxyDialogSurvivesReopenAndLateSignals() {
        FakeEngineClient engine; LauncherController c(&engine); c.start();
        auto snapshot = readySnapshot();
        snapshot.insert("proxy", QJsonObject{{"host", "127.0.0.1"}, {"port", 10808}});
        engine.reply("snapshot", snapshot);
        ProxySettingsDialog dialog(&c, nullptr);
        dialog.show(); QCoreApplication::processEvents();
        QPushButton *save = nullptr;
        for (auto *button : dialog.findChildren<QPushButton *>())
            if (button->text() == "Save") save = button;
        QVERIFY(save);
        save->click(); // Start a save, then close the engine session and hide the dialog.
        emit engine.response(3, "set_proxy", snapshot);
        QTRY_VERIFY(!dialog.isVisible());
        // A stale late signal for a finished session must not crash or reopen.
        emit c.proxySaveFailed("CONFIG_INVALID", "stale");
        QCoreApplication::processEvents();
        QVERIFY(!dialog.isVisible());
        auto applied = snapshot;
        applied.insert("proxy", QJsonObject{{"host", "::1"}, {"port", 7890}});
        engine.reply("snapshot", applied);
        dialog.reset(); // Reopen re-arms from the current snapshot, not the old draft.
        dialog.show(); QCoreApplication::processEvents();
        QCOMPARE(dialog.findChild<QSpinBox *>("proxyPort")->value(), 7890);
        QCOMPARE(dialog.findChildren<QPushButton *>().size(), 2); // No duplicate buttons or connections.
        save->click();
        QCOMPARE(engine.calls.last().params.value("host").toString(), "::1");
    }
    void proxySaveSuccessStillReportsDiscoveryError() {
        FakeEngineClient engine; LauncherController c(&engine); c.start();
        engine.reply("snapshot", readySnapshot());
        QSignalSpy saved(&c, &LauncherController::proxySaveSucceeded);
        QVERIFY(c.setProxy("127.0.0.1", 7890));
        // The refreshed snapshot may carry a discovery error; the save itself succeeded.
        auto result = readySnapshot();
        result.insert("error", QJsonObject{{"code", "DESKTOP_DISCOVERY_FAILED"}, {"message", "Not found."}});
        emit engine.response(2, "set_proxy", result);
        QCOMPARE(saved.count(), 1);
        QCOMPARE(c.errorCode(), "DESKTOP_DISCOVERY_FAILED");
        QVERIFY(!c.busy());
    }
    void doubleSubmitSendsOneRequest() {
        FakeEngineClient engine; LauncherController c(&engine); c.start();
        engine.reply("snapshot", readySnapshot());
        QVERIFY(c.setProxy("127.0.0.1", 7890));
        QVERIFY(!c.setProxy("127.0.0.1", 7891)); // The controller refuses a second concurrent save.
        QCOMPARE(engine.calls.size(), 2); // snapshot + exactly one set_proxy
        QCOMPARE(engine.calls.last().method, "set_proxy");
        QCOMPARE(engine.calls.last().params.value("port").toInt(), 7890);
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
    // Finding #1: a disabled Launch used to be unexplained because the window
    // never surfaced config_readiness or elevation, which the engine already
    // parses. Each blocked precondition must be readable without opening
    // Details.
    void blockedLaunchStatesItsReason() {
        const auto reasonFor = [](const QJsonObject &snapshot) {
            FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
            auto *launch = window.findChild<QPushButton *>("launch");
            c.start(); engine.reply("snapshot", snapshot);
            if (!launch->isEnabled()) return launch->toolTip();
            for (auto *label : window.findChildren<QLabel *>())
                if (label->toolTip().contains("Launch is unavailable")) return label->toolTip();
            return QString();
        };
        auto elevated = readySnapshot(); elevated.insert("elevation", "elevated");
        QVERIFY(reasonFor(elevated).contains("elevated"));
        auto unverified = readySnapshot(); unverified.insert("elevation", "unknown");
        QVERIFY(reasonFor(unverified).contains("verify"));
        auto unready = readySnapshot(); unready.insert("config_readiness", "repair_required");
        QVERIFY(reasonFor(unready).contains("not ready"));
        auto missing = readySnapshot();
        missing.insert("desktop", QJsonObject{{"state", "missing"}, {"display_name", ""}, {"package_version", ""}});
        QVERIFY(reasonFor(missing).contains("not discovered"));
        auto running = readySnapshot();
        running.insert("process", QJsonObject{{"state", "running"}});
        QVERIFY(reasonFor(running).contains("already running"));
        auto oddMethod = readySnapshot();
        oddMethod.insert("launch", QJsonObject{{"method", "brand_new_method"}});
        QVERIFY(reasonFor(oddMethod).contains("does not recognize"));
        // Several blockers at once must all be reported, not just the first.
        auto combined = readySnapshot();
        combined.insert("elevation", "elevated"); combined.insert("config_readiness", "repair_required");
        const auto both = reasonFor(combined);
        QVERIFY(both.contains("elevated")); QVERIFY(both.contains("not ready"));
        // A launchable state states no blocker at all.
        QVERIFY(reasonFor(readySnapshot()).isEmpty());
    }
    // Finding #2/#3: the theme must keep every outline and focus ring at or
    // above the WCAG 2.x non-text threshold, and stay legible in both schemes.
    void themeFillsClearContrastFloors() {
        const auto cr = [](const QColor &a, const QColor &b) {
            const auto channel = [](qreal c) { return c <= 0.04045 ? c / 12.92 : std::pow((c + 0.055) / 1.055, 2.4); };
            const auto lum = [&channel](const QColor &c) {
                return 0.2126 * channel(c.redF()) + 0.7152 * channel(c.greenF()) + 0.0722 * channel(c.blueF());
            };
            const auto la = lum(a), lb = lum(b);
            return (std::max(la, lb) + 0.05) / (std::min(la, lb) + 0.05);
        };
        for (const auto scheme : {Qt::ColorScheme::Light, Qt::ColorScheme::Dark}) {
            QGuiApplication::styleHints()->setColorScheme(scheme);
            applyTheme(*qobject_cast<QApplication *>(QCoreApplication::instance()));
            const auto t = ThemeTokens::system();
            // Every surface the border token lands on must clear 3:1.
            for (const auto &surface : {t.window, t.surface, t.hover})
                QVERIFY2(cr(t.border, surface) >= 3.0, qPrintable(QStringLiteral("border on surface failed")));
            // Focus rings: accent on the control fill, window on the accent fill.
            QVERIFY(cr(t.accent, t.surface) >= 3.0);
            QVERIFY(cr(t.accent, t.hover) >= 3.0);
            QVERIFY(cr(t.window, t.accent) >= 3.0);
            // The progress chunk must stay readable on its track, and the
            // track must stay visible against the window.
            QVERIFY(cr(t.text, t.border) >= 3.0);
            QVERIFY(cr(t.border, t.window) >= 3.0);
            // Body and secondary text keep their 4.5:1 floors.
            for (const auto &surface : {t.window, t.surface, t.hover}) {
                QVERIFY(cr(t.text, surface) >= 4.5);
                QVERIFY(cr(t.secondary, surface) >= 4.5);
            }
        }
        QGuiApplication::styleHints()->setColorScheme(Qt::ColorScheme::Dark);
        applyTheme(*qobject_cast<QApplication *>(QCoreApplication::instance()));
    }
    // Findings #4/#6: the status line must show the human message, must not
    // grow the window, and must flag itself as an error rather than as an
    // idle note.
    void statusLineStaysReadableAndBounded() {
        FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
        window.resize(480, 400); window.show(); QCoreApplication::processEvents();
        c.start(); engine.reply("snapshot", readySnapshot()); QCoreApplication::processEvents();
        auto *status = window.findChild<ElidedLabel *>("status");
        QVERIFY(status);
        emit engine.requestFailed(2, "launch", "ENGINE_SHUTDOWN_TIMEOUT",
            "Activated into an existing Desktop instance; whether this launch's proxy settings were "
            "re-applied is unconfirmed. Exit Desktop fully, then launch from Guard again. Network "
            "coverage is not verified.", false);
        QCoreApplication::processEvents();
        QVERIFY2(window.height() <= 400, "a long status message must not grow the window");
        QVERIFY2(status->accessibleName().contains("existing Desktop instance")
                 || status->toolTip().contains("existing Desktop instance"),
                 "the human message must be reachable, not only the machine code");
    }
    // Finding #12: an unrecognized engine token stays visible instead of being
    // flattened to "Unknown", which hid the very thing a support log needs.
    void unknownEngineTokensAreNotFlattened() {
        FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
        c.start();
        auto unknown = readySnapshot();
        unknown.insert("coverage", QJsonObject{{"state", "brand_new_state"}});
        unknown.insert("launch", QJsonObject{{"method", "brand_new_method"}});
        engine.reply("snapshot", unknown);
        QCoreApplication::processEvents();
        bool sawCoverage = false, sawMethod = false;
        for (auto *label : window.findChildren<ElidedLabel *>()) {
            const auto text = label->accessibleName();
            if (text.contains("brand_new_state")) sawCoverage = true;
            if (text.contains("brand_new_method")) sawMethod = true;
        }
        QVERIFY2(sawCoverage, "an unrecognized coverage state must stay visible");
        QVERIFY2(sawMethod, "an unrecognized launch method must stay visible");
    }
    // Finding #8: the primary action owns the first tab stop; the row-level Edit
    // is reachable by mouse and by accelerator but must not intercept Tab.
    void primaryActionOwnsFirstTabStop() {
        FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
        c.start(); engine.reply("snapshot", readySnapshot());
        window.show(); window.activateWindow(); QCoreApplication::processEvents();
        auto *launch = window.findChild<QPushButton *>("launch");
        QVERIFY(launch);
        QVERIFY2(launch->focusPolicy() != Qt::NoFocus, "Launch must stay keyboard reachable");
        QTRY_COMPARE(window.focusWidget(), static_cast<QWidget *>(launch));
        QPushButton *edit = nullptr;
        for (auto *button : window.findChildren<QPushButton *>())
            if (button->text() == "Edit") edit = button;
        QVERIFY(edit);
        QVERIFY2(edit->focusPolicy() == Qt::NoFocus, "the row Edit must stay out of the tab chain");
    }
    // Finding #7: the About shortcut table is derived from the registered
    // QShortcut objects, so a documented key without a binding — or a binding
    // missing from the table — cannot appear.
    void aboutShortcutsMatchRegisteredBindings() {
        FakeEngineClient engine; LauncherController c(&engine); MainWindow window(&c);
        const auto text = window.aboutText();
        QVERIFY(!text.isEmpty());
        const auto bindings = window.findChildren<QShortcut *>();
        QVERIFY(bindings.size() >= 10);
        for (auto *binding : bindings) {
            const auto key = binding->key().toString(QKeySequence::NativeText);
            QVERIFY2(text.contains(key), qPrintable(QStringLiteral("About table omits bound key: %1").arg(key)));
        }
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
