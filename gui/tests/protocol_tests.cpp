#include "engine/protocol.h"
#include "engine/snapshot.h"

#include <QJsonDocument>
#include <QtTest>

using namespace guard;

class ProtocolTests final : public QObject {
    Q_OBJECT
private slots:
    void decodeResponse();
    void fragmentedFrames();
    void strictUtf8_data();
    void strictUtf8();
    void malformedEnvelope_data();
    void malformedEnvelope();
    void boundedFrames();
    void boundedRequests();
    void incompleteEof();
    void safeIdentifiers();
    void unknownEnumsFailClosed();
    void snapshotAndConsent();
};

void ProtocolTests::decodeResponse()
{
    QJsonObject message;
    QString error;
    QVERIFY(decodeMessage(R"({"schema":1,"id":9,"ok":true,"result":{"protocol_version":1}})", &message, &error));
    QCOMPARE(message.value("id").toInt(), 9);
    QCOMPARE(message.value("result").toObject().value("protocol_version").toInt(), 1);
    QVERIFY(decodeMessage(R"({"schema":1,"id":9,"ok":false,"error":{"code":"BRIDGE_BUSY","message":"Busy","retryable":true}})", &message, &error));
    QVERIFY(decodeMessage(R"({"schema":1,"event":"operation_finished","operation_id":1,"ok":true,"result":{}})", &message, &error));
}

void ProtocolTests::fragmentedFrames()
{
    ProtocolDecoder decoder;
    QVector<QJsonObject> messages;
    QString error;
    const QByteArray frame = R"({"schema":1,"id":1,"ok":true,"result":{"text":"中文"}})";
    const auto split = frame.indexOf("中文") + 1;
    QVERIFY(decoder.feed(frame.left(split), &messages, &error));
    QVERIFY(messages.isEmpty());
    QVERIFY(decoder.feed(frame.mid(split) + "\r\n" + frame + "\n", &messages, &error));
    QCOMPARE(messages.size(), 2);
    QCOMPARE(messages.first().value("result").toObject().value("text").toString(), QStringLiteral("中文"));
    QVERIFY(decoder.finish(&error));
}

void ProtocolTests::strictUtf8_data()
{
    QTest::addColumn<QByteArray>("invalid");
    QTest::newRow("continuation") << QByteArray::fromHex("80");
    QTest::newRow("overlong") << QByteArray::fromHex("c0af");
    QTest::newRow("surrogate") << QByteArray::fromHex("eda080");
    QTest::newRow("out-of-range") << QByteArray::fromHex("f4908080");
    QTest::newRow("truncated") << QByteArray::fromHex("e4b8");
}

void ProtocolTests::strictUtf8()
{
    QFETCH(QByteArray, invalid);
    QJsonObject message;
    QString error;
    const auto frame = QByteArray("{\"schema\":1,\"id\":1,\"ok\":true,\"result\":{\"text\":\"")
        + invalid + "\"}}";
    QVERIFY(!decodeMessage(frame, &message, &error));
}

void ProtocolTests::malformedEnvelope_data()
{
    QTest::addColumn<QByteArray>("frame");
    QTest::newRow("array") << QByteArray("[]");
    QTest::newRow("concat") << QByteArray("{}{}");
    QTest::newRow("bad-schema") << QByteArray(R"({"schema":2,"id":1,"ok":true,"result":{}})");
    QTest::newRow("string-schema") << QByteArray(R"({"schema":"1","id":1,"ok":true,"result":{}})");
    QTest::newRow("string-id") << QByteArray(R"({"schema":1,"id":"1","ok":true,"result":{}})");
    QTest::newRow("fraction-id") << QByteArray(R"({"schema":1,"id":1.5,"ok":true,"result":{}})");
    QTest::newRow("zero-id") << QByteArray(R"({"schema":1,"id":0,"ok":true,"result":{}})");
    QTest::newRow("missing-result") << QByteArray(R"({"schema":1,"id":1,"ok":true})");
    QTest::newRow("bad-error") << QByteArray(R"({"schema":1,"id":1,"ok":false,"error":{}})");
    QTest::newRow("mixed-envelope") << QByteArray(R"({"schema":1,"id":1,"event":"operation_state","operation_id":1,"state":"running","message":""})");
    QTest::newRow("unknown-event") << QByteArray(R"({"schema":1,"event":"future","operation_id":1})");
    QTest::newRow("bad-operation") << QByteArray(R"({"schema":1,"event":"operation_finished","operation_id":1,"ok":true})");
}

void ProtocolTests::malformedEnvelope()
{
    QFETCH(QByteArray, frame);
    QJsonObject message;
    QString error;
    QVERIFY(!decodeMessage(frame, &message, &error));
    QVERIFY(!error.isEmpty());
}

