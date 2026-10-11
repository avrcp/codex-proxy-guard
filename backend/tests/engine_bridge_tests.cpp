#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "bridge.h"
#include "cli.h"
#include "../../tests/support/test_support.h"

#include <windows.h>

#include <string>

using namespace cpg;
using namespace cpgtest;

namespace {

std::string request(int id, const std::string &method, const Json &params = Json::object())
{
    return jsonDump(Json{{"schema", 1}, {"id", id}, {"method", method}, {"params", params}});
}

std::string codeOf(const std::vector<Json> &messages)
{
    return messages.empty() ? std::string() : messages.front().value("error", Json::object()).value("code", "");
}

Desktop noDesktop(const Config &, const Cancellation &)
{
    throw Error("APPX_NOT_FOUND", "No Desktop fixture.");
}

std::vector<Json> complete(BridgeSession &session)
{
    for (int elapsed = 0; elapsed < 5000; ++elapsed) {
        const std::vector<Json> messages = session.poll();
        if (!messages.empty()) return messages;
        Sleep(1);
    }
    return {};
}

std::filesystem::path enginePath() { return std::filesystem::path(utf8ToWide(TARGET_ENGINE_PATH)); }

bool messagesOk(const std::vector<Json> &messages)
{
    return !messages.empty() && messages.front().value("ok", false);
}

} // namespace

TEST_CASE("protocol gate")
{
    TemporaryDirectory dir;
    BridgeSession s(dir.file(L"guard.toml"));
    CHECK(codeOf(s.request(request(1, "snapshot"))) == "BRIDGE_HELLO_REQUIRED");
    CHECK(s.request(request(2, "hello")).front()["result"]["protocol_version"] == 1);
    CHECK(codeOf(s.request(request(2, "hello"))) == "BRIDGE_ID_REUSED");
    CHECK(codeOf(s.request(request(3, "hello", Json{{"secret", "value"}}))) == "BRIDGE_PARAMS_INVALID");
    CHECK_FALSE(s.request(request(4, "shutdown")).empty());
    CHECK(s.closed());
    CHECK_FALSE(std::filesystem::exists(dir.file(L"guard.toml")));
}

TEST_CASE("rejects untrusted protocol")
{
    TemporaryDirectory dir;
    BridgeSession s(dir.file(L"guard.toml"));
    const std::string invalid[] = {
        R"({"schema":1,"id":1,"id":2,"method":"hello","params":{}})",
        R"({"schema":1,"id":1.5,"method":"hello","params":{}})",
        R"({"schema":1,"id":9007199254740992,"method":"hello","params":{}})",
        R"({"schema":1,"id":1,"method":"hello","params":{},"extra":true})",
        std::string(33000, 'x'), std::string("\xff", 1)};
    for (const std::string &frame : invalid)
        CHECK_FALSE(messagesOk(s.request(frame)));
}

TEST_CASE("no unconfirmed mutations")
{
    TemporaryDirectory dir;
    const std::filesystem::path path = dir.file(L"guard.toml");
    initializeConfig(path, {}, false);
    const auto original = Config::load(path);
    BridgeSession s(path);
    s.request(request(1, "hello"));
    CHECK(codeOf(s.request(request(2, "set_backend_proxy_consent",
        Json{{"enabled", true}, {"confirmation_token", "guessed"}}))) == "CONFIG_INVALID");
    CHECK(codeOf(s.request(request(3, "start_launch",
        Json{{"repair", true}, {"confirmation_token", "guessed"}}))) == "CONFIG_INVALID");
    CHECK(Config::load(path) == original);
}

