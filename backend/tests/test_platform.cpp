#include "platform.h"
#include "platform_protocol_p.h"
#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QTemporaryDir>
#include <QtTest>
#include <thread>

using namespace cpg;
namespace {
QJsonObject application(const QString &id = "App", const QString &executable = "app\\ChatGPT.exe")
{
    return {{"application_id", id}, {"manifest_executable", executable},
        {"entry_point", "Windows.FullTrustApplication"}, {"runtime_behavior", QJsonValue::Null},
        {"trust_level", QJsonValue::Null}};
}
QJsonObject record(const QString &root, const QString &package = "OpenAI.Codex")
{
    return {{"package_name", package}, {"package_version", "1.0.0.0"}, {"architecture", "x64"},
        {"package_full_name", package + "_1.0.0.0_x64__fixture"},
        {"package_family_name", package + "_fixture"}, {"install_location", root},
        {"applications", QJsonArray{application(), application("CommandRunner", "app/runner.exe")}}};
}
QByteArray envelope(const QJsonArray &records)
{
    return QJsonDocument(QJsonObject{{"schema_version", 1}, {"records", records}}).toJson(QJsonDocument::Compact);
}
bool createDesktop(const QString &root)
{
    if (!QDir(root).mkpath("app")) return false;
    QFile file(root + "/app/ChatGPT.exe");
    return file.open(QIODevice::WriteOnly) && file.write("fixture") == 7;
}
QString discoveryFailure(const QByteArray &bytes, const QString &path = {})
{
    try { (void)parseDiscovery(bytes, path); }
    catch (const Error &error) { return error.code; }
    return {};
}
QByteArray encode(const QJsonObject &object) { return QJsonDocument(object).toJson(QJsonDocument::Compact); }
QJsonObject request()
{
    return {{"version", 1}, {"aumid", "OpenAI.Codex_fixture!App"},
        {"expected_package_full_name", "OpenAI.Codex_1.0.0.0_x64__fixture"},
        {"expected_executable", "C:/fixture/app/ChatGPT.exe"},
        {"arguments", "--proxy-server=\"http://127.0.0.1:10808\""}};
}
QJsonObject receipt()
{
    return {{"version", 1}, {"failure", QJsonValue::Null}, {"activation_returned", true},
        {"activation_hresult", QJsonValue::Null}, {"pid", 123}, {"instance", "created"},
        {"package_identity", "matched"}, {"observed_package_full_name", "OpenAI.Codex_fixture"},
        {"aumid", "matched"}, {"observed_aumid", "OpenAI.Codex_fixture!App"},
        {"elevation", false}, {"early_exit_code", QJsonValue::Null}, {"observation_failure", QJsonValue::Null}};
}
}
class PlatformTest final : public QObject {
    Q_OBJECT
private slots:
    void discoversRegisteredEntryWithNulls()
    {
        QTemporaryDir temporary;
        QVERIFY(temporary.isValid());
        QVERIFY(createDesktop(temporary.path()));
        const auto desktop = parseDiscovery(envelope({record(temporary.path())}));
        QVERIFY(desktop.registered);
        QCOMPARE(desktop.aumid, "OpenAI.Codex_fixture!App");
        QCOMPARE(desktop.runtimeKind, "full_trust_desktop");
        const auto publicBytes = encode(desktop.publicJson());
        QVERIFY(!publicBytes.contains(temporary.path().toUtf8()));
        QVERIFY(!desktop.publicJson().contains("executable"));
        QVERIFY(!desktop.publicJson().contains("install_location"));
        auto noAttributes = record(temporary.path());
        auto app = application();
        app.remove("runtime_behavior"); app.remove("trust_level");
        noAttributes["applications"] = QJsonArray{app};
        QCOMPARE(parseDiscovery(envelope({noAttributes})).runtimeKind, "full_trust_desktop");
    }
    void codexIsPreferredOverClassic()
    {
        QTemporaryDir temporary;
        QVERIFY(createDesktop(temporary.path()));
        QCOMPARE(parseDiscovery(envelope({record(temporary.path(), "OpenAI.ChatGPT-Desktop"),
            record(temporary.path())})).packageName, "OpenAI.Codex");
    }
    void discoveryProtocolFailsClosed()
    {
        QTemporaryDir temporary;
        QVERIFY(createDesktop(temporary.path()));
        const auto path = temporary.path() + "/app/ChatGPT.exe";
        for (const auto &bytes : {QByteArray(), QByteArray("[]"), QByteArray("{}"),
             QByteArray("{\"schema_version\":1,\"records\":null}"),
             QByteArray("{\"schema_version\":1,\"records\":{}}"),
             QByteArray("{\"schema_version\":1,\"records\":[]}")+QByteArray(1, char(-1))})
            QVERIFY2(!discoveryFailure(bytes, path).isEmpty(), bytes.constData());
        QCOMPARE(discoveryFailure("{\"schema_version\":2,\"records\":[]}", path), "APPX_DISCOVERY_PROTOCOL_UNSUPPORTED");
        QCOMPARE(discoveryFailure(envelope({})), "CODEX_NOT_INSTALLED");
        QVERIFY(!parseDiscovery(envelope({}), path).registered);
        auto bad = record(temporary.path());
        auto app = application(); app["trust_level"] = 7;
        bad["applications"] = QJsonArray{app};
        QCOMPARE(discoveryFailure(envelope({bad}), path), "APPX_DISCOVERY_PROTOCOL_INVALID");
        bad["applications"] = QJsonArray{application(), application()};
        QCOMPARE(discoveryFailure(envelope({bad})), "APPX_APPLICATION_AMBIGUOUS");
        bad = record(temporary.path()); bad["package_family_name"] = "";
        QCOMPARE(discoveryFailure(envelope({bad})), "APPX_METADATA_INCOMPLETE");
    }
    void overridePreservesRegistrationAndRejectsOtherPackageExe()
    {
        QTemporaryDir temporary;
        QVERIFY(createDesktop(temporary.path()));
        const auto data = envelope({record(temporary.path())});
        const auto desktop = parseDiscovery(data, temporary.path() + "/app/ChatGPT.exe");
        QVERIFY(desktop.registered);
        QCOMPARE(desktop.discoverySource, "executable_override");
        QFile other(temporary.path() + "/app/Other.exe");
        QVERIFY(other.open(QIODevice::WriteOnly)); other.close();
        QCOMPARE(discoveryFailure(data, other.fileName()), "APPX_OVERRIDE_INVALID");
    }
    void manifestTraversalIsRejected()
    {
        QTemporaryDir temporary;
        QVERIFY(createDesktop(temporary.path()));
        for (const auto &path : {"../ChatGPT.exe", "/ChatGPT.exe", "C:/ChatGPT.exe", "app/../../ChatGPT.exe"}) {
            auto data = record(temporary.path(), "OpenAI.ChatGPT-Desktop");
            data["applications"] = QJsonArray{application("Desktop", path)};
            QCOMPARE(discoveryFailure(envelope({data})), "APPX_EXECUTABLE_INVALID");
        }
    }
    void activationProtocolHasNoSideEffects()
    {
        QCOMPARE(detail::activationRequest(encode(request())), request());
        for (const auto &field : {"version", "aumid", "expected_package_full_name", "expected_executable", "arguments"}) {
            auto invalid = request(); invalid.remove(field);
            QVERIFY_THROWS_EXCEPTION(Error, detail::activationRequest(encode(invalid)));
        }
        for (const auto &aumid : {"family", "family!", "!App", "family!App extra", "family!App!Other"}) {
            auto invalid = request(); invalid["aumid"] = aumid;
            QVERIFY_THROWS_EXCEPTION(Error, detail::activationRequest(encode(invalid)));
        }
        auto invalid = request(); invalid["arguments"] = "--flag=日本";
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationRequest(encode(invalid)));
        invalid = request(); invalid["arguments"] = QString(4097, 'x');
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationRequest(encode(invalid)));
        invalid = request(); invalid["expected_executable"] = "relative.exe";
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationRequest(encode(invalid)));
        invalid = request(); invalid["extra"] = true;
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationRequest(encode(invalid)));
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationRequest(QByteArray(16385, 'x')));
    }
    void receiptsRejectMalformedFacts()
    {
        QCOMPARE(detail::activationReceipt(encode(receipt())), receipt());
        auto invalid = receipt(); invalid["pid"] = 1.5;
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationReceipt(encode(invalid)));
        invalid = receipt(); invalid["version"] = 2;
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationReceipt(encode(invalid)));
        invalid = receipt(); invalid["activation_returned"] = false;
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationReceipt(encode(invalid)));
        invalid = receipt(); invalid["package_identity"] = "true";
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationReceipt(encode(invalid)));
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationReceipt(encode(receipt()) + encode(receipt())));
        QVERIFY_THROWS_EXCEPTION(Error, detail::activationReceipt(encode(receipt()) + QByteArray(1, char(-1))));
    }
    void workerRejectsMalformedInputBeforeActivation()
    {
        const auto result = runHelper(QString::fromUtf8(TARGET_ENGINE_PATH), {"internal-activate-package"},
            "{\"version\":99}", {}, 5000, 65536);
        QVERIFY(result.exitCode != 0);
        QVERIFY(result.out.isEmpty());
        QVERIFY(result.err.contains("APPX_ACTIVATION_PROTOCOL_INVALID"));
    }
    void ownedHelperCancellationAndTimeout()
    {
        const QString self = QCoreApplication::applicationFilePath();
        Cancellation alreadyCancelled; alreadyCancelled.cancel();
        try { (void)runHelper(self, {"--test-owned-helper-sleep"}, {}, alreadyCancelled, 1000, 1024); QFAIL("Cancellation ignored"); }
        catch (const Error &error) { QCOMPARE(error.code, "LAUNCH_CANCELLED"); }
        try { (void)runHelper(self, {"--test-owned-helper-sleep"}, {}, {}, 100, 1024); QFAIL("Timeout ignored"); }
        catch (const Error &error) { QCOMPARE(error.code, "HELPER_TIMEOUT"); }
        Cancellation cancellation;
        std::jthread canceller([cancellation] { QThread::msleep(100); cancellation.cancel(); });
        try { (void)runHelper(self, {"--test-owned-helper-sleep"}, {}, cancellation, 5000, 1024); QFAIL("Cancellation ignored"); }
        catch (const Error &error) { QCOMPARE(error.code, "LAUNCH_CANCELLED"); }
    }
    void ownedHelperOutputLimitAndSubmission()
    {
        const QString self = QCoreApplication::applicationFilePath();
        try { (void)runHelper(self, {"--test-owned-helper-output"}, {}, {}, 5000, 1024); QFAIL("Output budget ignored"); }
        catch (const Error &error) { QCOMPARE(error.code, "HELPER_OUTPUT_LIMIT"); }
        try { (void)runHelper(self, {"--test-owned-helper-sleep"}, "complete request", {}, 100, 1024,
            QProcessEnvironment::systemEnvironment(), true); QFAIL("Activation timeout ignored"); }
        catch (const Error &error) { QCOMPARE(error.code, "APPX_ACTIVATION_OUTCOME_UNKNOWN"); }
    }
    void normalizedPathsAndCurrentProcess()
    {
        QCOMPARE(normalizedWindowsPath("\\\\?\\C:\\Fixture\\App.exe"), "c:/fixture/app.exe");
        QCOMPARE(normalizedWindowsPath("\\\\?\\UNC\\server\\share\\App.exe"), "//server/share/app.exe");
        Desktop desktop; desktop.executable = QCoreApplication::applicationFilePath();
        const auto state = desktopProcessState(desktop);
        QCOMPARE(state.state, "running");
        QCOMPARE(state.pid, static_cast<quint32>(QCoreApplication::applicationPid()));
    }
};
int main(int argc, char **argv)
{
    QCoreApplication app(argc, argv);
    const auto arguments = app.arguments();
    if (arguments.contains("--test-owned-helper-sleep")) { QThread::msleep(10000); return 0; }
    if (arguments.contains("--test-owned-helper-output")) {
        QFile output; if (!output.open(stdout, QIODevice::WriteOnly)) return 1;
        (void)output.write(QByteArray(65536, 'x')); output.flush(); return 0;
    }
    PlatformTest test;
    return QTest::qExec(&test, argc, argv);
}
#include "test_platform.moc"
