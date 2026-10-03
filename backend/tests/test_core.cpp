#include "core.h"
#include <QtTest>
#include <QDir>
#include <QFile>
#include <QJsonDocument>
#include <QProcess>
#include <QTemporaryDir>
#include <windows.h>
#include <functional>

using namespace cpg;
namespace cpg::testing { void setEnvTransactionHook(std::function<void(bool)> hook); }
namespace {
struct EnvHook {
    explicit EnvHook(std::function<void(bool)> hook) { testing::setEnvTransactionHook(std::move(hook)); }
    ~EnvHook() { testing::setEnvTransactionHook({}); }
};
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
    void envSyntaxGateRejectsUnsafeBoundaries()
    {
        QTemporaryDir root; QVERIFY(root.isValid());
        const QString env = root.filePath(".env");
        const QString path = root.filePath("guard.toml");
        const auto config = authorized(root.path());
        initializeConfig(path, config, false);
        const auto originalConfig = readBytes(path);
        const QByteArray begin = "# BEGIN CODEX PROXY GUARD: proxy-v1\n";
        const QByteArray end = "# END CODEX PROXY GUARD: proxy-v1\n";
        const QByteArray inner = "HTTP_PROXY=http://127.0.0.1:10808\nHTTPS_PROXY=http://127.0.0.1:10808\nNO_PROXY=localhost,127.0.0.1,::1\n";
        // A complete marker block that is only the content of a multi-line
        // quoted value is not a managed block, and unterminated quoting never
        // becomes one through appending.
        const QList<QByteArray> unsafe{
            "TEMPLATE='\n" + begin + inner + end + "'\n",
            "TEMPLATE=\"\n" + begin + inner + end + "\"\n",
            "TEMPLATE='unterminated\n",
            "TEMPLATE=\"unterminated",
            "VALUE='esc\\' apen\n",
            "VALUE=tail\\\nNEXT=1\n",
            begin + inner + end + "TEMPLATE='open\n"};
        for (const auto &bytes : unsafe) {
            writeBytes(env, bytes);
            QCOMPARE(inspectProxyEnv(config), "unsupported");
            QCOMPARE(errorCode([&] { prepareProxyEnv(config); }), "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED");
            QCOMPARE(readBytes(env), bytes); // Refusal never touches a byte.
            QString revokeMessage;
            try { updateConsent(path, config, false, config.home); }
            catch (const Error &error) { QCOMPARE(error.code, "BACKEND_PROXY_REVOKE_FAILED"); revokeMessage = error.message; }
            QVERIFY(!revokeMessage.isEmpty());
            QVERIFY2(revokeMessage.contains("Cause: BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED."), qPrintable(revokeMessage));
            QCOMPARE(readBytes(env), bytes);
            QCOMPARE(readBytes(path), originalConfig); // Consent and its bound Home survive for retry.
            QCOMPARE(Config::load(path), config);
        }
        // An existing valid block plus later unsupported syntax refuses the
        // whole edit; Guard never deletes the part that "looks like its own".
        const QByteArray mixed = begin + inner + end + "TEMPLATE='open\n";
        writeBytes(env, mixed);
        QCOMPARE(errorCode([&] { revokeProxyEnv(config.home); }), "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED");
        QCOMPARE(readBytes(env), mixed);
    }
    void envSyntaxGateAcceptsSingleLineQuoting()
    {
        QTemporaryDir root; QVERIFY(root.isValid());
        const auto config = authorized(root.path());
        const QString env = root.filePath(".env");
        const QByteArray safe = "# someone's \"settings\"\r\n"
            "OTHER='single quoted value'\r\n"
            "ANOTHER=\"double \\\"escaped\\\" value\"\r\n"
            "HASHY=literal#hash\r\n"
            "TRAILING=value # trailing comment with 'quote'\r\n"
            "export SIMPLE=plain\r\n";
        writeBytes(env, safe);
        prepareProxyEnv(config);
        QCOMPARE(inspectProxyEnv(config), "current");
        QVERIFY(readBytes(env).startsWith(safe));
        QVERIFY(readBytes(env).contains("HTTP_PROXY=http://127.0.0.1:10808\n"));
        revokeProxyEnv(root.path());
        QCOMPARE(readBytes(env), safe);
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
    void editedManagedValuesPreserveConsentAndBytes()
    {
        QTemporaryDir root; QVERIFY(root.isValid()); const auto config = authorized(root.path());
        const QString env = root.filePath(".env"); const QString path = root.filePath("guard.toml");
        initializeConfig(path, config, false); const auto originalConfig = readBytes(path);
        const QByteArray endpoint = "http://127.0.0.1:10808";
        const QList<QList<QByteArray>> invalid{
            {"http://192.0.2.1:10808", "http://192.0.2.1:10808", "localhost"},
            {endpoint, "http://127.0.0.1:7890", "localhost"},
            {"https://127.0.0.1:10808", "https://127.0.0.1:10808", "localhost"},
            {"http://user:pass@127.0.0.1:10808", "http://user:pass@127.0.0.1:10808", "localhost"},
            {"http://127.0.0.1:00080", "http://127.0.0.1:00080", "localhost"},
            {"http://127.0.0.1:0", "http://127.0.0.1:0", "localhost"},
            {"http://127.0.0.1:65536", "http://127.0.0.1:65536", "localhost"},
            {endpoint, endpoint, ""}, {endpoint, endpoint, " localhost"},
            {endpoint, endpoint, QByteArray("local") + '\0' + "host"},
            {endpoint, endpoint, QByteArray(256, 'a')}, {endpoint, endpoint, QByteArray(4097, 'a')}
        };
        for (const auto &values : invalid) {
            const QByteArray bytes = "# user data\r\n# BEGIN CODEX PROXY GUARD: proxy-v1\nHTTP_PROXY=" + values[0]
                + "\nHTTPS_PROXY=" + values[1] + "\nNO_PROXY=" + values[2] + "\n# END CODEX PROXY GUARD: proxy-v1\n";
            writeBytes(env, bytes);
            QCOMPARE(inspectProxyEnv(config), "invalid");
            QCOMPARE(errorCode([&] { prepareProxyEnv(config); }), "BACKEND_PROXY_BLOCK_INVALID");
            QCOMPARE(errorCode([&] { revokeProxyEnv(config.home); }), "BACKEND_PROXY_BLOCK_INVALID");
            QCOMPARE(errorCode([&] { updateConsent(path, config, false, config.home); }), "BACKEND_PROXY_REVOKE_FAILED");
            QCOMPARE(readBytes(env), bytes); QCOMPARE(readBytes(path), originalConfig);
        }
        QVERIFY(QFile::remove(env));
        Config unusual = config; unusual.host = "0:0:0:0:0:0:0:1";
        unusual.noProxy = {",", QString(240, 'a') + ",,", QString(240, 'b')};
        prepareProxyEnv(unusual); QCOMPARE(inspectProxyEnv(unusual), "current");
        // A valid prior endpoint is stale, not externally corrupted.
        QCOMPARE(inspectProxyEnv(config), "stale"); prepareProxyEnv(config); QCOMPARE(inspectProxyEnv(config), "current");
        revokeProxyEnv(config.home); QVERIFY(!QFileInfo::exists(env));
    }
    void invalidRecoveryNeverErasesConsentEvidence()
    {
        QTemporaryDir root; QVERIFY(root.isValid()); const QString path = root.filePath("guard.toml");
        const QString env = root.filePath(".env"); const auto config = authorized(root.path());
        prepareProxyEnv(config); const auto originalEnv = readBytes(env);
        const QList<QByteArray> invalid{
            "version=1\n[codex]\nmanage_codex_proxy_env=true\nproxy_env_home='C:/existing/home'\n",
            "[proxy\n[codex]\nmanage_codex_proxy_env=true\nproxy_env_home='C:/existing/home'\n",
            "version=2\n[codex]\nproxy_env_home='C:/existing/home'\nBROKEN =\n",
            "version=1\n[codex]\n\"manage_codex_proxy_\\u0065nv\"=true\n",
            "[codex]\n\"proxy_env_\\U00000068ome\"='C:/existing/home'\nBROKEN =\n",
            QByteArray("version=1\n") + QByteArray::fromHex("ff")
        };
        for (const auto &bytes : invalid) {
            writeBytes(path, bytes);
            QCOMPARE(errorCode([&] { updateProxy(path, Config{}, true, "127.0.0.1", 7890); }), "CONFIG_CONSENT_REPAIR_REQUIRED");
            QCOMPARE(errorCode([&] { initializeConfig(path, Config{}, true); }), "CONFIG_CONSENT_REPAIR_REQUIRED");
            QCOMPARE(readBytes(path), bytes); QCOMPARE(readBytes(env), originalEnv);
        }
        // Repair the syntax manually without dropping the binding; the normal
        // confirmed revoke path can then safely remove the block and consent.
        writeBytes(path, config.toml()); const auto revoked = updateConsent(path, config, false, config.home);
        QVERIFY(!revoked.manageBackend); QVERIFY(!QFileInfo::exists(env));
        writeBytes(path, "version=1\n");
        QCOMPARE(updateProxy(path, Config{}, true, "127.0.0.1", 7890).port, 7890);
    }
    void legacyExtendedHomeSupportsActualFileOperations()
    {
        QTemporaryDir root; QVERIFY(root.isValid());
        const QString extended = "\\\\?\\" + QDir::toNativeSeparators(root.path());
        Config config = authorized(extended); const QString env = root.filePath(".env");
        QVERIFY(errorCode([&] { prepareProxyEnv(config); }).isEmpty());
        QCOMPARE(inspectProxyEnv(config), "current"); QVERIFY(QFileInfo::exists(env));
        config.port = 7890; QVERIFY(errorCode([&] { prepareProxyEnv(config); }).isEmpty());
        QCOMPARE(inspectProxyEnv(config), "current");
        QVERIFY(errorCode([&] { revokeProxyEnv(extended); }).isEmpty()); QVERIFY(!QFileInfo::exists(env));
        writeBytes(env, "OTHER=preserved\r\n");
        QVERIFY(errorCode([&] { prepareProxyEnv(config); }).isEmpty());
        QVERIFY(errorCode([&] { revokeProxyEnv(extended); }).isEmpty());
        QCOMPARE(readBytes(env), "OTHER=preserved\r\n");
        const QString configPath = extended + "\\guard.toml";
        QVERIFY(errorCode([&] { initializeConfig(configPath, config, false); }).isEmpty());
        QCOMPARE(Config::load(configPath), config);
        ConfigLease lease(configPath, config);
    }
    void transactionExcludesExternalRenameAndSaveDeterministically()
    {
        QTemporaryDir root; QVERIFY(root.isValid());
        Config config = authorized(root.path());
        const QString env = root.filePath(".env"); const QString away = root.filePath("renamed.env");
        const QString candidate = root.filePath("editor-save.tmp");
        const QByteArray original = "OTHER=preserved\r\n";
        writeBytes(env, original); prepareProxyEnv(config);
        for (int mode = 0; mode < 3; ++mode) {
            if (mode == 2) QVERIFY(QFile::remove(env));
            writeBytes(candidate, "OTHER=editor-new-value\r\n");
            QJsonObject result;
            bool helperRan = false;
            {
                EnvHook hook([&](bool staged) {
                    if (staged) return;
                    // This child runs only after the transaction has compared
                    // bytes, precisely at the former pathname-race window.
                    QProcess child;
                    child.start(QCoreApplication::applicationFilePath(), {"--env-race-helper", env, away, candidate});
                    if (!child.waitForStarted(2000) || !child.waitForFinished(3000)) {
                        child.kill(); child.waitForFinished(1000);
                        throw Error("TEST_HELPER_FAILED", "The deterministic writer fixture did not complete.");
                    }
                    result = parseJsonObject(child.readAllStandardOutput(), 4096, "TEST_HELPER_FAILED");
                    helperRan = true;
                });
                config.port = 7890;
                const auto failure = errorCode([&] {
                    if (mode == 1) revokeProxyEnv(config.home);
                    else prepareProxyEnv(config);
                });
                QVERIFY2(failure.isEmpty(), qPrintable(failure));
            }
            QVERIFY(helperRan);
            QVERIFY(!result.value("rename_succeeded").toBool());
            QVERIFY(!result.value("save_succeeded").toBool());
            QVERIFY(!result.value("write_open_succeeded").toBool());
            QVERIFY(!QFileInfo::exists(away)); QCOMPARE(readBytes(candidate), "OTHER=editor-new-value\r\n");
            if (mode == 0) { QVERIFY(readBytes(env).startsWith(original)); QCOMPARE(inspectProxyEnv(config), "current"); }
            if (mode == 1) QCOMPARE(readBytes(env), original);
            if (mode == 2) QCOMPARE(inspectProxyEnv(config), "current");
        }
    }
    void abortedTransactionsPublishNothingAndKeepConsent()
    {
        try {
        QTemporaryDir root; QVERIFY(root.isValid()); const auto config = authorized(root.path());
        const QString env = root.filePath(".env"); const QString path = root.filePath("guard.toml");
        initializeConfig(path, config, false); const auto originalConfig = readBytes(path);
        {
            EnvHook hook([&](bool staged) { if (staged) throw Error("TEST_ABORT", "Abort after staging."); });
            QCOMPARE(errorCode([&] { prepareProxyEnv(config); }), "TEST_ABORT");
        }
        QVERIFY(!QFileInfo::exists(env));
        for (const auto &prefix : QList<QByteArray>{QByteArray{}, QByteArray("OTHER=preserved\r\n")}) {
            writeBytes(env, prefix); prepareProxyEnv(config); const auto originalEnv = readBytes(env);
            bool isolated = false;
            {
                struct Reader {
                    HANDLE value;
                    ~Reader() { if (value != INVALID_HANDLE_VALUE) CloseHandle(value); }
                } reader{CreateFileW(reinterpret_cast<const wchar_t *>(env.utf16()), GENERIC_READ,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr)};
                QVERIFY(reader.value != INVALID_HANDLE_VALUE);
                EnvHook hook([&](bool staged) {
                    if (!staged) return;
                    QByteArray visible(originalEnv.size(), Qt::Uninitialized);
                    DWORD got = 0;
                    isolated = ReadFile(reader.value, visible.data(), static_cast<DWORD>(visible.size()), &got, nullptr)
                        && got == static_cast<DWORD>(visible.size()) && visible == originalEnv;
                    throw Error("TEST_ABORT", "Abort after staging.");
                });
                QCOMPARE(errorCode([&] { updateConsent(path, config, false, config.home); }), "BACKEND_PROXY_REVOKE_FAILED");
            }
            QVERIFY(isolated); QCOMPARE(readBytes(env), originalEnv); QCOMPARE(readBytes(path), originalConfig);
            auto updated = config; updated.port = 7890;
            {
                EnvHook hook([&](bool staged) { if (staged) throw Error("TEST_ABORT", "Abort after staging."); });
                QCOMPARE(errorCode([&] { prepareProxyEnv(updated); }), "TEST_ABORT");
            }
            QCOMPARE(readBytes(env), originalEnv);
        }
        } catch (const Error &error) { QFAIL(qPrintable(error.code + ": " + error.message)); }
        catch (const std::exception &error) { QFAIL(error.what()); }
    }
    void commitTimeoutPreservesFileConfigurationAndConsent()
    {
        QTemporaryDir root; QVERIFY(root.isValid()); const auto config = authorized(root.path());
        const QString env = root.filePath(".env"); const QString path = root.filePath("guard.toml");
        initializeConfig(path, config, false); const auto originalConfig = readBytes(path);
        for (int operation = 0; operation < 3; ++operation) {
            // Create, update an existing file, then confirmed consent revoke.
            // The hook does not throw: the actual CommitTransaction call must
            // reject its expired five-second transaction and roll back.
            QByteArray originalEnv;
            if (operation != 0) {
                writeBytes(env, "OTHER=preserved\r\n"); prepareProxyEnv(config); originalEnv = readBytes(env);
            }
            bool staged = false;
            QString failure;
            QString message;
            {
                EnvHook hook([&](bool written) {
                    if (written) { staged = true; QTest::qSleep(5500); }
                });
                try {
                    if (operation == 2) updateConsent(path, config, false, config.home);
                    else {
                        auto updated = config; updated.port = 7890; prepareProxyEnv(updated);
                    }
                } catch (const Error &error) { failure = error.code; message = error.message; }
            }
            QVERIFY(staged);
            if (operation == 2) {
                QCOMPARE(failure, "BACKEND_PROXY_REVOKE_FAILED");
                QVERIFY(message.contains("Cause: BACKEND_PROXY_ENV_IO."));
            } else {
                QCOMPARE(failure, "BACKEND_PROXY_ENV_IO");
                QVERIFY(message.contains("could not commit"));
            }
            if (operation == 0) QVERIFY(!QFileInfo::exists(env));
            else QCOMPARE(readBytes(env), originalEnv);
            QCOMPARE(readBytes(path), originalConfig); QCOMPARE(Config::load(path), config);
        }
    }
};
int main(int argc, char **argv)
{
    QCoreApplication application(argc, argv);
    const auto arguments = application.arguments();
    if (arguments.size() == 5 && arguments[1] == "--env-race-helper") {
        const QString env = QDir::toNativeSeparators(arguments[2]);
        const QString away = QDir::toNativeSeparators(arguments[3]);
        const QString candidate = QDir::toNativeSeparators(arguments[4]);
        const auto wide = [](const QString &text) { return reinterpret_cast<const wchar_t *>(text.utf16()); };
        const bool renamed = MoveFileExW(wide(env), wide(away), 0) != FALSE;
        const DWORD renameError = GetLastError();
        const bool saved = MoveFileExW(wide(candidate), wide(env), MOVEFILE_REPLACE_EXISTING) != FALSE;
        const DWORD saveError = GetLastError();
        const HANDLE writer = CreateFileW(wide(env), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
        const DWORD writeError = GetLastError();
        const bool opened = writer != INVALID_HANDLE_VALUE;
        if (opened) CloseHandle(writer);
        const QJsonObject result{{"rename_succeeded", renamed}, {"rename_error", static_cast<qint64>(renameError)},
            {"save_succeeded", saved}, {"save_error", static_cast<qint64>(saveError)},
            {"write_open_succeeded", opened}, {"write_error", static_cast<qint64>(writeError)}};
        QFile output; output.open(stdout, QIODevice::WriteOnly); output.write(QJsonDocument(result).toJson(QJsonDocument::Compact));
        return 0;
    }
    CoreTests tests;
    return QTest::qExec(&tests, argc, argv);
}
#include "test_core.moc"