TEST_CASE("real pipe shutdown without EOF")
{
    TemporaryDirectory dir;
    PipeProcess process;
    REQUIRE(process.start(enginePath(), {L"--config", dir.file(L"guard.toml").wstring(), L"bridge"}));
    REQUIRE(process.writeAll(request(1, "hello") + '\n' + request(2, "shutdown") + '\n'));
    CHECK(process.waitExit(5000) == 0);
    const std::string output = process.readAll();
    const std::size_t first = output.find('\n');
    const std::size_t second = output.find('\n', first + 1);
    REQUIRE(first != std::string::npos);
    REQUIRE(second != std::string::npos);
    const Json hello = parseStrictJsonObject(output.substr(0, first + 1), 4096, "TEST");
    const Json bye = parseStrictJsonObject(output.substr(first + 1, second - first), 4096, "TEST");
    CHECK(hello.value("ok", false));
    CHECK(bye["result"]["shutting_down"]);
    const bool exactlyTwoLines = output.find('\n', second + 1) == std::string::npos;
    CHECK(exactlyTwoLines);
    CHECK_FALSE(std::filesystem::exists(dir.file(L"guard.toml")));
}

TEST_CASE("stalled stdout cannot pin the bridge")
{
    // Use an intentionally unread native pipe to exercise the engine's
    // output deadline (the runner drains its pipes while waiting).
    TemporaryDirectory dir;
    PipeProcess process;
    REQUIRE(process.start(enginePath(), {L"--config", dir.file(L"guard.toml").wstring(), L"bridge"}));
    std::string frames;
    for (int id = 1; id < 65; ++id) frames += request(id, "hello") + '\n';
    REQUIRE(frames.size() <= 4096);
    REQUIRE(process.writeAll(frames));
    CHECK(process.waitExit(8000) == 2);
    CHECK_FALSE(std::filesystem::exists(dir.file(L"guard.toml")));
}

TEST_CASE("single-use consent and stale config")
{
    TemporaryDirectory dir;
    const std::filesystem::path path = dir.file(L"guard.toml");
    Config config;
    config.home = dir.path;
    initializeConfig(path, config, false);
    BridgeSession s(path, noDesktop, [] { return std::string("not_elevated"); });
    s.request(request(1, "hello"));
    CHECK(s.request(request(2, "snapshot")).empty());
    auto messages = complete(s);
    REQUIRE_FALSE(messages.empty());
    auto snapshot = messages.front()["result"];
    const auto proposal = snapshot["confirmations"]["backend_proxy"];
    CHECK(proposal["home"] == wideToUtf8(dir.path.wstring()));
    const std::string token = proposal.value("token", "");
    CHECK_FALSE(token.empty());
    CHECK(s.request(request(3, "set_backend_proxy_consent",
        Json{{"enabled", true}, {"confirmation_token", token}})).empty());
    CHECK(codeOf(s.request(request(4, "set_proxy", Json{{"host", "127.0.0.1"}, {"port", 8080}}))) == "BRIDGE_BUSY");
    CHECK_FALSE(complete(s).empty());
    CHECK(Config::load(path).manageBackend);
    CHECK(codeOf(s.request(request(5, "set_backend_proxy_consent",
        Json{{"enabled", false}, {"confirmation_token", token}}))) == "CONFIRMATION_REQUIRED");
    CHECK_FALSE(std::filesystem::exists(dir.file(L".env")));
    s.request(request(6, "snapshot"));
    messages = complete(s);
    REQUIRE_FALSE(messages.empty());
    const std::string revokeToken = messages.front()["result"]["confirmations"]["backend_proxy"].value("token", "");
    CHECK_FALSE(revokeToken.empty());
    auto current = Config::load(path);
    updateProxy(path, current, false, "localhost", 8080);
    CHECK(codeOf(s.request(request(7, "set_backend_proxy_consent",
        Json{{"enabled", false}, {"confirmation_token", revokeToken}}))) == "CONFIG_CHANGED");
    CHECK(Config::load(path).manageBackend);
}

TEST_CASE("malformed config can be repaired but cannot launch")
{
    TemporaryDirectory dir;
    const std::filesystem::path path = dir.file(L"guard.toml");
    writeBytes(path, "malformed !");
    BridgeSession s(path, noDesktop);
    s.request(request(1, "hello"));
    s.request(request(2, "snapshot"));
    auto messages = complete(s);
    REQUIRE_FALSE(messages.empty());
    CHECK(messages.front()["result"].value("config_readiness", "") == "repair_required");
    CHECK(codeOf(s.request(request(3, "start_launch", Json{{"repair", false}}))) == "CONFIG_INVALID");
    s.request(request(4, "set_proxy", Json{{"host", "127.0.0.1"}, {"port", 8080}}));
    messages = complete(s);
    REQUIRE_FALSE(messages.empty());
    CHECK(messages.front().value("ok", false));
    CHECK(Config::load(path).port == 8080);
}

