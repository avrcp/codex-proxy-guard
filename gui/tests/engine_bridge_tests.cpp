#include "engine/engine_bridge.h"

#include <QCoreApplication>
#include <QDir>
#include <QSignalSpy>
#include <QtTest>

using namespace guard;

class EngineBridgeTests final : public QObject {
    Q_OBJECT
private slots:
    void handshakeAndRequest();
    void requestErrorKeepsEngine();
    void crashAndExplicitRestart();
    void invalidTransport_data();
    void invalidTransport();
    void diagnosticsStayOffProtocol();
    void cancelOperation();
    void shutdownActiveOperation();
    void shutdownBeforeStart();
    void unresponsiveChildIsReapedWithUnknownOutcome();
    void protocolFailureThenHungCleanupStillReportsUnknownOutcome();
};

static QString helperPath()
{
    return QDir(QCoreApplication::applicationDirPath()).absoluteFilePath(QStringLiteral("fake_engine.exe"));
}

void EngineBridgeTests::handshakeAndRequest()
{
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy ready(&bridge, &IEngineClient::ready);
    QSignalSpy responses(&bridge, &IEngineClient::response);
    QSignalSpy failures(&bridge, &IEngineClient::failed);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    QCOMPARE(bridge.request("snapshot"), 0ULL);
    bridge.start();
    QTRY_COMPARE(ready.count(), 1);
    const auto id = bridge.request("snapshot");
    QVERIFY(id > 1);
    QTRY_COMPARE(responses.count(), 1);
    QCOMPARE(responses.first().at(0).toULongLong(), id);
    QCOMPARE(responses.first().at(1).toString(), QStringLiteral("snapshot"));
    QVERIFY(failures.isEmpty());
    bridge.shutdown();
    QTRY_COMPARE(stopped.count(), 1);
}

void EngineBridgeTests::requestErrorKeepsEngine()
{
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy ready(&bridge, &IEngineClient::ready);
    QSignalSpy errors(&bridge, &IEngineClient::requestFailed);
    QSignalSpy failures(&bridge, &IEngineClient::failed);
    QSignalSpy responses(&bridge, &IEngineClient::response);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    bridge.start();
    QTRY_COMPARE(ready.count(), 1);
    const auto id = bridge.request("set_proxy", {{"host", "invalid"}});
    QTRY_COMPARE(errors.count(), 1);
    QCOMPARE(errors.first().at(0).toULongLong(), id);
    QCOMPARE(errors.first().at(2).toString(), QStringLiteral("CONFIG_INVALID"));
    QVERIFY(bridge.request("snapshot") != 0);
    QTRY_COMPARE(responses.count(), 1);
    QVERIFY(failures.isEmpty());
    bridge.shutdown();
    QTRY_COMPARE(stopped.count(), 1);
}

void EngineBridgeTests::crashAndExplicitRestart()
{
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy ready(&bridge, &IEngineClient::ready);
    QSignalSpy failures(&bridge, &IEngineClient::failed);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    bridge.start();
    QTRY_COMPARE(ready.count(), 1);
    QVERIFY(bridge.request("set_proxy", {{"host", "crash"}}) != 0);
    QTRY_COMPARE(failures.count(), 1);
    QTRY_COMPARE(stopped.count(), 1);
    QCOMPARE(bridge.request("start_launch", {{"repair", false}}), 0ULL);
    QCOMPARE(ready.count(), 1); // No automatic restart or launch replay.
    bridge.start();
    QTRY_COMPARE(ready.count(), 2);
    bridge.shutdown();
    QTRY_COMPARE(stopped.count(), 2);
}

void EngineBridgeTests::invalidTransport_data()
{
    QTest::addColumn<QString>("scenario");
    QTest::newRow("request-id-mismatch") << QStringLiteral("mismatch");
    QTest::newRow("oversize-frame") << QStringLiteral("oversize");
}

void EngineBridgeTests::invalidTransport()
{
    QFETCH(QString, scenario);
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy ready(&bridge, &IEngineClient::ready);
    QSignalSpy failures(&bridge, &IEngineClient::failed);
    QSignalSpy responses(&bridge, &IEngineClient::response);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    bridge.start();
    QTRY_COMPARE(ready.count(), 1);
    QVERIFY(bridge.request("set_proxy", {{"host", scenario}}) != 0);
    QTRY_COMPARE(failures.count(), 1);
    QCOMPARE(failures.first().at(0).toString(), QStringLiteral("BRIDGE_PROTOCOL_INVALID"));
    QTRY_COMPARE(stopped.count(), 1);
    QVERIFY(responses.isEmpty());
}

