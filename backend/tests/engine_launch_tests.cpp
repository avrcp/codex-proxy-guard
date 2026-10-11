#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "launch_internal.h"
#include "../../tests/support/test_support.h"

#include <windows.h>
#include <shellapi.h>

#include <algorithm>
#include <cstdlib>
#include <thread>

using namespace cpg;
using namespace cpgtest;

namespace {

std::filesystem::path wide(const std::string &utf8)
{
    return std::filesystem::path(utf8ToWide(utf8));
}

Json cleanWorker()
{
    return Json{{"version", 1}, {"activation_returned", true}, {"pid", 42},
                {"activation_hresult", nullptr}, {"failure", nullptr},
                {"early_exit_code", nullptr}, {"observation_failure", nullptr},
                {"instance", "created"}, {"package_identity", "matched"}, {"aumid", "matched"},
                {"elevation", false}};
}

struct Fixture {
    Config config;
    Cancellation cancel;
    detail::LaunchServices services;
    Desktop desktop;
    std::vector<std::string> calls;
    Json worker = cleanWorker();
    Fixture()
    {
        desktop.registered = true;
        desktop.executable = wide("C:/fixture/Desktop.exe");
        desktop.packageFullName = "OpenAI.Codex_1.0_x64__test";
        desktop.packageFamilyName = "OpenAI.Codex_test";
        desktop.applicationId = "App";
        desktop.manifestExecutable = "app/Desktop.exe";
        desktop.aumid = "OpenAI.Codex_test!App";
        desktop.runtimeKind = "full_trust_desktop";
        services.requireElevation = [&] { calls.push_back("elevation"); };
        services.discover = [&] { calls.push_back("discover"); return desktop; };
        services.process = [&](const Desktop &) { calls.push_back("process"); return ProcessState{"stopped", 0}; };
        services.prepare = [&] { calls.push_back("prepare"); };
        services.resolve = [&] { calls.push_back("resolve"); return CodexCli{wide("C:/fixture/codex.exe"), wide("C:/fixture/home")}; };
        services.stop = [&](const CodexCli &) { calls.push_back("stop"); return std::string("stopped"); };
        services.markSubmitted = [&] { calls.push_back("mark"); };
        services.activate = [&](const Desktop &, const std::string &arguments) {
            calls.push_back(arguments.empty() ? "activate_empty" : "activate");
            return worker;
        };
        services.native = [&](const Desktop &, const UnicodeEnvironment &) {
            calls.push_back("native");
            return Json{{"pid", 43}};
        };
    }
    void authorize()
    {
        config.manageBackend = true;
        config.home = wide("C:/fixture/home");
    }
    Json run(LaunchOptions options = {}) { return detail::launchWith(config, options, cancel, services); }
    bool called(const std::string &name) const
    {
        return std::find(calls.begin(), calls.end(), name) != calls.end();
    }
    int count(const std::string &name) const
    {
        return static_cast<int>(std::count(calls.begin(), calls.end(), name));
    }
    int firstIndex(const std::string &name) const
    {
        const auto position = std::find(calls.begin(), calls.end(), name);
        return position == calls.end() ? -1 : static_cast<int>(position - calls.begin());
    }
};

void writeFixture(const std::filesystem::path &path)
{
    std::error_code error;
    std::filesystem::create_directories(path.parent_path(), error);
    writeBytes(path, "fixture, never executed");
}

struct HelperMode {
    explicit HelperMode(const std::string &value)
    {
        const std::wstring wide = utf8ToWide(value);
        SetEnvironmentVariableW(L"CPG_LAUNCH_TEST_HELPER", wide.c_str());
    }
    ~HelperMode() { SetEnvironmentVariableW(L"CPG_LAUNCH_TEST_HELPER", nullptr); }
};

} // namespace

