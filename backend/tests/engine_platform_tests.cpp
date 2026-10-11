#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "platform.h"
#include "platform_protocol_p.h"
#include "../../tests/support/test_support.h"

#include <windows.h>
#include <shellapi.h>

#include <string>
#include <thread>

using namespace cpg;
using namespace cpgtest;

namespace {

Json application(const std::string &id = "App", const std::string &executable = "app\\ChatGPT.exe")
{
    return Json{{"application_id", id}, {"manifest_executable", executable},
        {"entry_point", "Windows.FullTrustApplication"}, {"runtime_behavior", nullptr},
        {"trust_level", nullptr}};
}

Json record(const std::filesystem::path &root, const std::string &package = "OpenAI.Codex")
{
    return Json{{"package_name", package}, {"package_version", "1.0.0.0"}, {"architecture", "x64"},
        {"package_full_name", package + "_1.0.0.0_x64__fixture"},
        {"package_family_name", package + "_fixture"}, {"install_location", wideToUtf8(root.wstring())},
        {"applications", Json::array({application(), application("CommandRunner", "app/runner.exe")})}};
}

std::string envelope(const Json &records)
{
    return jsonDump(Json{{"schema_version", 1}, {"records", records}});
}

bool createDesktop(const std::filesystem::path &root)
{
    std::error_code error;
    std::filesystem::create_directories(root / "app", error);
    if (error) return false;
    writeBytes(root / L"app" / L"ChatGPT.exe", "fixture");
    return true;
}

std::string discoveryFailure(const std::string &bytes, const std::filesystem::path &path = {})
{
    try { (void)parseDiscovery(bytes, path); }
    catch (const Error &error) { return error.code; }
    return {};
}

std::string encode(const Json &object) { return jsonDump(object); }

Json request()
{
    return Json{{"version", 1}, {"aumid", "OpenAI.Codex_fixture!App"},
        {"expected_package_full_name", "OpenAI.Codex_1.0.0.0_x64__fixture"},
        {"expected_executable", "C:/fixture/app/ChatGPT.exe"},
        {"arguments", "--proxy-server=\"http://127.0.0.1:10808\""}};
}

Json receipt()
{
    return Json{{"version", 1}, {"failure", nullptr}, {"activation_returned", true},
        {"activation_hresult", nullptr}, {"pid", 123}, {"instance", "created"},
        {"package_identity", "matched"}, {"observed_package_full_name", "OpenAI.Codex_fixture"},
        {"aumid", "matched"}, {"observed_aumid", "OpenAI.Codex_fixture!App"},
        {"elevation", false}, {"early_exit_code", nullptr}, {"observation_failure", nullptr}};
}

const char *enginePath()
{
    return TARGET_ENGINE_PATH;
}

} // namespace

TEST_CASE("discovers registered entry with nulls")
{
    TemporaryDirectory temporary;
    REQUIRE(createDesktop(temporary.path));
    const Desktop desktop = parseDiscovery(envelope(Json::array({record(temporary.path)})));
    CHECK(desktop.registered);
    CHECK(desktop.aumid == "OpenAI.Codex_fixture!App");
    CHECK(desktop.runtimeKind == "full_trust_desktop");
    const std::string publicBytes = jsonDump(desktop.publicJson());
    CHECK(publicBytes.find(wideToUtf8(temporary.path.wstring())) == std::string::npos);
    CHECK_FALSE(desktop.publicJson().contains("executable"));
    CHECK_FALSE(desktop.publicJson().contains("install_location"));
    Json noAttributes = record(temporary.path);
    Json app = application();
    app.erase("runtime_behavior");
    app.erase("trust_level");
    noAttributes["applications"] = Json::array({app});
    CHECK(parseDiscovery(envelope(Json::array({noAttributes}))).runtimeKind == "full_trust_desktop");
}

TEST_CASE("codex is preferred over classic")
{
    TemporaryDirectory temporary;
    REQUIRE(createDesktop(temporary.path));
    CHECK(parseDiscovery(envelope(Json::array({record(temporary.path, "OpenAI.ChatGPT-Desktop"),
        record(temporary.path)}))).packageName == "OpenAI.Codex");
}