void ProtocolTests::boundedFrames()
{
    ProtocolDecoder decoder;
    QVector<QJsonObject> messages;
    QString error;
    QByteArray frame = R"({"schema":1,"id":1,"ok":true,"result":{}})";
    frame.append(QByteArray(MaxResponseBytes - frame.size(), ' '));
    QVERIFY(decoder.feed(frame + '\n', &messages, &error));
    QCOMPARE(messages.size(), 1);
    QVERIFY(!decoder.feed(QByteArray(MaxResponseBytes + 1, 'x'), &messages, &error));
    QVERIFY(!decoder.feed("\n", &messages, &error));
    decoder.reset();
    messages.clear();
    QVERIFY(decoder.feed(frame.left(MaxResponseBytes), &messages, &error));
    QVERIFY(!decoder.feed("x\n", &messages, &error));
}

void ProtocolTests::boundedRequests()
{
    const auto normal = encodeRequest(1, QStringLiteral("snapshot"), {});
    QVERIFY(normal.endsWith('\n'));
    const auto document = QJsonDocument::fromJson(normal);
    QCOMPARE(document.object().value("method").toString(), QStringLiteral("snapshot"));
    QVERIFY(encodeRequest(1, QStringLiteral("set_proxy"), {{"host", QString(MaxRequestBytes, 'x')}}).isEmpty());
    QVERIFY(encodeRequest(0, QStringLiteral("snapshot"), {}).isEmpty());
    QVERIFY(encodeRequest(MaxProtocolId + 1, QStringLiteral("snapshot"), {}).isEmpty());
}

void ProtocolTests::incompleteEof()
{
    ProtocolDecoder decoder;
    QVector<QJsonObject> messages;
    QString error;
    QVERIFY(decoder.feed(R"({"schema":1)", &messages, &error));
    QVERIFY(!decoder.finish(&error));
}

void ProtocolTests::safeIdentifiers()
{
    quint64 id = 0;
    QVERIFY(protocolId(QJsonValue(static_cast<qint64>(MaxProtocolId)), &id));
    QCOMPARE(id, MaxProtocolId);
    QVERIFY(!protocolId(QJsonValue(static_cast<qint64>(MaxProtocolId + 1)), &id));
    QVERIFY(!protocolId(QJsonValue(-1), &id));
    QVERIFY(!protocolId(QJsonValue(true), &id));
}

static QJsonObject validSnapshot()
{
    return {{"config_readiness", "ready"}, {"elevation", "not_elevated"},
        {"launch", QJsonObject{{"method", "appmodel_activation"}}},
        {"desktop", QJsonObject{{"state", "found"}}},
        {"process", QJsonObject{{"state", "stopped"}}},
        {"coverage", QJsonObject{{"state", "pending"}, {"enabled", true}}},
        {"actions", QJsonObject{{"can_launch", true}, {"can_repair", true}, {"can_edit_proxy", true},
                                {"can_authorize_backend_proxy", true}, {"can_revoke_backend_proxy", true}}},
        {"confirmations", QJsonObject{{"backend_proxy", QJsonObject{{"token", "once"}, {"enabled", true},
                                                                   {"home", "C:/example/.codex"}}},
                                      {"repair", QJsonObject{{"token", "repair-once"}}}}}};
}

void ProtocolTests::unknownEnumsFailClosed()
{
    auto object = validSnapshot();
    object.insert("process", QJsonObject{{"state", "future"}});
    object.insert("desktop", QJsonObject{{"state", "future"}});
    object.insert("coverage", QJsonObject{{"state", "future"}});
    const auto snapshot = Snapshot::fromJson(object);
    QCOMPARE(snapshot.processState, ProcessState::Unknown);
    QCOMPARE(snapshot.desktopState, DesktopState::Unknown);
    QCOMPARE(snapshot.coverage, Coverage::Unknown);
    QVERIFY(!snapshot.canLaunch);
    QVERIFY(!snapshot.canRepair);
    QVERIFY(!Snapshot::fromJson({}).canLaunch);
    object = validSnapshot();
    object.insert("elevation", "future");
    QVERIFY(!Snapshot::fromJson(object).canLaunch);
    object = validSnapshot();
    object.insert("launch", QJsonObject{{"method", "future"}});
    QVERIFY(!Snapshot::fromJson(object).canLaunch);
}

void ProtocolTests::snapshotAndConsent()
{
    auto object = validSnapshot();
    auto snapshot = Snapshot::fromJson(object);
    QVERIFY(snapshot.canLaunch);
    QVERIFY(snapshot.canRepair);
    QVERIFY(snapshot.canAuthorizeBackendProxy);
    QVERIFY(!snapshot.canRevokeBackendProxy);
    QCOMPARE(snapshot.backendConfirmationHome, QStringLiteral("C:/example/.codex"));
    QCOMPARE(snapshot.coverage, Coverage::Pending);
    object.insert("confirmations", QJsonObject{});
    snapshot = Snapshot::fromJson(object);
    QVERIFY(!snapshot.canAuthorizeBackendProxy);
    QVERIFY(!snapshot.canRevokeBackendProxy);
    QVERIFY(!snapshot.canRepair);
    object.insert("busy", true);
    snapshot = Snapshot::fromJson(object);
    QVERIFY(!snapshot.canLaunch);
    QVERIFY(!snapshot.canEditProxy);
}

QTEST_GUILESS_MAIN(ProtocolTests)
#include "protocol_tests.moc"