TEST_CASE("CLI rejects removed and conflicting options")
{
    const std::vector<std::vector<std::wstring>> argumentSets = {
        {L"daemon-stop"},
        {L"launch", L"--auto-stop-daemon"},
        {L"launch", L"--refresh-codex-daemon", L"--activation-only"},
        {L"bridge", L"--force"},
        {L"internal-activate-package", L"--config", L"unused"}};
    for (const auto &arguments : argumentSets) {
        ProcessRequest processRequest;
        processRequest.program = enginePath();
        processRequest.arguments = arguments;
        processRequest.environment = UnicodeEnvironment::current();
        processRequest.timeout = std::chrono::milliseconds(5000);
        processRequest.stdoutLimit = 4096;
        const ProcessResult result = runOwnedProcess(processRequest, {});
        INFO(result.out << " | " << result.err);
        CHECK(result.exitCode != 0);
    }
}

TEST_CASE("provenance does not load configuration")
{
    TemporaryDirectory dir;
    const std::filesystem::path path = dir.path / L"missing" / L"config.toml";
    ProcessRequest processRequest;
    processRequest.program = enginePath();
    processRequest.arguments = {L"--config", path.wstring(), L"build-info"};
    processRequest.environment = UnicodeEnvironment::current();
    processRequest.timeout = std::chrono::milliseconds(5000);
    processRequest.stdoutLimit = 64 * 1024;
    const ProcessResult result = runOwnedProcess(processRequest, {});
    CHECK(result.exitCode == 0);
    const Json info = parseStrictJsonObject(result.out, 64 * 1024, "TEST");
    CHECK(info.value("language", "") == "C++20");
    CHECK_FALSE(std::filesystem::exists(path));
}

TEST_CASE("licenses command streams the embedded notices")
{
    // The GUI licenses viewer consumes this exact stream, so its content
    // contract is anchored here against the production executable.
    ProcessRequest processRequest;
    processRequest.program = enginePath();
    processRequest.arguments = {L"licenses"};
    processRequest.environment = UnicodeEnvironment::current();
    processRequest.timeout = std::chrono::milliseconds(10000);
    processRequest.stdoutLimit = 4 * 1024 * 1024;
    const ProcessResult result = runOwnedProcess(processRequest, {});
    CHECK(result.exitCode == 0);
    CHECK(result.out.size() > 100 * 1024); // measured corpus ~380 KiB at 0.6.1-rc.1
    CHECK(result.out.find("===== LICENSE =====") != std::string::npos);
    CHECK(result.out.find("===== THIRD_PARTY_NOTICES.md =====") != std::string::npos);
    const std::string suffix = "\n-----\n\n";
    REQUIRE(result.out.size() >= suffix.size());
    CHECK(result.out.compare(result.out.size() - suffix.size(), suffix.size(), suffix) == 0);
}

TEST_CASE("renamed executable in unicode directory stays headless")
{
    TemporaryDirectory dir;
    const std::filesystem::path target = dir.path / L"\u8bb8 \u53ef \u76ee\u5f55";
    std::error_code error;
    std::filesystem::create_directories(target, error);
    REQUIRE_FALSE(error);
    const std::filesystem::path copied = target / L"\u5b88\u536b exe.exe";
    std::filesystem::copy_file(enginePath(), copied, error);
    REQUIRE_FALSE(error);
    ProcessRequest processRequest;
    processRequest.program = copied;
    processRequest.arguments = {L"licenses"};
    processRequest.environment = UnicodeEnvironment::current();
    processRequest.timeout = std::chrono::milliseconds(10000);
    processRequest.stdoutLimit = 4 * 1024 * 1024;
    const ProcessResult result = runOwnedProcess(processRequest, {});
    CHECK(result.exitCode == 0);
    CHECK_FALSE(result.out.empty());
}

CPG_TEST_MAIN
