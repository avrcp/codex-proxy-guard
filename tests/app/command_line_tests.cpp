#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "command_line.h"
#include "cli.h"
#include "cpg/port.h"
#include "cpg/console_control.h"
#include "cpg/win_handle.h"
#include "../../tests/support/test_support.h"
using namespace cpg;
using namespace cpgtest;

TEST_CASE("interactive spellings resolve the same absolute default once") {
    TemporaryDirectory dir;
    const auto expected = dir.path / L"settings.toml";
    for (const auto &args : std::vector<std::vector<std::wstring>>{{}, {L"console"}, {L"--plain"}, {L"console", L"--plain"}}) {
        const auto command = parseCommandLine(args);
        CHECK(command.kind == CommandKind::Interactive);
        int calls = 0;
        const auto path = resolveConfigPath(command, [&] { ++calls; return expected; });
        CHECK(path == expected);
        CHECK(path.is_absolute());
        CHECK(calls == 1);
    }
}
TEST_CASE("relative Unicode config and role words are path values") {
    for (const auto &path : {L"bridge", L"中文 路径/配置.toml"}) {
        const auto command = parseCommandLine({L"--config", path});
        CHECK(command.kind == CommandKind::Interactive);
        const auto resolved = resolveConfigPath(command, []()->std::filesystem::path { throw std::runtime_error("must not resolve default"); });
        CHECK(resolved == std::filesystem::absolute(path).lexically_normal());
    }
    CHECK(parseCommandLine({L"--config=--smoke-test"}).configOverride == std::filesystem::path(L"--smoke-test"));
}
TEST_CASE("invalid options fail before configuration access") {
    const std::vector<std::vector<std::wstring>> cases = {
        {L"--config="}, {L"--config", L""}, {L"--config"},
        {L"--config", L"--plain"}, {L"init-config", L"--proxy-port", L"--force"},
        {L"--config=a", L"--config=b"}, {L"--unknown"}, {L"bridge"},
        {L"launch", L"--plain"}, {L"launch", L"--refresh-codex-daemon", L"--activation-only"},
        {L"init-config", L"--proxy-port=7890abc"}, {L"init-config", L"--proxy-port=1.5"},
        {L"init-config", L"--proxy-port=0"}, {L"init-config", L"--proxy-port=65536"},
        {L"init-config", L"--proxy-host=localhost", L"--proxy-host=127.0.0.1"},
        {L"--help", L"launch"}, {L"--help=anything"}, {L"--build-info"},
        {L"internal-activate-package", L"--config=unused"}};
    for (const auto &args : cases) CHECK(errorCode([&] { (void)parseCommandLine(args); }) == "CLI_INVALID");
}
TEST_CASE("strict port has one shared ASCII contract") {
    CHECK(parsePortStrict("1") == 1);
    CHECK(parsePortStrict("65535") == 65535);
    for (const auto *text : {"", "0", "65536", "7890abc", "1.5", "-1", " 80", "８０"})
        CHECK(errorCode([&] { (void)parsePortStrict(text); }) == "CONFIG_INVALID");
}
namespace {
std::filesystem::path observedPath;
bool observedPlain = false;
int observeEntry(const std::filesystem::path &path, bool plain) { observedPath = path; observedPlain = plain; return 37; }
}
TEST_CASE("production dispatch passes resolved paths to every interactive entry") {
    TemporaryDirectory dir;
    const auto path = dir.path / L"entry.toml";
    setInteractiveConsoleEntry(&observeEntry);
    for (const auto &prefix : std::vector<std::vector<std::wstring>>{{}, {L"console"}, {L"--plain"}}) {
        auto args = prefix;
        args.insert(args.end(), {L"--config", path.wstring()});
        CHECK(runCli(args) == 37);
        CHECK(observedPath == path);
        CHECK(observedPath.is_absolute());
        CHECK(observedPlain == (prefix == std::vector<std::wstring>{L"--plain"}));
    }
    setInteractiveConsoleEntry(nullptr);
    CHECK_FALSE(std::filesystem::exists(path));
}
TEST_CASE("CLI owner translates real Ctrl C and BREAK and reaps the operation") {
    wchar_t executable[32768]{};
    REQUIRE(GetModuleFileNameW(nullptr, executable, 32768) > 0);
    for (const auto *mode : {L"cancel", L"receipt", L"events"}) {
        std::wstring command = L"\"" + std::wstring(executable) + L"\" --control-test " + mode;
        STARTUPINFOW startup{};
        startup.cb = sizeof startup;
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = SW_HIDE;
        PROCESS_INFORMATION child{};
        REQUIRE(CreateProcessW(executable, command.data(), nullptr, nullptr, FALSE,
            CREATE_NEW_CONSOLE, nullptr, nullptr, &startup, &child));
        UniqueHandle process(child.hProcess), thread(child.hThread);
        const DWORD wait = WaitForSingleObject(process.get(), 10000);
        if (wait != WAIT_OBJECT_0) {
            CHECK(TerminateProcess(process.get(), 99));
            CHECK(WaitForSingleObject(process.get(), 5000) == WAIT_OBJECT_0);
        }
        REQUIRE(wait == WAIT_OBJECT_0);
        DWORD exitCode = 99;
        REQUIRE(GetExitCodeProcess(process.get(), &exitCode));
        CHECK(exitCode == 0);
    }
}