TEST_CASE("proxy arguments")
{
    Config config;
    CHECK(proxyPlan(config).arguments
          == "--proxy-server=\"http://127.0.0.1:10808\" --proxy-bypass-list=\"localhost;127.0.0.1;[::1]\"");
    config.host = "::1";
    CHECK(proxyPlan(config).endpoint == "http://[::1]:10808");
    for (const std::string &entry : {"*", "example.com", "8.8.8.8", "<local>"}) {
        config.noProxy = {entry};
        CHECK_FALSE(errorCode([&] { (void)proxyPlan(config); }).empty());
    }
}

TEST_CASE("environment contract")
{
    Config config;
    const UnicodeEnvironment environment = proxyEnvironment(config);
    CHECK(environment.value(L"HTTP_PROXY").value() == utf8ToWide(config.proxyUrl()));
    CHECK(environment.value(L"HTTPS_PROXY").value() == utf8ToWide(config.proxyUrl()));
    CHECK(environment.value(L"NO_PROXY").value() == utf8ToWide(config.noProxyValue()));
    CHECK_FALSE(environment.has(L"ALL_PROXY"));
}

TEST_CASE("daemon status strict")
{
    CHECK(detail::daemonStopStatus("{\"status\":\"stopped\"}") == "stopped");
    CHECK(detail::daemonStopStatus(" {\"status\":\"notRunning\",\"extra\":true}\n") == "not_needed");
    const std::string outputs[] = {
        std::string(), "not json", "[]", "{\"status\":false}", "{\"status\":\"Stopped\"}",
        "{\"status\":\"running\"}", "{\"status\":\"not_running\"}",
        "{\"status\":\"stopped\"}{}", "{\"status\":\"stopped\",\"status\":\"stopped\"}",
        "{\"status\":\"stopped\",\"x\":\"\xff\"}", std::string(65537, ' ')};
    for (const std::string &output : outputs)
        CHECK(detail::daemonStopStatus(output).empty());
}

TEST_CASE("normal launch never touches the CLI")
{
    Fixture f;
    f.config.cliOverride = wide("Z:/does-not-exist/codex.exe");
    const Json receipt = f.run();
    CHECK(receipt.value("daemon_preparation", "") == "skipped");
    CHECK(receipt.value("backend_proxy_config", "") == "not_authorized");
    CHECK(receipt.value("proxy_delivery", "") == "activation_arguments");
    CHECK_FALSE(f.called("resolve"));
    CHECK_FALSE(f.called("stop"));
    CHECK_FALSE(f.called("prepare"));
    CHECK(f.count("activate") == 1);
    CHECK_FALSE(f.called("native"));
}

TEST_CASE("repair prepares then stops once")
{
    Fixture f;
    f.authorize();
    const Json receipt = f.run({true, false});
    CHECK(receipt.value("backend_proxy_config", "") == "prepared");
    CHECK(receipt.value("daemon_preparation", "") == "stopped");
    CHECK(f.firstIndex("prepare") < f.firstIndex("resolve"));
    CHECK(f.firstIndex("resolve") < f.firstIndex("stop"));
    CHECK(f.firstIndex("stop") < f.firstIndex("activate"));
    CHECK(f.count("stop") == 1);
    CHECK(f.count("discover") == 3);
    CHECK(f.count("process") == 3);
}

TEST_CASE("authorized normal launch prepares without daemon")
{
    Fixture f;
    f.authorize();
    const Json receipt = f.run();
    CHECK(receipt.value("proxy_delivery", "") == "activation_arguments_and_home_config");
    CHECK(f.count("prepare") == 1);
    CHECK_FALSE(f.called("resolve"));
    CHECK_FALSE(f.called("stop"));
}

TEST_CASE("invalid identity fails before preparation")
{
    Fixture f;
    f.authorize();
    f.desktop.aumid = "other!App";
    CHECK(errorCode([&] { (void)f.run({true, false}); }) == "APPX_METADATA_INCOMPLETE");
    CHECK_FALSE(f.called("prepare"));
    CHECK_FALSE(f.called("resolve"));
    CHECK_FALSE(f.called("mark"));
}

