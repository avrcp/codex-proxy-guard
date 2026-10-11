#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "startup_mode.h"

#include "../../tests/support/test_support.h"

using namespace cpg;
using namespace cpgtest;

namespace {
std::vector<std::wstring> args(std::initializer_list<std::wstring> values)
{
    return std::vector<std::wstring>(values);
}
} // namespace

TEST_CASE("no arguments selects the interactive role")
{
    const StartupMode mode = classifyStartup({});
    CHECK(mode.role == StartupRole::Gui);
    CHECK_FALSE(mode.shouldExit);
    CHECK(mode.configPath.empty());
}

TEST_CASE("headless commands and options classify to the CLI role")
{
    for (const wchar_t *command : {L"bridge", L"internal-activate-package", L"build-info",
                                   L"launch", L"init-config", L"config-path", L"console",
                                   L"licenses"}) {
        CHECK(classifyStartup(args({command})).role == StartupRole::Cli);
    }
    CHECK(classifyStartup(args({L"--build-info"})).role == StartupRole::Cli);
    CHECK(classifyStartup(args({L"--help"})).role == StartupRole::Cli);
    CHECK(classifyStartup(args({L"--version"})).role == StartupRole::Cli);
    CHECK(classifyStartup(args({L"--config", L"C:/tmp/config.toml"})).role == StartupRole::Gui);
    CHECK(classifyStartup(args({L"--config=C:/tmp/config.toml"})).role == StartupRole::Gui);
    CHECK(classifyStartup(args({L"--smoke-test"})).role == StartupRole::Gui);
    CHECK(classifyStartup(args({L"--smoke-test"})).smokeTest);
}

TEST_CASE("option values containing role words cannot flip the role")
{
    const StartupMode bridge = classifyStartup(args({L"--config", L"C:/tmp/bridge"}));
    CHECK(bridge.role == StartupRole::Gui);
    CHECK(bridge.configPath == L"C:/tmp/bridge");
    const StartupMode smoke = classifyStartup(args({L"--config=C:/dir/--smoke-test"}));
    CHECK(smoke.role == StartupRole::Gui);
    CHECK(smoke.configPath == L"C:/dir/--smoke-test");
    const StartupMode command = classifyStartup(args({L"--config", L"bridge", L"launch"}));
    CHECK(command.role == StartupRole::Cli);
    const StartupMode separator = classifyStartup(args({L"--", L"bridge"}));
    CHECK(separator.role == StartupRole::Cli);
}

TEST_CASE("classification failures exit before any role")
{
    const std::vector<std::vector<std::wstring>> rejected = {
        args({L"unknown-command"}),
        args({L"--config", L"a", L"--config", L"b"}),
        args({L"--config"}),
        args({L"--config=a", L"--config=b"}),
        args({L"--smoke-test", L"launch"}),
        args({L"--smoke-test", L"--build-info"}),
    };
    for (const auto &arguments : rejected) {
        const StartupMode mode = classifyStartup(arguments);
        CHECK(mode.shouldExit);
        CHECK(mode.exitCode != 0);
        CHECK_FALSE(mode.errorMessage.empty());
    }
}

CPG_TEST_MAIN
