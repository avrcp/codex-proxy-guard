#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "../../tests/support/test_support.h"

#include <windows.h>

#include <string>

using namespace cpg;
using namespace cpgtest;

namespace {

std::filesystem::path enginePath() { return std::filesystem::path(utf8ToWide(TARGET_ENGINE_PATH)); }

} // namespace

TEST_CASE("CLI rejects removed and conflicting options")
{
    const std::vector<std::vector<std::wstring>> argumentSets = {
        {L"daemon-stop"},
        {L"launch", L"--auto-stop-daemon"},
        {L"launch", L"--refresh-codex-daemon", L"--activation-only"},
        {L"bridge"},
        {L"internal-activate-package", L"--config", L"unused"},
        // --plain belongs to the interactive mode only; an empty --config
        // value must be rejected instead of silently using a null path.
        {L"launch", L"--plain"},
        {L"init-config", L"--plain"},
        {L"licenses", L"--plain"},
        {L"--config=", L"build-info"},
        {L"--config", L"", L"config-path"}};
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
        CHECK(result.err.find("CLI_INVALID") != std::string::npos);
    }
}

TEST_CASE("usage text matches the real command surface")
{
    ProcessRequest processRequest;
    processRequest.program = enginePath();
    processRequest.arguments = {L"--help"};
    processRequest.environment = UnicodeEnvironment::current();
    processRequest.timeout = std::chrono::milliseconds(5000);
    processRequest.stdoutLimit = 64 * 1024;
    const ProcessResult result = runOwnedProcess(processRequest, {});
    CHECK(result.exitCode == 0);
    CHECK(result.out.find("launch") != std::string::npos);
    CHECK(result.out.find("console") != std::string::npos);
    // The bridge role was removed; usage must not advertise it as available.
    CHECK(result.out.find("bridge") == std::string::npos);
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
    CHECK(result.out.size() > 4 * 1024); // project + notices + toml++ + nlohmann
    CHECK(result.out.find("===== LICENSE =====") != std::string::npos);
    CHECK(result.out.find("===== THIRD_PARTY_NOTICES.md =====") != std::string::npos);
    CHECK(result.out.find("===== json/LICENSE.MIT =====") != std::string::npos);
    CHECK(result.out.find("===== toml++/LICENSE =====") != std::string::npos);
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