TEST_CASE("repair requires consent")
{
    Fixture f;
    CHECK(errorCode([&] { (void)f.run({true, false}); }) == "BACKEND_PROXY_REQUIRED_FOR_REPAIR");
    CHECK_FALSE(f.called("resolve"));
    CHECK_FALSE(f.called("stop"));
}

TEST_CASE("env failure does not interrupt the daemon")
{
    Fixture f;
    f.authorize();
    f.services.prepare = [] { throw Error("BACKEND_PROXY_CONFLICT", "fixture conflict"); };
    CHECK(errorCode([&] { (void)f.run({true, false}); }) == "BACKEND_PROXY_CONFLICT");
    CHECK_FALSE(f.called("resolve"));
    CHECK_FALSE(f.called("stop"));
    CHECK_FALSE(f.called("activate"));
}

TEST_CASE("failed stop does not launch")
{
    Fixture f;
    f.authorize();
    f.services.stop = [](const CodexCli &) -> std::string { throw Error("CODEX_DAEMON_STOP_FAILED", "fixture"); };
    CHECK(errorCode([&] { (void)f.run({true, false}); }) == "CODEX_DAEMON_STOP_FAILED");
    CHECK_FALSE(f.called("mark"));
    CHECK_FALSE(f.called("activate"));
}

TEST_CASE("cancellation after stop does not launch")
{
    Fixture f;
    f.authorize();
    f.services.stop = [&](const CodexCli &) { f.cancel.cancel(); return std::string("stopped"); };
    CHECK(errorCode([&] { (void)f.run({true, false}); }) == "LAUNCH_CANCELLED");
    CHECK_FALSE(f.called("mark"));
}

TEST_CASE("changed package after stop does not launch")
{
    Fixture f;
    f.authorize();
    f.services.stop = [&](const CodexCli &) { f.desktop.packageFullName += "_changed"; return std::string("stopped"); };
    CHECK(errorCode([&] { (void)f.run({true, false}); }) == "APPX_PACKAGE_CHANGED");
    CHECK_FALSE(f.called("activate"));
}

TEST_CASE("running and unknown refused")
{
    for (const std::string &state : {"running", "unknown"}) {
        Fixture f;
        f.config.refuseIfRunning = false;
        f.services.process = [&](const Desktop &) { return ProcessState{state, 9}; };
        CHECK_FALSE(errorCode([&] { (void)f.run(); }).empty());
        CHECK_FALSE(f.called("prepare"));
        CHECK_FALSE(f.called("mark"));
    }
}

TEST_CASE("running after stop refused")
{
    Fixture f;
    f.authorize();
    bool stopped = false;
    f.services.stop = [&](const CodexCli &) { stopped = true; return std::string("stopped"); };
    f.services.process = [&](const Desktop &) { return ProcessState{stopped ? "running" : "stopped", 0}; };
    CHECK(errorCode([&] { (void)f.run({true, false}); }) == "CODEX_ALREADY_RUNNING");
    CHECK_FALSE(f.called("mark"));
}

TEST_CASE("elevated fails before discovery")
{
    Fixture f;
    f.services.requireElevation = [] { throw Error("GUARD_ELEVATED", "fixture"); };
    CHECK(errorCode([&] { (void)f.run(); }) == "GUARD_ELEVATED");
    CHECK(f.calls.empty());
}

TEST_CASE("activation-only is explicitly unproxied")
{
    Fixture f;
    f.authorize();
    const Json receipt = f.run({false, true});
    CHECK(receipt.value("proxy_delivery", "") == "not_established");
    CHECK(receipt.value("backend_proxy_config", "") == "not_applicable");
    CHECK(receipt.value("proxy_endpoint", Json("")).is_null());
    CHECK(f.called("activate_empty"));
    CHECK_FALSE(f.called("prepare"));
    CHECK_FALSE(f.called("resolve"));
    CHECK(errorCode([&] { (void)f.run({true, true}); }) == "INVALID_LAUNCH_OPTIONS");
}