TEST_CASE("private console close waits for owner cleanup and has a bounded deadline") {
    wchar_t executable[32768]{};
    REQUIRE(GetModuleFileNameW(nullptr, executable, 32768) > 0);
    for (const auto *mode : {L"settle", L"deadline"}) {
        const std::wstring markerName = L"Local\\CPG-close-test-"
            + std::to_wstring(GetCurrentProcessId()) + L"-" + mode;
        UniqueHandle marker(CreateEventW(nullptr, TRUE, FALSE, markerName.c_str()));
        REQUIRE(marker);
        std::wstring command = L"\"" + std::wstring(executable) + L"\" --control-close-test "
            + mode + L" " + markerName;
        STARTUPINFOW startup{};
        startup.cb = sizeof startup;
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = SW_HIDE;
        PROCESS_INFORMATION child{};
        const auto started = GetTickCount64();
        REQUIRE(CreateProcessW(executable, command.data(), nullptr, nullptr, FALSE,
            CREATE_NEW_CONSOLE, nullptr, nullptr, &startup, &child));
        UniqueHandle process(child.hProcess), thread(child.hThread);
        const DWORD wait = WaitForSingleObject(process.get(), 8000);
        const auto elapsed = GetTickCount64() - started;
        if (wait != WAIT_OBJECT_0) {
            CHECK(TerminateProcess(process.get(), 99));
            CHECK(WaitForSingleObject(process.get(), 5000) == WAIT_OBJECT_0);
        }
        REQUIRE(wait == WAIT_OBJECT_0);
        if (std::wstring_view(mode) == L"settle")
            CHECK(WaitForSingleObject(marker.get(), 0) == WAIT_OBJECT_0);
        else {
            CHECK(WaitForSingleObject(marker.get(), 0) == WAIT_TIMEOUT);
            CHECK(elapsed >= 3000);
            CHECK(elapsed < 6500);
        }
    }
}
int main(int argc, char **argv) {
    if (argc == 4 && std::string(argv[1]) == "--control-close-test") {
        // Only this child owns its NEW_CONSOLE. Never send a console-close
        // message to the test runner's console or any user's application.
        const std::string mode = argv[2];
        UniqueHandle marker(OpenEventA(EVENT_MODIFY_STATE, FALSE, argv[3]));
        if (!marker) return 20;
        ConsoleControlScope controls;
        const HWND window = GetConsoleWindow();
        if (!window || !PostMessageW(window, WM_CLOSE, 0, 0)) return 21;
        if (mode == "deadline") { Sleep(10000); return 22; }
        if (WaitForSingleObject(controls.signalEvent(), 5000) != WAIT_OBJECT_0) return 23;
        if (!controls.takeCancel()) return 24;
        // Taking one request must preserve the other and keep input awake.
        if (WaitForSingleObject(controls.signalEvent(), 0) != WAIT_OBJECT_0) return 25;
        if (!controls.takeClose()) return 26;
        if (WaitForSingleObject(controls.signalEvent(), 0) != WAIT_TIMEOUT) return 27;
        // Represents a foreground operation unwinding and reaping its helper.
        Sleep(200);
        SetEvent(marker.get());
        return 0; // the scope acknowledges only after cleanup above finished
    }
    if (argc == 3 && std::string(argv[1]) == "--control-test") {
        if (std::string(argv[2]) == "events") {
            ConsoleControlScope controls;
            if (errorCode([&] { ConsoleControlScope duplicate; }) != "CONSOLE_CONTROL_FAILED") return 30;
            for (int index = 0; index < 16; ++index) {
                if (!GenerateConsoleCtrlEvent(index % 2 ? CTRL_C_EVENT : CTRL_BREAK_EVENT, 0)) return 31;
                if (WaitForSingleObject(controls.signalEvent(), 5000) != WAIT_OBJECT_0) return 32;
                if (controls.takeClose()) return 33;
                if (WaitForSingleObject(controls.signalEvent(), 0) != WAIT_OBJECT_0) return 34;
                if (!controls.takeCancel()) return 35;
                if (WaitForSingleObject(controls.signalEvent(), 0) != WAIT_TIMEOUT) return 36;
            }
            return 0;
        }
        const bool receipt = std::string(argv[2]) == "receipt";
        try {
            const auto value = runConsoleOperation([&](const Cancellation &cancel) -> Json {
                if (!GenerateConsoleCtrlEvent(receipt ? CTRL_BREAK_EVENT : CTRL_C_EVENT, 0))
                    throw Error("TEST_FAILED", "Could not signal the private test console.");
                if (WaitForSingleObject(cancel.eventHandle(), 5000) != WAIT_OBJECT_0)
                    throw Error("TEST_FAILED", "Console cancellation was not delivered.");
                if (!receipt) cancel.check();
                return Json{{"activation_state", "returned"}, {"pid", 1234}};
            });
            if (!receipt || value.at("pid") != 1234) return 2;
            // Scope unregisters cleanly and can be acquired again.
            ConsoleControlScope next;
            return next.takeCancel() ? 3 : 0;
        } catch (const Error &error) {
            return !receipt && error.code == "LAUNCH_CANCELLED" ? 0 : 4;
        }
    }
    return runDoctestMain(argc, argv);
}