void EngineBridgeTests::diagnosticsStayOffProtocol()
{
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy ready(&bridge, &IEngineClient::ready);
    QSignalSpy failures(&bridge, &IEngineClient::failed);
    QSignalSpy responses(&bridge, &IEngineClient::response);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    bridge.start();
    QTRY_COMPARE(ready.count(), 1);
    QVERIFY(bridge.request("set_proxy", {{"host", "stderr"}}) != 0);
    QTRY_COMPARE(responses.count(), 1);
    QVERIFY(responses.first().at(2).toJsonObject().value("safe").toBool());
    QVERIFY(failures.isEmpty());
    bridge.shutdown();
    QTRY_COMPARE(stopped.count(), 1);
}

void EngineBridgeTests::cancelOperation()
{
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy ready(&bridge, &IEngineClient::ready);
    QSignalSpy events(&bridge, &IEngineClient::event);
    QSignalSpy responses(&bridge, &IEngineClient::response);
    QSignalSpy failures(&bridge, &IEngineClient::failed);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    quint64 refreshId = 0;
    connect(&bridge, &IEngineClient::event, &bridge, [&bridge, &refreshId](const QJsonObject &event) {
        if (event.value("event").toString() == "operation_finished")
            refreshId = bridge.request("snapshot");
    });
    bridge.start();
    QTRY_COMPARE(ready.count(), 1);
    QVERIFY(bridge.request("start_launch", {{"repair", false}}) != 0);
    QTRY_COMPARE(events.count(), 1);
    QCOMPARE(bridge.request("snapshot"), 0ULL);
    QVERIFY(bridge.request("cancel_operation", {{"operation_id", 1}}) != 0);
    QTRY_COMPARE(events.count(), 2);
    QCOMPARE(events.last().first().toJsonObject().value("event").toString(), QStringLiteral("operation_finished"));
    QVERIFY(refreshId != 0); // Completion before cancel acknowledgement must still refresh.
    QTRY_COMPARE(responses.count(), 3);
    QVERIFY(failures.isEmpty());
    bridge.shutdown();
    QTRY_COMPARE(stopped.count(), 1);
}

void EngineBridgeTests::shutdownActiveOperation()
{
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy ready(&bridge, &IEngineClient::ready);
    QSignalSpy events(&bridge, &IEngineClient::event);
    QSignalSpy failures(&bridge, &IEngineClient::failed);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    bridge.start();
    QTRY_COMPARE(ready.count(), 1);
    QVERIFY(bridge.request("start_launch", {{"repair", false}}) != 0);
    QTRY_COMPARE(events.count(), 1);
    bridge.shutdown();
    QTRY_COMPARE(stopped.count(), 1);
    QVERIFY(failures.isEmpty());
    QCOMPARE(events.count(), 1); // Closing UI never receives a late business result.
}

void EngineBridgeTests::shutdownBeforeStart()
{
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    bridge.shutdown();
    bridge.shutdown();
    QCOMPARE(stopped.count(), 1);
}

void EngineBridgeTests::unresponsiveChildIsReapedWithUnknownOutcome()
{
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy ready(&bridge, &IEngineClient::ready);
    QSignalSpy responses(&bridge, &IEngineClient::response);
    QSignalSpy failures(&bridge, &IEngineClient::failed);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    bridge.start(); QTRY_COMPARE(ready.count(), 1);
    QVERIFY(bridge.request("set_proxy", {{"host", "hang"}}) != 0);
    QTRY_COMPARE(responses.count(), 1);
    bridge.shutdown();
    QTRY_COMPARE_WITH_TIMEOUT(stopped.count(), 1, 30000);
    QCOMPARE(failures.count(), 1);
    QCOMPARE(failures.first().at(0).toString(), QStringLiteral("ENGINE_SHUTDOWN_TIMEOUT"));
    QVERIFY(failures.first().at(1).toString().contains("unknown"));
}

void EngineBridgeTests::protocolFailureThenHungCleanupStillReportsUnknownOutcome()
{
    EngineBridge bridge(helperPath(), nullptr);
    QSignalSpy ready(&bridge, &IEngineClient::ready);
    QSignalSpy failures(&bridge, &IEngineClient::failed);
    QSignalSpy stopped(&bridge, &IEngineClient::stopped);
    bridge.start(); QTRY_COMPARE(ready.count(), 1);
    QVERIFY(bridge.request("set_proxy", {{"host", "mismatch_hang"}}) != 0);
    QTRY_COMPARE(failures.count(), 1);
    QCOMPARE(failures.first().at(0).toString(), QStringLiteral("BRIDGE_PROTOCOL_INVALID"));
    QTRY_COMPARE_WITH_TIMEOUT(stopped.count(), 1, 30000);
    QCOMPARE(failures.count(), 2);
    QCOMPARE(failures.last().at(0).toString(), QStringLiteral("ENGINE_SHUTDOWN_TIMEOUT"));
}

QTEST_GUILESS_MAIN(EngineBridgeTests)
#include "engine_bridge_tests.moc"
