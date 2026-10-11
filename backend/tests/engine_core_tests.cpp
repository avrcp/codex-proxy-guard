#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "core.h"
#include "../../tests/support/test_support.h"

#include <windows.h>
#include <shellapi.h>

#include <cstring>
#include <functional>
#include <memory>
#include <vector>

using namespace cpg;
namespace cpg::testing { void setEnvTransactionHook(std::function<void(bool)> hook); }
using namespace cpgtest;

namespace {

struct EnvHook {
    explicit EnvHook(std::function<void(bool)> hook) { testing::setEnvTransactionHook(std::move(hook)); }
    ~EnvHook() { testing::setEnvTransactionHook({}); }
};

Config authorized(const std::filesystem::path &home)
{
    Config config;
    config.manageBackend = true;
    config.home = home;
    return config;
}

std::filesystem::path wide(const std::string &utf8) { return std::filesystem::path(utf8ToWide(utf8)); }

} // namespace

TEST_CASE("strict TOML and round trip")
{
    Config expected;
    expected.executableOverride = wide("C:/程序/Codex.exe");
    expected.cliOverride = wide("D:\\tools\\codex.exe");
    expected.manageBackend = true;
    expected.home = wide("C:/fixture/.codex");
    CHECK(Config::parse(expected.toml()) == expected);
    CHECK(Config::parse("# no fields\n") == Config{});
    CHECK(Config::parse("[proxy]\nhost = '::1'\nport = 0x1ED2\n").port == 7890);
    const std::string invalid[] = {
        "version=1", "version=2.0", "version=true", "version=2147483648", "version=2\nversion=2",
        "legacy=true", "[proxy]\nlegacy=true", "[codex]\nauto_stop=true", "[tui]\nlegacy=true",
        "proxy=[]", "[proxy]\nport='7890'", "[proxy]\nno_proxy=['localhost', 1]",
        "[codex]\nrefuse_if_running='true'", "[tui]\nalternate_screen='Auto'",
        "[proxy]\nport=1979-05-27", "[proxy]\nport=65536", "[proxy]\nport=0",
        std::string("# ") + hexString("c080"), std::string(65537, '#')};
    for (const std::string &bytes : invalid)
        CHECK(errorCode([&] { (void)Config::parse(bytes); }) == "CONFIG_INVALID");
}

TEST_CASE("loopback and bounds")
{
    Config config;
    const std::string validHosts[] = {"localhost", "LOCALHOST", "127.0.0.1", "127.255.2.3", "::1", "0:0:0:0:0:0:0:1"};
    for (const std::string &host : validHosts) {
        config.host = host;
        CHECK(errorCode([&] { config.validate(); }).empty());
    }
    const std::string invalidHosts[] = {"", " 127.0.0.1", "localhost ", "127.1", "127.0.0.01", "192.0.2.1",
                                        "0.0.0.0", "::", "::ffff:127.0.0.1", "[::1]", "localhost.evil", "localhost@evil"};
    for (const std::string &host : invalidHosts) {
        config.host = host;
        CHECK(errorCode([&] { config.validate(); }) == "CONFIG_INVALID");
    }
    config = Config{};
    config.host = "::1";
    CHECK(config.proxyUrl() == "http://[::1]:10808");
    config.scheme = "HTTP";
    config.validate();
    config.scheme = "socks5";
    CHECK(errorCode([&] { config.validate(); }) == "CONFIG_INVALID");
    config = Config{};
    const std::string invalidEntries[] = {"", " ", "localhost\n", " a", std::string(256, 'a'),
                                          std::string(86 * 3, '\xe7'), std::string("a") + '\0' + "b"};
    for (const std::string &value : invalidEntries) {
        config.noProxy = {value};
        CHECK(errorCode([&] { config.validate(); }) == "CONFIG_INVALID");
    }
    config.noProxy = std::vector<std::string>(32, std::string(255, 'a'));
    CHECK(errorCode([&] { config.validate(); }) == "CONFIG_INVALID");
    config.noProxy = {"localhost"};
    config.validate();
}