TEST_CASE("discovery protocol fails closed")
{
    TemporaryDirectory temporary;
    REQUIRE(createDesktop(temporary.path));
    const std::filesystem::path path = temporary.path / L"app" / L"ChatGPT.exe";
    const std::string inputs[] = {
        std::string(), "[]", "{}",
        "{\"schema_version\":1,\"records\":null}",
        "{\"schema_version\":1,\"records\":{}}",
        "{\"schema_version\":1,\"records\":[]}" + std::string(1, char(-1))};
    for (const std::string &bytes : inputs)
        CHECK_FALSE(discoveryFailure(bytes, path).empty());
    CHECK(discoveryFailure("{\"schema_version\":2,\"records\":[]}", path) == "APPX_DISCOVERY_PROTOCOL_UNSUPPORTED");
    CHECK(discoveryFailure(envelope(Json::array({}))) == "CODEX_NOT_INSTALLED");
    CHECK_FALSE(parseDiscovery(envelope(Json::array({})), path).registered);
    Json bad = record(temporary.path);
    Json app = application();
    app["trust_level"] = 7;
    bad["applications"] = Json::array({app});
    CHECK(discoveryFailure(envelope(Json::array({bad})), path) == "APPX_DISCOVERY_PROTOCOL_INVALID");
    bad["applications"] = Json::array({application(), application()});
    CHECK(discoveryFailure(envelope(Json::array({bad}))) == "APPX_APPLICATION_AMBIGUOUS");
    bad = record(temporary.path);
    bad["package_family_name"] = "";
    CHECK(discoveryFailure(envelope(Json::array({bad}))) == "APPX_METADATA_INCOMPLETE");
}

TEST_CASE("override preserves registration and rejects other package exe")
{
    TemporaryDirectory temporary;
    REQUIRE(createDesktop(temporary.path));
    const std::string data = envelope(Json::array({record(temporary.path)}));
    const std::filesystem::path overridePath = temporary.path / L"app" / L"ChatGPT.exe";
    const Desktop desktop = parseDiscovery(data, overridePath);
    CHECK(desktop.registered);
    CHECK(desktop.discoverySource == "executable_override");
    const std::filesystem::path other = temporary.path / L"app" / L"Other.exe";
    writeBytes(other, "");
    CHECK(discoveryFailure(data, other) == "APPX_OVERRIDE_INVALID");
}

TEST_CASE("manifest traversal is rejected")
{
    TemporaryDirectory temporary;
    REQUIRE(createDesktop(temporary.path));
    for (const std::string &path : {"../ChatGPT.exe", "/ChatGPT.exe", "C:/ChatGPT.exe", "app/../../ChatGPT.exe"}) {
        Json data = record(temporary.path, "OpenAI.ChatGPT-Desktop");
        data["applications"] = Json::array({application("Desktop", path)});
        CHECK(discoveryFailure(envelope(Json::array({data}))) == "APPX_EXECUTABLE_INVALID");
    }
}

TEST_CASE("activation protocol has no side effects")
{
    CHECK(detail::activationRequest(encode(request())) == request());
    for (const char *field : {"version", "aumid", "expected_package_full_name", "expected_executable", "arguments"}) {
        Json invalid = request();
        invalid.erase(field);
        CHECK_THROWS_AS(detail::activationRequest(encode(invalid)), Error);
    }
    for (const char *aumid : {"family", "family!", "!App", "family!App extra", "family!App!Other"}) {
        Json invalid = request();
        invalid["aumid"] = aumid;
        CHECK_THROWS_AS(detail::activationRequest(encode(invalid)), Error);
    }
    Json invalid = request();
    invalid["arguments"] = "--flag=\xe6\x97\xa5\xe6\x9c\xac";
    CHECK_THROWS_AS(detail::activationRequest(encode(invalid)), Error);
    invalid = request();
    invalid["arguments"] = std::string(4097, 'x');
    CHECK_THROWS_AS(detail::activationRequest(encode(invalid)), Error);
    invalid = request();
    invalid["expected_executable"] = "relative.exe";
    CHECK_THROWS_AS(detail::activationRequest(encode(invalid)), Error);
    invalid = request();
    invalid["extra"] = true;
    CHECK_THROWS_AS(detail::activationRequest(encode(invalid)), Error);
    CHECK_THROWS_AS(detail::activationRequest(std::string(16385, 'x')), Error);
}