TEST_CASE("native launch uses only the environment")
{
    Fixture f;
    f.desktop.registered = false;
    std::string injectedEndpoint;
    f.services.native = [&](const Desktop &, const UnicodeEnvironment &environment) {
        injectedEndpoint = wideToUtf8(environment.value(L"HTTP_PROXY").value_or(L""));
        f.calls.push_back("native");
        return Json{{"pid", 43}};
    };
    const Json receipt = f.run();
    CHECK(injectedEndpoint == f.config.proxyUrl());
    CHECK(receipt.value("proxy_delivery", "") == "process_environment");
    CHECK(f.called("native"));
    CHECK_FALSE(f.called("activate"));
    CHECK_FALSE(f.called("prepare"));
    CHECK_FALSE(f.called("stop"));
    CHECK(errorCode([&] { (void)f.run({false, true}); }) == "ACTIVATION_ONLY_UNSUPPORTED");
}

TEST_CASE("instance observation passes through without retry")
{
    for (const std::string &instance : {"created", "reused", "unknown"}) {
        Fixture f;
        f.worker["instance"] = instance;
        const Json receipt = f.run();
        CHECK(receipt.value("instance", "") == instance);
        CHECK(f.count("activate") == 1); // Reuse never triggers a second activation.
    }
}

TEST_CASE("activation failures never retry")
{
    for (const char *field : {"package_identity", "aumid"}) {
        Fixture f;
        f.worker[field] = "mismatch";
        CHECK_FALSE(errorCode([&] { (void)f.run(); }).empty());
        CHECK(f.count("activate") == 1);
        CHECK_FALSE(f.called("native"));
    }
    {
        Json worker = cleanWorker();
        worker["activation_returned"] = false;
        CHECK(errorCode([&] { (void)detail::activationReceipt(worker); }) == "APPX_ACTIVATION_OUTCOME_UNKNOWN");
    }
    {
        Json worker = cleanWorker();
        worker["early_exit_code"] = 0;
        CHECK(errorCode([&] { (void)detail::activationReceipt(worker); }) == "APPX_TARGET_EXITED_EARLY");
    }
}

TEST_CASE("resolver controlled paths")
{
    TemporaryDirectory root;
    const std::filesystem::path cwd = root.file(L"cwd");
    const std::filesystem::path trusted = root.file(L"trusted path");
    writeFixture(cwd / L"codex.exe");
    writeFixture(trusted / L"codex.exe");
    const std::string path = ";.;relative;" + wideToUtf8(cwd.wstring()) + ';' + wideToUtf8(trusted.wstring());
    const auto cli = detail::resolveCliFrom({}, root.path, path, {}, cwd);
    CHECK(cli.executable == *canonicalPathOf(trusted / L"codex.exe", false));
    CHECK(errorCode([&] { (void)detail::resolveCliFrom({}, root.path, wideToUtf8(cwd.wstring()), {}, cwd); }) == "CODEX_CLI_UNAVAILABLE");
    CHECK(errorCode([&] { (void)detail::resolveCliFrom(root.file(L"missing"), root.path, path, {}, cwd); }) == "CODEX_HOME_INVALID");
    CHECK(errorCode([&] { (void)detail::resolveCliFrom({}, root.path, path, wide("codex.exe"), cwd); }) == "CODEX_CLI_OVERRIDE_INVALID");
    const std::filesystem::path shim = root.file(L"codex.cmd");
    writeFixture(shim);
    CHECK(errorCode([&] { (void)detail::resolveCliFrom({}, root.path, path, shim, cwd); }) == "CODEX_CLI_OVERRIDE_INVALID");
}

TEST_CASE("resolver home order and pinning")
{
    TemporaryDirectory root;
    const std::filesystem::path home = root.file(L"home");
    const std::filesystem::path standalone = home / L"packages" / L"standalone" / L"current" / L"codex.exe";
    const std::filesystem::path daemon = home / L"packages" / L"app-server-daemon" / L"current" / L"bin" / L"codex.exe";
    writeFixture(standalone);
    CHECK(detail::resolveCliFrom(wide("home"), {}, {}, {}, root.path).executable
          == *canonicalPathOf(standalone, false));
    writeFixture(daemon);
    const auto cli = detail::resolveCliFrom(wide("home"), {}, {}, {}, root.path);
    CHECK(cli.executable == *canonicalPathOf(daemon, false));
    CHECK(cli.home == *canonicalPathOf(home, true));
}