TEST_CASE("lease excludes mutation and deletion")
{
    TemporaryDirectory root;
    const std::filesystem::path path = root.file(L"guard.toml");
    const Config config;
    initializeConfig(path, config, false);
    {
        ConfigLease lease(path, config);
        ConfigLease second(path, config);
        CHECK(Config::load(path) == config);
        CHECK(errorCode([&] { updateProxy(path, config, false, "127.0.0.1", 7890); }) == "CONFIG_LOCK_FAILED");
        CHECK(errorCode([&] { initializeConfig(path, config, true); }) == "CONFIG_LOCK_FAILED");
        // An external writer cannot open the leased file for writing or remove it.
        HANDLE writer = CreateFileW(path.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr,
                                    OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
        CHECK(writer == INVALID_HANDLE_VALUE);
        CHECK_FALSE(DeleteFileW(path.c_str()));
    }
    const auto updated = updateProxy(path, config, false, "127.0.0.1", 7890);
    CHECK(updated.port == 7890);
    CHECK(errorCode([&] { ConfigLease stale(path, config); }) == "CONFIG_CHANGED");
    CHECK(errorCode([&] { updateProxy(path, config, false, "::1", 7891); }) == "CONFIG_CHANGED");
    CHECK(errorCode([&] { updateProxy(path, config, true, "::1", 7891); }) == "CONFIG_CHANGED");
    CHECK(Config::load(path) == updated);
    writeBytes(path, "version=1\n");
    CHECK(updateProxy(path, config, true, "::1", 7891).port == 7891);
    CHECK(Config::load(path).host == "::1");
}

TEST_CASE("consent and scope")
{
    TemporaryDirectory root;
    const std::filesystem::path path = root.file(L"guard.toml");
    Config config;
    config.home = root.file(L"codex-home");
    initializeConfig(path, config, false);
    CHECK(inspectProxyEnv(config) == "not_authorized");
    CHECK(errorCode([&] { prepareProxyEnv(config); }) == "BACKEND_PROXY_SCOPE_UNCONFIRMED");
    CHECK(errorCode([&] { updateConsent(path, config, true, root.file(L"other")); }) == "BACKEND_PROXY_SCOPE_UNCONFIRMED");
    config = updateConsent(path, config, true, config.home);
    CHECK(config.manageBackend);
    CHECK(inspectProxyEnv(config) == "pending");
    CHECK_FALSE(std::filesystem::exists(config.home));
    CHECK(errorCode([&] { initializeConfig(path, Config{}, true); }) == "BACKEND_PROXY_REVOKE_REQUIRED");
    prepareProxyEnv(config);
    CHECK(inspectProxyEnv(config) == "current");
    const std::filesystem::path home = config.home;
    config = updateConsent(path, config, false, home);
    CHECK_FALSE(config.manageBackend);
    CHECK(config.home.empty());
    CHECK_FALSE(std::filesystem::exists(home / L".env"));
    CHECK(Config::load(path) == config);
    for (const std::string &invalidHome : {"relative", "C:relative", "\\rooted", "//server",
                                           "//?/GLOBALROOT/Device/HarddiskVolume1", "//./C:/device"}) {
        config.home = wide(invalidHome);
        CHECK(errorCode([&] { (void)consentHome(config); }) == "BACKEND_PROXY_SCOPE_UNCONFIRMED");
    }
    config.home = wide("\\\\?\\C:\\Users\\fixture\\.codex");
    CHECK(consentHome(config) == config.home);
    config.home = wide("\\\\?\\UNC\\server\\share\\.codex");
    CHECK(consentHome(config) == config.home);
}

TEST_CASE("env preserves bytes and is idempotent")
{
    TemporaryDirectory root;
    const auto config = authorized(root.path);
    const std::filesystem::path env = root.file(L".env");
    const std::string original = "# user comments\r\nOTHER=value\r\n\r\n";
    writeBytes(env, original);
    prepareProxyEnv(config);
    const std::string first = readBytes(env);
    CHECK(first.substr(0, original.size()) == original);
    prepareProxyEnv(config);
    CHECK(readBytes(env) == first);
    Config moved = config;
    moved.port = 7890;
    CHECK(inspectProxyEnv(moved) == "stale");
    prepareProxyEnv(moved);
    CHECK(readBytes(env).substr(0, original.size()) == original);
    CHECK(readBytes(env).find("HTTP_PROXY=http://127.0.0.1:7890\n") != std::string::npos);
    revokeProxyEnv(root.path);
    CHECK(readBytes(env) == original);
    // A suffix with no final newline is preserved exactly on update/revoke.
    prepareProxyEnv(moved);
    writeBytes(env, readBytes(env) + "TAIL=unchanged");
    Config again = moved;
    again.port = 7891;
    prepareProxyEnv(again);
    revokeProxyEnv(root.path);
    CHECK(readBytes(env) == original + "TAIL=unchanged");
    writeBytes(env, hexString("efbbbf") + "OTHER=preserved\r\n");
    const std::string withBom = readBytes(env);
    prepareProxyEnv(moved);
    revokeProxyEnv(root.path);
    CHECK(readBytes(env) == withBom);
}

TEST_CASE("env conflict and malformed block")
{
    TemporaryDirectory root;
    const auto config = authorized(root.path);
    const std::filesystem::path env = root.file(L".env");
    const std::string lines[] = {
        "HTTP_PROXY=existing\n", " export https_proxy = existing\r\n", "export\tNO_PROXY=existing",
        "all_proxy=existing", hexString("efbbbf") + "HTTP_PROXY=existing"};
    for (const std::string &line : lines) {
        writeBytes(env, line);
        CHECK(inspectProxyEnv(config) == "conflict");
        CHECK(errorCode([&] { prepareProxyEnv(config); }) == "BACKEND_PROXY_CONFIG_CONFLICT");
        CHECK(readBytes(env) == line);
    }
    const std::string begin = "# BEGIN CODEX PROXY GUARD: proxy-v1\n";
    const std::string end = "# END CODEX PROXY GUARD: proxy-v1\n";
    const std::string blocks[] = {begin, end, begin + end, begin + "EXTRA=1\n" + end,
        begin + begin + end, "# BEGIN CODEX PROXY GUARD: proxy-v2\n", "# END CODEX PROXY GUARD: proxy-v2\n"};
    for (const std::string &block : blocks) {
        writeBytes(env, block);
        CHECK(inspectProxyEnv(config) == "invalid");
        CHECK(errorCode([&] { prepareProxyEnv(config); }) == "BACKEND_PROXY_BLOCK_INVALID");
        CHECK(errorCode([&] { revokeProxyEnv(root.path); }) == "BACKEND_PROXY_BLOCK_INVALID");
        CHECK(readBytes(env) == block);
    }
    writeBytes(env, hexString("ff"));
    CHECK(inspectProxyEnv(config) == "invalid");
    CHECK(errorCode([&] { prepareProxyEnv(config); }) == "BACKEND_PROXY_ENV_ENCODING");
    writeBytes(env, std::string(1024 * 1024 + 1, '#'));
    CHECK(inspectProxyEnv(config) == "unavailable");
}

TEST_CASE("concurrent env writer refuses replacement")
{
    TemporaryDirectory root;
    const std::filesystem::path env = root.file(L".env");
    const auto config = authorized(root.path);
    const std::string original = "OTHER=preserved\r\n";
    writeBytes(env, original);
    const HANDLE writer = CreateFileW(env.c_str(), GENERIC_WRITE,
                                      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                                      OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    REQUIRE(writer != INVALID_HANDLE_VALUE);
    const std::string failure = errorCode([&] { prepareProxyEnv(config); });
    CloseHandle(writer);
    CHECK(failure == "BACKEND_PROXY_ENV_IO");
    CHECK(readBytes(env) == original);
    prepareProxyEnv(config);
    CHECK(inspectProxyEnv(config) == "current");
}

TEST_CASE("env syntax gate rejects unsafe boundaries")
{
    TemporaryDirectory root;
    const std::filesystem::path env = root.file(L".env");
    const std::filesystem::path path = root.file(L"guard.toml");
    const auto config = authorized(root.path);
    initializeConfig(path, config, false);
    const std::string originalConfig = readBytes(path);
    const std::string begin = "# BEGIN CODEX PROXY GUARD: proxy-v1\n";
    const std::string end = "# END CODEX PROXY GUARD: proxy-v1\n";
    const std::string inner = "HTTP_PROXY=http://127.0.0.1:10808\nHTTPS_PROXY=http://127.0.0.1:10808\nNO_PROXY=localhost,127.0.0.1,::1\n";
    // A complete marker block that is only the content of a multi-line
    // quoted value is not a managed block, and unterminated quoting never
    // becomes one through appending.
    const std::string unsafe[] = {
        "TEMPLATE='\n" + begin + inner + end + "'\n",
        "TEMPLATE=\"\n" + begin + inner + end + "\"\n",
        "TEMPLATE='unterminated\n",
        "TEMPLATE=\"unterminated",
        "VALUE='esc\\' apen\n",
        "VALUE=tail\\\nNEXT=1\n",
        begin + inner + end + "TEMPLATE='open\n"};
    for (const std::string &bytes : unsafe) {
        writeBytes(env, bytes);
        CHECK(inspectProxyEnv(config) == "unsupported");
        CHECK(errorCode([&] { prepareProxyEnv(config); }) == "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED");
        CHECK(readBytes(env) == bytes); // Refusal never touches a byte.
        std::string revokeMessage;
        try { updateConsent(path, config, false, config.home); }
        catch (const Error &error) {
            REQUIRE(error.code == "BACKEND_PROXY_REVOKE_FAILED");
            revokeMessage = error.message;
        }
        CHECK_FALSE(revokeMessage.empty());
        CHECK(revokeMessage.find("Cause: BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED.") != std::string::npos);
        CHECK(readBytes(env) == bytes);
        CHECK(readBytes(path) == originalConfig); // Consent and its bound Home survive for retry.
        CHECK(Config::load(path) == config);
    }
    // An existing valid block plus later unsupported syntax refuses the
    // whole edit; Guard never deletes the part that "looks like its own".
    const std::string mixed = begin + inner + end + "TEMPLATE='open\n";
    writeBytes(env, mixed);
    CHECK(errorCode([&] { revokeProxyEnv(config.home); }) == "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED");
    CHECK(readBytes(env) == mixed);
}

TEST_CASE("env syntax gate accepts single-line quoting")
{
    TemporaryDirectory root;
    const auto config = authorized(root.path);
    const std::filesystem::path env = root.file(L".env");
    const std::string safe = "# someone's \"settings\"\r\n"
        "OTHER='single quoted value'\r\n"
        "ANOTHER=\"double \\\"escaped\\\" value\"\r\n"
        "HASHY=literal#hash\r\n"
        "TRAILING=value # trailing comment with 'quote'\r\n"
        "export SIMPLE=plain\r\n";
    writeBytes(env, safe);
    prepareProxyEnv(config);
    CHECK(inspectProxyEnv(config) == "current");
    CHECK(readBytes(env).substr(0, safe.size()) == safe);
    CHECK(readBytes(env).find("HTTP_PROXY=http://127.0.0.1:10808\n") != std::string::npos);
    revokeProxyEnv(root.path);
    CHECK(readBytes(env) == safe);
}

TEST_CASE("failed revoke preserves authorization and file")
{
    TemporaryDirectory root;
    const std::filesystem::path path = root.file(L"guard.toml");
    const std::filesystem::path env = root.file(L".env");
    const Config config = authorized(root.path);
    initializeConfig(path, config, false);
    const std::string broken = "# BEGIN CODEX PROXY GUARD: proxy-v1\nEXTRA=1\n# END CODEX PROXY GUARD: proxy-v1\n";
    writeBytes(env, broken);
    const std::string originalConfig = readBytes(path);
    CHECK(errorCode([&] { updateConsent(path, config, false, config.home); }) == "BACKEND_PROXY_REVOKE_FAILED");
    CHECK(readBytes(env) == broken);
    CHECK(readBytes(path) == originalConfig);
    REQUIRE(DeleteFileW(env.c_str()));
    prepareProxyEnv(config);
    const std::string validBlock = readBytes(env);
    // A reader refusing delete access prevents the atomic remove. Consent
    // must remain retryable and must not strand a block in an unknown Home.
    const HANDLE locked = CreateFileW(env.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                      OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    REQUIRE(locked != INVALID_HANDLE_VALUE);
    const std::string failure = errorCode([&] { updateConsent(path, config, false, config.home); });
    CloseHandle(locked);
    CHECK(failure == "BACKEND_PROXY_REVOKE_FAILED");
    CHECK(readBytes(env) == validBlock);
    CHECK(readBytes(path) == originalConfig);
    const auto revoked = updateConsent(path, config, false, config.home);
    CHECK_FALSE(revoked.manageBackend);
}

TEST_CASE("edited managed values preserve consent and bytes")
{
    TemporaryDirectory root;
    const auto config = authorized(root.path);
    const std::filesystem::path env = root.file(L".env");
    const std::filesystem::path path = root.file(L"guard.toml");
    initializeConfig(path, config, false);
    const std::string originalConfig = readBytes(path);
    const std::string endpoint = "http://127.0.0.1:10808";
    const std::vector<std::vector<std::string>> invalid{
        {"http://192.0.2.1:10808", "http://192.0.2.1:10808", "localhost"},
        {endpoint, "http://127.0.0.1:7890", "localhost"},
        {"https://127.0.0.1:10808", "https://127.0.0.1:10808", "localhost"},
        {"http://user:pass@127.0.0.1:10808", "http://user:pass@127.0.0.1:10808", "localhost"},
        {"http://127.0.0.1:00080", "http://127.0.0.1:00080", "localhost"},
        {"http://127.0.0.1:0", "http://127.0.0.1:0", "localhost"},
        {"http://127.0.0.1:65536", "http://127.0.0.1:65536", "localhost"},
        {endpoint, endpoint, ""}, {endpoint, endpoint, " localhost"},
        {endpoint, endpoint, std::string("local") + '\0' + "host"},
        {endpoint, endpoint, std::string(256, 'a')}, {endpoint, endpoint, std::string(4097, 'a')}};
    for (const auto &values : invalid) {
        const std::string bytes = "# user data\r\n# BEGIN CODEX PROXY GUARD: proxy-v1\nHTTP_PROXY=" + values[0]
            + "\nHTTPS_PROXY=" + values[1] + "\nNO_PROXY=" + values[2] + "\n# END CODEX PROXY GUARD: proxy-v1\n";
        writeBytes(env, bytes);
        CHECK(inspectProxyEnv(config) == "invalid");
        CHECK(errorCode([&] { prepareProxyEnv(config); }) == "BACKEND_PROXY_BLOCK_INVALID");
        CHECK(errorCode([&] { revokeProxyEnv(config.home); }) == "BACKEND_PROXY_BLOCK_INVALID");
        CHECK(errorCode([&] { updateConsent(path, config, false, config.home); }) == "BACKEND_PROXY_REVOKE_FAILED");
        CHECK(readBytes(env) == bytes);
        CHECK(readBytes(path) == originalConfig);
    }
    REQUIRE(DeleteFileW(env.c_str()));
    Config unusual = config;
    unusual.host = "0:0:0:0:0:0:0:1";
    unusual.noProxy = {",", std::string(240, 'a') + ",,", std::string(240, 'b')};
    prepareProxyEnv(unusual);
    CHECK(inspectProxyEnv(unusual) == "current");
    // A valid prior endpoint is stale, not externally corrupted.
    CHECK(inspectProxyEnv(config) == "stale");
    prepareProxyEnv(config);
    CHECK(inspectProxyEnv(config) == "current");
    revokeProxyEnv(config.home);
    CHECK_FALSE(std::filesystem::exists(env));
}

TEST_CASE("invalid recovery never erases consent evidence")
{
    TemporaryDirectory root;
    const std::filesystem::path path = root.file(L"guard.toml");
    const std::filesystem::path env = root.file(L".env");
    const auto config = authorized(root.path);
    prepareProxyEnv(config);
    const std::string originalEnv = readBytes(env);
    const std::string invalid[] = {
        "version=1\n[codex]\nmanage_codex_proxy_env=true\nproxy_env_home='C:/existing/home'\n",
        "[proxy\n[codex]\nmanage_codex_proxy_env=true\nproxy_env_home='C:/existing/home'\n",
        "version=2\n[codex]\nproxy_env_home='C:/existing/home'\nBROKEN =\n",
        "version=1\n[codex]\n\"manage_codex_proxy_\\u0065nv\"=true\n",
        "[codex]\n\"proxy_env_\\U00000068ome\"='C:/existing/home'\nBROKEN =\n",
        std::string("version=1\n") + hexString("ff")};
    for (const std::string &bytes : invalid) {
        writeBytes(path, bytes);
        CHECK(errorCode([&] { updateProxy(path, Config{}, true, "127.0.0.1", 7890); }) == "CONFIG_CONSENT_REPAIR_REQUIRED");
        CHECK(errorCode([&] { initializeConfig(path, Config{}, true); }) == "CONFIG_CONSENT_REPAIR_REQUIRED");
        CHECK(readBytes(path) == bytes);
        CHECK(readBytes(env) == originalEnv);
    }
    // Repair the syntax manually without dropping the binding; the normal
    // confirmed revoke path can then safely remove the block and consent.
    writeBytes(path, config.toml());
    const auto revoked = updateConsent(path, config, false, config.home);
    CHECK_FALSE(revoked.manageBackend);
    CHECK_FALSE(std::filesystem::exists(env));
    writeBytes(path, "version=1\n");
    CHECK(updateProxy(path, Config{}, true, "127.0.0.1", 7890).port == 7890);
}

TEST_CASE("legacy extended home supports actual file operations")
{
    TemporaryDirectory root;
    const std::wstring wide = root.path.wstring();
    const std::filesystem::path extended = wide.substr(0, 2) == L"\\\\"
        ? root.path : std::filesystem::path(L"\\\\?\\" + wide);
    Config config = authorized(extended);
    const std::filesystem::path env = root.file(L".env");
    CHECK(errorCode([&] { prepareProxyEnv(config); }).empty());
    CHECK(inspectProxyEnv(config) == "current");
    CHECK(std::filesystem::exists(env));
    config.port = 7890;
    CHECK(errorCode([&] { prepareProxyEnv(config); }).empty());
    CHECK(inspectProxyEnv(config) == "current");
    CHECK(errorCode([&] { revokeProxyEnv(extended); }).empty());
    CHECK_FALSE(std::filesystem::exists(env));
    writeBytes(env, "OTHER=preserved\r\n");
    CHECK(errorCode([&] { prepareProxyEnv(config); }).empty());
    CHECK(errorCode([&] { revokeProxyEnv(extended); }).empty());
    CHECK(readBytes(env) == "OTHER=preserved\r\n");
    const std::filesystem::path configPath = extended / L"guard.toml";
    CHECK(errorCode([&] { initializeConfig(configPath, config, false); }).empty());
    CHECK(Config::load(configPath) == config);
    ConfigLease lease(configPath, config);
}

TEST_CASE("transaction excludes external rename-and-save deterministically")
{
    TemporaryDirectory root;
    Config config = authorized(root.path);
    const std::filesystem::path env = root.file(L".env");
    const std::filesystem::path away = root.file(L"renamed.env");
    const std::filesystem::path candidate = root.file(L"editor-save.tmp");
    const std::string original = "OTHER=preserved\r\n";
    writeBytes(env, original);
    prepareProxyEnv(config);
    for (int mode = 0; mode < 3; ++mode) {
        if (mode == 2) REQUIRE(DeleteFileW(env.c_str()));
        writeBytes(candidate, "OTHER=editor-new-value\r\n");
        Json result;
        bool helperRan = false;
        {
            EnvHook hook([&](bool staged) {
                if (staged) return;
                // This child runs only after the transaction has compared
                // bytes, precisely at the former pathname-race window.
                ProcessRequest request;
                request.program = currentExecutablePath();
                request.arguments = {L"--env-race-helper", env.wstring(), away.wstring(), candidate.wstring()};
                request.environment = UnicodeEnvironment::current();
                request.timeout = std::chrono::milliseconds(5000);
                request.stdoutLimit = 4096;
                const ProcessResult child = runOwnedProcess(request, {});
                result = parseStrictJsonObject(child.out, 4096, "TEST_HELPER_FAILED");
                helperRan = true;
            });
            config.port = 7890;
            const std::string failure = errorCode([&] {
                if (mode == 1) revokeProxyEnv(config.home);
                else prepareProxyEnv(config);
            });
            REQUIRE(failure.empty());
        }
        REQUIRE(helperRan);
        CHECK_FALSE(result.value("rename_succeeded", false));
        CHECK_FALSE(result.value("save_succeeded", false));
        CHECK_FALSE(result.value("write_open_succeeded", false));
        CHECK_FALSE(std::filesystem::exists(away));
        CHECK(readBytes(candidate) == "OTHER=editor-new-value\r\n");
        if (mode == 0) {
            CHECK(readBytes(env).substr(0, original.size()) == original);
            CHECK(inspectProxyEnv(config) == "current");
        }
        if (mode == 1) CHECK(readBytes(env) == original);
        if (mode == 2) CHECK(inspectProxyEnv(config) == "current");
    }
}

TEST_CASE("aborted transactions publish nothing and keep consent")
{
    TemporaryDirectory root;
    const auto config = authorized(root.path);
    const std::filesystem::path env = root.file(L".env");
    const std::filesystem::path path = root.file(L"guard.toml");
    initializeConfig(path, config, false);
    const std::string originalConfig = readBytes(path);
    {
        EnvHook hook([&](bool staged) {
            if (staged) throw Error("TEST_ABORT", "Abort after staging.");
        });
        CHECK(errorCode([&] { prepareProxyEnv(config); }) == "TEST_ABORT");
    }
    CHECK_FALSE(std::filesystem::exists(env));
    for (const std::string &prefix : {std::string(), std::string("OTHER=preserved\r\n")}) {
        writeBytes(env, prefix);
        prepareProxyEnv(config);
        const std::string originalEnv = readBytes(env);
        bool isolated = false;
        {
            struct Reader {
                HANDLE value = nullptr;
                ~Reader() { if (value != INVALID_HANDLE_VALUE) CloseHandle(value); }
            } reader{CreateFileW(env.c_str(), GENERIC_READ,
                                 FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                                 OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr)};
            REQUIRE(reader.value != INVALID_HANDLE_VALUE);
            EnvHook hook([&](bool staged) {
                if (!staged) return;
                std::string visible(originalEnv.size(), '\0');
                DWORD got = 0;
                isolated = ReadFile(reader.value, visible.data(),
                                    static_cast<DWORD>(visible.size()), &got, nullptr)
                    && got == static_cast<DWORD>(visible.size()) && visible == originalEnv;
                throw Error("TEST_ABORT", "Abort after staging.");
            });
            CHECK(errorCode([&] { updateConsent(path, config, false, config.home); }) == "BACKEND_PROXY_REVOKE_FAILED");
        }
        CHECK(isolated);
        CHECK(readBytes(env) == originalEnv);
        CHECK(readBytes(path) == originalConfig);
        Config updated = config;
        updated.port = 7890;
        {
            EnvHook hook([&](bool staged) {
                if (staged) throw Error("TEST_ABORT", "Abort after staging.");
            });
            CHECK(errorCode([&] { prepareProxyEnv(updated); }) == "TEST_ABORT");
        }
        CHECK(readBytes(env) == originalEnv);
    }
}

TEST_CASE("commit timeout preserves file, configuration and consent")
{
    TemporaryDirectory root;
    const auto config = authorized(root.path);
    const std::filesystem::path env = root.file(L".env");
    const std::filesystem::path path = root.file(L"guard.toml");
    initializeConfig(path, config, false);
    const std::string originalConfig = readBytes(path);
    for (int operation = 0; operation < 3; ++operation) {
        // Create, update an existing file, then confirmed consent revoke.
        // The hook does not throw: the actual CommitTransaction call must
        // reject its expired five-second transaction and roll back.
        std::string originalEnv;
        if (operation != 0) {
            writeBytes(env, "OTHER=preserved\r\n");
            prepareProxyEnv(config);
            originalEnv = readBytes(env);
        }
        bool staged = false;
        std::string failure;
        std::string message;
        {
            EnvHook hook([&](bool written) {
                if (written) {
                    staged = true;
                    Sleep(5500);
                }
            });
            try {
                if (operation == 2) updateConsent(path, config, false, config.home);
                else {
                    Config updated = config;
                    updated.port = 7890;
                    prepareProxyEnv(updated);
                }
            } catch (const Error &error) {
                failure = error.code;
                message = error.message;
            }
        }
        REQUIRE(staged);
        if (operation == 2) {
            CHECK(failure == "BACKEND_PROXY_REVOKE_FAILED");
            CHECK(message.find("Cause: BACKEND_PROXY_ENV_IO.") != std::string::npos);
        } else {
            CHECK(failure == "BACKEND_PROXY_ENV_IO");
            CHECK(message.find("could not commit") != std::string::npos);
        }
        if (operation == 0) CHECK_FALSE(std::filesystem::exists(env));
        else CHECK(readBytes(env) == originalEnv);
        CHECK(readBytes(path) == originalConfig);
        CHECK(Config::load(path) == config);
    }
}

// Deterministic writer fixture: the child attempts rename-save-editor
// operations exactly at the transaction's comparison window.
static int envRaceHelper(const std::vector<std::wstring> &arguments)
{
    const std::filesystem::path env = arguments[2];
    const std::filesystem::path away = arguments[3];
    const std::filesystem::path candidate = arguments[4];
    const BOOL renamed = MoveFileExW(env.c_str(), away.c_str(), 0);
    const DWORD renameError = GetLastError();
    const BOOL saved = MoveFileExW(candidate.c_str(), env.c_str(), MOVEFILE_REPLACE_EXISTING);
    const DWORD saveError = GetLastError();
    HANDLE writer = CreateFileW(env.c_str(), GENERIC_WRITE,
                                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                                OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    const DWORD writeError = GetLastError();
    const bool opened = writer != INVALID_HANDLE_VALUE;
    if (opened) CloseHandle(writer);
    const Json result{{"rename_succeeded", renamed != FALSE},
                      {"rename_error", static_cast<std::uint64_t>(renameError)},
                      {"save_succeeded", saved != FALSE},
                      {"save_error", static_cast<std::uint64_t>(saveError)},
                      {"write_open_succeeded", opened},
                      {"write_error", static_cast<std::uint64_t>(writeError)}};
    const std::string output = jsonDump(result);
    DWORD written = 0;
    WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), output.data(),
              static_cast<DWORD>(output.size()), &written, nullptr);
    return 0;
}

int main(int argc, char **argv)
{
    // The wide command line is authoritative for fixture paths; ANSI argv
    // only feeds doctest's own option parsing.
    int count = 0;
    LPWSTR *wide = CommandLineToArgvW(GetCommandLineW(), &count);
    std::vector<std::wstring> wideArguments;
    if (wide) {
        wideArguments.assign(wide, wide + count);
        LocalFree(wide);
    }
    if (wideArguments.size() == 5 && wideArguments[1] == L"--env-race-helper")
        return envRaceHelper(wideArguments);
    return cpgtest::runDoctestMain(argc, argv);
}