TEST_CASE("receipts reject malformed facts")
{
    CHECK(detail::activationReceipt(encode(receipt())) == receipt());
    Json invalid = receipt();
    invalid["pid"] = 1.5;
    CHECK_THROWS_AS(detail::activationReceipt(encode(invalid)), Error);
    invalid = receipt();
    invalid["version"] = 2;
    CHECK_THROWS_AS(detail::activationReceipt(encode(invalid)), Error);
    invalid = receipt();
    invalid["activation_returned"] = false;
    CHECK_THROWS_AS(detail::activationReceipt(encode(invalid)), Error);
    invalid = receipt();
    invalid["package_identity"] = "true";
    CHECK_THROWS_AS(detail::activationReceipt(encode(invalid)), Error);
    CHECK_THROWS_AS(detail::activationReceipt(encode(receipt()) + encode(receipt())), Error);
    CHECK_THROWS_AS(detail::activationReceipt(encode(receipt()) + std::string(1, char(-1))), Error);
}

TEST_CASE("worker rejects malformed input before activation")
{
    const HelperResult result = runHelper(std::filesystem::path(utf8ToWide(enginePath())),
        {L"internal-activate-package"}, "{\"version\":99}", {}, std::chrono::milliseconds(5000), 65536);
    CHECK(result.exitCode != 0);
    CHECK(result.out.empty());
    CHECK(result.err.find("APPX_ACTIVATION_PROTOCOL_INVALID") != std::string::npos);
}

TEST_CASE("owned helper cancellation and timeout")
{
    const std::filesystem::path self = currentExecutablePath();
    Cancellation alreadyCancelled;
    alreadyCancelled.cancel();
    CHECK(errorCode([&] {
        (void)runHelper(self, {L"--test-owned-helper-sleep"}, {}, alreadyCancelled,
                        std::chrono::milliseconds(1000), 1024);
    }) == "LAUNCH_CANCELLED");
    CHECK(errorCode([&] {
        (void)runHelper(self, {L"--test-owned-helper-sleep"}, {}, Cancellation{},
                        std::chrono::milliseconds(100), 1024);
    }) == "HELPER_TIMEOUT");
    Cancellation cancellation;
    std::jthread canceller([cancellation] { Sleep(100); cancellation.cancel(); });
    CHECK(errorCode([&] {
        (void)runHelper(self, {L"--test-owned-helper-sleep"}, {}, cancellation,
                        std::chrono::milliseconds(5000), 1024);
    }) == "LAUNCH_CANCELLED");
    canceller.join();
}

TEST_CASE("owned helper output limit and submission")
{
    const std::filesystem::path self = currentExecutablePath();
    CHECK(errorCode([&] {
        (void)runHelper(self, {L"--test-owned-helper-output"}, {}, Cancellation{},
                        std::chrono::milliseconds(5000), 1024);
    }) == "HELPER_OUTPUT_LIMIT");
    CHECK(errorCode([&] {
        (void)runHelper(self, {L"--test-owned-helper-sleep"}, "complete request", Cancellation{},
                        std::chrono::milliseconds(100), 1024,
                        UnicodeEnvironment::current(), true);
    }) == "APPX_ACTIVATION_OUTCOME_UNKNOWN");
}

TEST_CASE("normalized paths and current process")
{
    CHECK(normalizedWindowsPath(L"\\\\?\\C:\\Fixture\\App.exe") == L"c:/fixture/app.exe");
    CHECK(normalizedWindowsPath(L"\\\\?\\UNC\\server\\share\\App.exe") == L"//server/share/app.exe");
    Desktop desktop;
    desktop.executable = currentExecutablePath();
    const ProcessState state = desktopProcessState(desktop);
    CHECK(state.state == "running");
    CHECK(state.pid == static_cast<std::uint32_t>(GetCurrentProcessId()));
}

// Hidden child modes spawned through runHelper above; pure-ASCII arguments.
// ANSI argv from this environment's CRT startup is untrustworthy for wmain,
// so the wide command line is re-derived via CommandLineToArgvW (the same
// authoritative path the production entry point uses).
int main(int argc, char **argv)
{
    int count = 0;
    LPWSTR *wide = CommandLineToArgvW(GetCommandLineW(), &count);
    std::vector<std::wstring> wideArguments;
    if (wide) {
        wideArguments.assign(wide, wide + count);
        LocalFree(wide);
    }
    for (std::size_t index = 1; index < wideArguments.size(); ++index) {
        const std::wstring &argument = wideArguments[index];
        if (argument == L"--test-owned-helper-sleep") { Sleep(10000); return 0; }
        if (argument == L"--test-owned-helper-output") {
            const std::string flood(65536, 'x');
            DWORD written = 0;
            WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), flood.data(),
                      static_cast<DWORD>(flood.size()), &written, nullptr);
            return 0;
        }
    }
    return cpgtest::runDoctestMain(argc, argv);
}
