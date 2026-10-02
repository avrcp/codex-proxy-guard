#include "core.h"
#include <QtTest>
#include <QDir>
#include <QFile>
#include <QTemporaryDir>
#include <windows.h>
#include <functional>

using namespace cpg;
namespace {
void writeBytes(const QString &path, const QByteArray &bytes)
{
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly) || file.write(bytes) != bytes.size()) throw std::runtime_error("fixture write failed");
}
QByteArray readBytes(const QString &path)
{
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly)) throw std::runtime_error("fixture read failed");
    return file.readAll();
}
QString errorCode(const std::function<void()> &operation)
{
    try { operation(); } catch (const Error &error) { return error.code; }
    return {};
}
Config authorized(const QString &home)
{
    Config config; config.manageBackend = true; config.home = home; return config;
}
}

class CoreTests : public QObject {
    Q_OBJECT
private slots:
    void strictTomlAndRoundTrip()
    {
        Config expected;
        expected.executableOverride = QString::fromUtf8("C:/程序/Codex.exe");
        expected.cliOverride = "D:\\tools\\codex.exe";
        expected.manageBackend = true; expected.home = "C:/fixture/.codex";
        QCOMPARE(Config::parse(expected.toml()), expected);
        QCOMPARE(Config::parse("# no fields\n"), Config{});
        QCOMPARE(Config::parse("[proxy]\nhost = '::1'\nport = 0x1ED2\n").port, 7890);
        const QList<QByteArray> invalid{
            "version=1", "version=2.0", "version=true", "version=2147483648", "version=2\nversion=2",
            "legacy=true", "[proxy]\nlegacy=true", "[codex]\nauto_stop=true", "[tui]\nlegacy=true",
            "proxy=[]", "[proxy]\nport='7890'", "[proxy]\nno_proxy=['localhost', 1]",
            "[codex]\nrefuse_if_running='true'", "[tui]\nalternate_screen='Auto'",
            "[proxy]\nport=1979-05-27", "[proxy]\nport=65536", "[proxy]\nport=0",
            QByteArray("# ") + QByteArray::fromHex("c080"), QByteArray(65537, '#')};
        for (const auto &bytes : invalid) QVERIFY2(!errorCode([&] { Config::parse(bytes); }).isEmpty(), bytes.constData());
    }
    void loopbackAndBounds()
    {
        Config config;
        for (const auto &host : QStringList{"localhost", "LOCALHOST", "127.0.0.1", "127.255.2.3", "::1", "0:0:0:0:0:0:0:1"}) {
            config.host = host; QVERIFY(errorCode([&] { config.validate(); }).isEmpty());
        }
        for (const auto &host : QStringList{"", " 127.0.0.1", "localhost ", "127.1", "127.0.0.01", "192.0.2.1", "0.0.0.0", "::", "::ffff:127.0.0.1", "[::1]", "localhost.evil", "localhost@evil"}) {
            config.host = host; QCOMPARE(errorCode([&] { config.validate(); }), "CONFIG_INVALID");
        }
        config = Config{}; config.host = "::1"; QCOMPARE(config.proxyUrl(), "http://[::1]:10808");
        config.scheme = "HTTP"; config.validate(); config.scheme = "socks5";
        QCOMPARE(errorCode([&] { config.validate(); }), "CONFIG_INVALID");
        config = Config{};
        for (const auto &value : QStringList{"", " ", "localhost\n", " a", QString(256, 'a'), QString::fromUtf8("界").repeated(86), QString("a") + QChar::Null}) {
            config.noProxy = {value}; QCOMPARE(errorCode([&] { config.validate(); }), "CONFIG_INVALID");
        }
        config.noProxy = QStringList(32, QString(255, 'a'));
        QCOMPARE(errorCode([&] { config.validate(); }), "CONFIG_INVALID");
        config.noProxy = {"localhost"}; config.validate();
    }
    void strictJson()
    {
        QCOMPARE(parseJsonObject("{\"ok\": [1,true,null,{\"x\":\"y\"}]}", 1000, "JSON_BAD").value("ok").toArray().size(), 4);
        const QList<QByteArray> invalid{
            "[]", "null", "{} trailing", "{\"a\":1,\"a\":2}", "{\"a\":1,\"\\u0061\":2}",
            "{\"outer\":[{\"x\":1,\"x\":2}]}", "{\"a\":}",
            QByteArray("{\"x\":\"") + QByteArray::fromHex("eda080") + "\"}",
            QByteArray("{\"x\":\"") + QByteArray::fromHex("f4908080") + "\"}",
            QByteArray("{\"x\":\"") + QByteArray::fromHex("c080") + "\"}"};
        for (const auto &bytes : invalid) QCOMPARE(errorCode([&] { parseJsonObject(bytes, 1000, "JSON_BAD"); }), "JSON_BAD");
        QVERIFY(!strictUtf8(QByteArray::fromHex("e282")));
        QVERIFY(strictUtf8(QString::fromUtf8("中文🙂").toUtf8()));
        QCOMPARE(errorCode([&] { parseJsonObject("{}", 1, "LIMIT"); }), "LIMIT");
        const QByteArray nested = "{\"a\":" + QByteArray(70, '[') + "0" + QByteArray(70, ']') + "}";
        QCOMPARE(errorCode([&] { parseJsonObject(nested, 1000, "JSON_BAD"); }), "JSON_BAD");
    }
    void publicTextRedactsBeforeTruncation()
    {
        const auto safe = publicText("Authorization: Bearer private-value", 1000);
        QVERIFY(safe.contains("[REDACTED]")); QVERIFY(!safe.contains("private-value"));
        QCOMPARE(publicText("http://alice:password@localhost/?token=abc", 1000), "http://[REDACTED]@localhost/?token=[REDACTED]");
        QCOMPARE(publicText("Cookie: value\nInstall safely", 1000), "Cookie:[REDACTED] Install safely");
        QVERIFY(!publicText("\x1b[31mred\x1b[0m\r\nnext", 1000).contains(QChar(0x1b)));
        QCOMPARE(publicText("123456", 3), "123");
        const QString profile = qEnvironmentVariable("USERPROFILE");
        if (!profile.isEmpty()) QVERIFY(!publicText(profile + "/file", 1000).contains(profile, Qt::CaseInsensitive));
    }
    void leaseExcludesMutationAndDeletion()
    {
        QTemporaryDir root; QVERIFY(root.isValid()); const QString path = root.filePath("guard.toml");
        const Config config; initializeConfig(path, config, false);
        {
            ConfigLease lease(path, config);
            ConfigLease second(path, config);
            QCOMPARE(Config::load(path), config);
            QCOMPARE(errorCode([&] { updateProxy(path, config, false, "127.0.0.1", 7890); }), "CONFIG_LOCK_FAILED");
            QCOMPARE(errorCode([&] { initializeConfig(path, config, true); }), "CONFIG_LOCK_FAILED");
            QFile external(path); QVERIFY(!external.open(QIODevice::WriteOnly)); QVERIFY(!QFile::remove(path));
        }
        const auto updated = updateProxy(path, config, false, "127.0.0.1", 7890);
        QCOMPARE(updated.port, 7890);
        QCOMPARE(errorCode([&] { ConfigLease stale(path, config); }), "CONFIG_CHANGED");
        QCOMPARE(errorCode([&] { updateProxy(path, config, false, "::1", 7891); }), "CONFIG_CHANGED");
        QCOMPARE(errorCode([&] { updateProxy(path, config, true, "::1", 7891); }), "CONFIG_CHANGED");
        QCOMPARE(Config::load(path), updated);
        writeBytes(path, "version=1\n");
        QCOMPARE(updateProxy(path, config, true, "::1", 7891).port, 7891);
        QCOMPARE(Config::load(path).host, "::1");
    }
    void consentAndScope()
    {
        QTemporaryDir root; QVERIFY(root.isValid()); const QString path = root.filePath("guard.toml");
        Config config; config.home = root.filePath("codex-home"); initializeConfig(path, config, false);
        QCOMPARE(inspectProxyEnv(config), "not_authorized");
        QCOMPARE(errorCode([&] { prepareProxyEnv(config); }), "BACKEND_PROXY_SCOPE_UNCONFIRMED");
        QCOMPARE(errorCode([&] { updateConsent(path, config, true, root.filePath("other")); }), "BACKEND_PROXY_SCOPE_UNCONFIRMED");
        config = updateConsent(path, config, true, config.home);
        QVERIFY(config.manageBackend); QCOMPARE(inspectProxyEnv(config), "pending");
        QVERIFY(!QFileInfo::exists(config.home));
        QCOMPARE(errorCode([&] { initializeConfig(path, Config{}, true); }), "BACKEND_PROXY_REVOKE_REQUIRED");
        prepareProxyEnv(config); QCOMPARE(inspectProxyEnv(config), "current");
        const QString home = config.home;
        config = updateConsent(path, config, false, home);
        QVERIFY(!config.manageBackend); QVERIFY(config.home.isEmpty()); QVERIFY(!QFileInfo::exists(QDir(home).filePath(".env")));
        QCOMPARE(Config::load(path), config);
        for (const auto &invalidHome : QStringList{"relative", "C:relative", "\\rooted", "//server", "//?/GLOBALROOT/Device/HarddiskVolume1", "//./C:/device"}) {
            config.home = invalidHome;
            QCOMPARE(errorCode([&] { consentHome(config); }), "BACKEND_PROXY_SCOPE_UNCONFIRMED");
        }
        config.home = "\\\\?\\C:\\Users\\fixture\\.codex";
        QCOMPARE(consentHome(config), config.home);
        config.home = "\\\\?\\UNC\\server\\share\\.codex";
        QCOMPARE(consentHome(config), config.home);
    }
    void envPreservesBytesAndIsIdempotent()
    {
        try {
        QTemporaryDir root; QVERIFY(root.isValid()); auto config = authorized(root.path()); const QString env = root.filePath(".env");
        const QByteArray original = "# user comments\r\nOTHER=value\r\n\r\n";
        writeBytes(env, original);
        prepareProxyEnv(config); const auto first = readBytes(env); QVERIFY(first.startsWith(original));
        prepareProxyEnv(config); QCOMPARE(readBytes(env), first);
        config.port = 7890; QCOMPARE(inspectProxyEnv(config), "stale"); prepareProxyEnv(config);
        QVERIFY(readBytes(env).startsWith(original)); QVERIFY(readBytes(env).contains("HTTP_PROXY=http://127.0.0.1:7890\n"));
        revokeProxyEnv(root.path()); QCOMPARE(readBytes(env), original);
        // A suffix with no final newline is preserved exactly on update/revoke.
        prepareProxyEnv(config); writeBytes(env, readBytes(env) + "TAIL=unchanged");
        config.port = 7891; prepareProxyEnv(config); revokeProxyEnv(root.path());
        QCOMPARE(readBytes(env), original + "TAIL=unchanged");
        writeBytes(env, QByteArray::fromHex("efbbbf") + "OTHER=preserved\r\n");
        const auto withBom = readBytes(env); prepareProxyEnv(config); revokeProxyEnv(root.path());
        QCOMPARE(readBytes(env), withBom);
        } catch (const Error &error) { QFAIL(qPrintable(error.code + ": " + error.message)); }
    }
    void envConflictAndMalformedBlock()
    {
        QTemporaryDir root; QVERIFY(root.isValid()); const auto config = authorized(root.path()); const QString env = root.filePath(".env");
        for (const auto &line : QList<QByteArray>{"HTTP_PROXY=existing\n", " export https_proxy = existing\r\n", "export\tNO_PROXY=existing", "all_proxy=existing", QByteArray::fromHex("efbbbf") + "HTTP_PROXY=existing"}) {
            writeBytes(env, line); QCOMPARE(inspectProxyEnv(config), "conflict");
            QCOMPARE(errorCode([&] { prepareProxyEnv(config); }), "BACKEND_PROXY_CONFIG_CONFLICT"); QCOMPARE(readBytes(env), line);
        }
        const QByteArray begin = "# BEGIN CODEX PROXY GUARD: proxy-v1\n";
        const QByteArray end = "# END CODEX PROXY GUARD: proxy-v1\n";
        for (const auto &block : QList<QByteArray>{begin, end, begin + end, begin + "EXTRA=1\n" + end,
            begin + begin + end, "# BEGIN CODEX PROXY GUARD: proxy-v2\n", "# END CODEX PROXY GUARD: proxy-v2\n"}) {
            writeBytes(env, block); QCOMPARE(inspectProxyEnv(config), "invalid");
            QCOMPARE(errorCode([&] { prepareProxyEnv(config); }), "BACKEND_PROXY_BLOCK_INVALID");
            QCOMPARE(errorCode([&] { revokeProxyEnv(root.path()); }), "BACKEND_PROXY_BLOCK_INVALID"); QCOMPARE(readBytes(env), block);
        }
        writeBytes(env, QByteArray::fromHex("ff")); QCOMPARE(inspectProxyEnv(config), "invalid");
        QCOMPARE(errorCode([&] { prepareProxyEnv(config); }), "BACKEND_PROXY_ENV_ENCODING");
        writeBytes(env, QByteArray(1024 * 1024 + 1, '#')); QCOMPARE(inspectProxyEnv(config), "unavailable");
    }
    void concurrentEnvWriterRefusesReplacement()
    {
        QTemporaryDir root; QVERIFY(root.isValid()); const QString env = root.filePath(".env");
        const auto config = authorized(root.path()); const QByteArray original = "OTHER=preserved\r\n";
        writeBytes(env, original);
        const HANDLE writer = CreateFileW(reinterpret_cast<const wchar_t *>(env.utf16()), GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
        QVERIFY(writer != INVALID_HANDLE_VALUE);
        const auto failure = errorCode([&] { prepareProxyEnv(config); });
        CloseHandle(writer);
        QCOMPARE(failure, "BACKEND_PROXY_ENV_IO"); QCOMPARE(readBytes(env), original);
        QCOMPARE(QDir(root.path()).entryList({".cpg-proxy-env-*.tmp"}, QDir::Files | QDir::Hidden).size(), 0);
        prepareProxyEnv(config); QCOMPARE(inspectProxyEnv(config), "current");
    }
    void failedRevokePreservesAuthorizationAndFile()
    {
        QTemporaryDir root; QVERIFY(root.isValid()); const QString path = root.filePath("guard.toml");
        const QString env = root.filePath(".env"); const Config config = authorized(root.path()); initializeConfig(path, config, false);
        const QByteArray broken = "# BEGIN CODEX PROXY GUARD: proxy-v1\nEXTRA=1\n# END CODEX PROXY GUARD: proxy-v1\n";
        writeBytes(env, broken); const auto originalConfig = readBytes(path);
        QCOMPARE(errorCode([&] { updateConsent(path, config, false, config.home); }), "BACKEND_PROXY_REVOKE_FAILED");
        QCOMPARE(readBytes(env), broken); QCOMPARE(readBytes(path), originalConfig);
        QVERIFY(QFile::remove(env)); prepareProxyEnv(config); const auto validBlock = readBytes(env);
        // A reader refusing delete access prevents the atomic remove. Consent
        // must remain retryable and must not strand a block in an unknown Home.
        const HANDLE locked = CreateFileW(reinterpret_cast<const wchar_t *>(env.utf16()), GENERIC_READ, FILE_SHARE_READ,
                                          nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
        QVERIFY(locked != INVALID_HANDLE_VALUE);
        const auto failure = errorCode([&] { updateConsent(path, config, false, config.home); });
        CloseHandle(locked);
        QCOMPARE(failure, "BACKEND_PROXY_REVOKE_FAILED"); QCOMPARE(readBytes(env), validBlock); QCOMPARE(readBytes(path), originalConfig);
        const auto revoked = updateConsent(path, config, false, config.home); QVERIFY(!revoked.manageBackend);
    }
};
QTEST_GUILESS_MAIN(CoreTests)
#include "test_core.moc"
