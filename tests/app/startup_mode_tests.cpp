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
    for (const wchar_t *command : {L"internal-activate-package", L"build-info",
                                   L"launch", L"init-config", L"config-path", L"console",
                                   L"licenses"}) {
        CHECK(classifyStartup(args({command})).role == StartupRole::Cli);
    }
    CHECK(classifyStartup(args({L"--build-info"})).role == StartupRole::Cli);
    CHECK(classifyStartup(args({L"--help"})).role == StartupRole::Cli);
    CHECK(classifyStartup(args({L"--version"})).role == StartupRole::Cli);
    CHECK(classifyStartup(args({L"--config", L"C:/tmp/config.toml"})).role == StartupRole::Gui);
    CHECK(classifyStartup(args({L"--config=A:/tmp/config.toml"})).role == StartupRole::Gui);
    // The retired --smoke-test spelling is delegated to the CLI parser,
    // which rejects unknown options; a --config value may still contain it.
    CHECK(classifyStartup(args({L"--config=C:/dir/--smoke-test"})).role == StartupRole::Gui);
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
    // The removed bridge role is an unknown command again, even past "--".
    CHECK(classifyStartup(args({L"--", L"bridge"})).shouldExit);
}

TEST_CASE("classification failures exit before any role")
{
    const std::vector<std::vector<std::wstring>> rejected = {
        args({L"unknown-command"}),
        args({L"--config", L"a", L"--config", L"b"}),
        args({L"--config"}),
        args({L"--config=a", L"--config=b"}),
        args({L"--", L"unknown-command"}),
    };
    for (const auto &arguments : rejected) {
        const StartupMode mode = classifyStartup(arguments);
        CHECK(mode.shouldExit);
        CHECK(mode.exitCode != 0);
        CHECK_FALSE(mode.errorMessage.empty());
    }
}

CPG_TEST_MAIN