TEST_CASE("public stop uses the bounded owned fixture")
{
    TemporaryDirectory root;
    const std::filesystem::path executable = root.file(L"codex.exe");
    std::error_code error;
    std::filesystem::copy_file(currentExecutablePath(), executable, error);
    REQUIRE_FALSE(error);
    const CodexCli cli{executable, root.path};
    for (const std::string &mode : {"stopped", "notRunning"}) {
        HelperMode scope(mode);
        CHECK(stopCodexDaemon(cli, Cancellation{}, std::chrono::milliseconds(5000))
              == (mode == "stopped" ? "stopped" : "not_needed"));
    }
    for (const std::string &mode : {"oversize", "invalid", "exit"}) {
        HelperMode scope(mode);
        CHECK(errorCode([&] { (void)stopCodexDaemon(cli, Cancellation{}, std::chrono::milliseconds(5000)); })
              == "CODEX_DAEMON_STOP_FAILED");
    }
    {
        HelperMode scope("unsupported");
        CHECK(errorCode([&] { (void)stopCodexDaemon(cli, Cancellation{}, std::chrono::milliseconds(5000)); })
              == "CODEX_DAEMON_UNSUPPORTED");
    }
    {
        HelperMode scope("wait");
        CHECK(errorCode([&] { (void)stopCodexDaemon(cli, Cancellation{}, std::chrono::milliseconds(100)); })
              == "CODEX_DAEMON_STOP_TIMEOUT");
    }
    {
        HelperMode scope("wait");
        Cancellation cancellation;
        std::thread canceller([&] {
            Sleep(100);
            cancellation.cancel();
        });
        const std::string code = errorCode([&] { (void)stopCodexDaemon(cli, cancellation, std::chrono::milliseconds(5000)); });
        canceller.join();
        CHECK(code == "LAUNCH_CANCELLED");
    }
}

// Codex CLI fixture mode: this branch only runs in the copy renamed to
// codex.exe; a real Codex CLI is never executed.
static int codexFixtureMode(const std::vector<std::wstring> &arguments)
{
    const wchar_t *modeValue = _wgetenv(L"CPG_LAUNCH_TEST_HELPER");
    const std::string mode = modeValue ? wideToUtf8(modeValue) : std::string();
    if (mode.empty()) return -1;
    if (arguments != std::vector<std::wstring>{L"app-server", L"daemon", L"stop"}) return 91;
    std::string output;
    int status = 0;
    if (mode == "wait") Sleep(10000);
    else if (mode == "oversize") output = std::string(65537, 'x');
    else if (mode == "invalid") output = "{\"status\":\"running\"}";
    else if (mode == "unsupported") { output = "unknown command: daemon"; status = 2; }
    else if (mode == "exit") status = 3;
    else output = "{\"status\":\"" + mode + "\"}";
    if (!output.empty()) {
        DWORD written = 0;
        WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), output.data(),
                  static_cast<DWORD>(output.size()), &written, nullptr);
    }
    return status;
}

int main(int argc, char **argv)
{
    int count = 0;
    LPWSTR *wide = CommandLineToArgvW(GetCommandLineW(), &count);
    const std::vector<std::wstring> arguments(wide && count > 1 ? wide + 1 : wide,
                                              wide && count > 1 ? wide + count : wide);
    if (wide) LocalFree(wide);
    const std::wstring ownName = currentExecutablePath().filename().wstring();
    if (ownName == L"codex.exe") {
        const int status = codexFixtureMode(arguments);
        if (status >= 0) return status;
    }
    return cpgtest::runDoctestMain(argc, argv);
}
