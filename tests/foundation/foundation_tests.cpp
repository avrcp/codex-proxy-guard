#define _CRT_SECURE_NO_WARNINGS
#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "cpg/build_info.h"
#include "cpg/cancellation.h"
#include "cpg/error.h"
#include "cpg/json.h"
#include "cpg/path.h"
#include "cpg/process.h"
#include "cpg/text.h"
#include "cpg/win_handle.h"
#include "process_test_hooks.h"

#include <windows.h>

#include <cstdio>
#include <array>
#include <atomic>
#include <barrier>
#include <future>
#include <filesystem>
#include <string>
#include <thread>
#include <vector>

using namespace cpg;

namespace {

std::string errorCode(const std::function<void()> &operation)
{
    try { operation(); } catch (const Error &error) { return error.code; }
    return {};
}

struct TemporaryDirectory {
    std::filesystem::path path;
    TemporaryDirectory()
    {
        wchar_t base[MAX_PATH] = {};
        GetTempPathW(MAX_PATH, base);
        wchar_t unique[MAX_PATH] = {};
        GetTempFileNameW(base, L"cpg", 0, unique);
        std::filesystem::remove(unique);
        path = unique;
        std::filesystem::create_directories(path);
    }
    ~TemporaryDirectory() { std::error_code ignored; std::filesystem::remove_all(path, ignored); }
};

ProcessRequest selfRequest(std::vector<std::wstring> arguments)
{
    ProcessRequest request;
    request.program = currentExecutablePath();
    request.arguments = std::move(arguments);
    request.environment = UnicodeEnvironment::current();
    return request;
}

std::wstring sentinelName()
{
    GUID id{};
    if (FAILED(CoCreateGuid(&id))) throw std::runtime_error("Cannot create sentinel identity");
    wchar_t text[39]{};
    if (StringFromGUID2(id, text, static_cast<int>(std::size(text))) == 0)
        throw std::runtime_error("Cannot format sentinel identity");
    return std::wstring(L"Local\\CPG-handle-sentinel-") + text;
}

int probeSentinel(HANDLE raw, const std::wstring &name)
{
    // Handle numbers are process-local slots. Snapshot the current object
    // BEFORE OpenEvent: opening the reference may itself reuse an empty slot.
    HANDLE duplicate = nullptr;
    if (!DuplicateHandle(GetCurrentProcess(), raw, GetCurrentProcess(), &duplicate,
                         0, FALSE, DUPLICATE_SAME_ACCESS)
        && GetLastError() != ERROR_INVALID_HANDLE) return 17;
    UniqueHandle candidate(duplicate);
    UniqueHandle expected(OpenEventW(SYNCHRONIZE | EVENT_MODIFY_STATE, FALSE, name.c_str()));
    if (!expected) return 18;
    if (!candidate || !CompareObjectHandles(candidate.get(), expected.get())) return 0;
    // A real inheritance leak must fail AND be visible through the parent's
    // independent sentinel observation. Never waive a matching kernel object.
    return SetEvent(candidate.get()) ? 7 : 19;
}

} // namespace

TEST_CASE("build info is embedded")
{
    CHECK(std::string(productVersion()).size() > 0);
    CHECK(std::string(buildCommit()).size() > 0);
}

TEST_CASE("strictUtf8 accepts and rejects the boundary forms")
{
    CHECK(strictUtf8("plain ascii"));
    CHECK(strictUtf8(std::string("\xe4\xb8\xad\xe6\x96\x87\xf0\x9f\x99\x82", 12)));
    CHECK_FALSE(strictUtf8(std::string("\xe2\x82", 2)));           // truncated
    CHECK_FALSE(strictUtf8(std::string("\xc0\x80", 2)));           // overlong NUL
    CHECK_FALSE(strictUtf8(std::string("\xed\xa0\x80", 3)));       // surrogate
    CHECK_FALSE(strictUtf8(std::string("\xf4\x90\x80\x80", 4)));   // beyond U+10FFFF
    CHECK_THROWS_AS(utf8ToWide(std::string_view("\xff", 1)), Error);
    CHECK_THROWS_AS(wideToUtf8(std::wstring(L"\xd800", 1)), Error);
    CHECK(utf8ToWide("aé") == std::wstring(L"a\x00e9"));
    CHECK(wideToUtf8(L"a\x00e9") == "a\xc3\xa9");
}

TEST_CASE("publicText redacts before truncation")
{
    const auto safe = publicText("Authorization: Bearer private-value", 1000);
    CHECK(safe.find("[REDACTED]") != std::string::npos);
    CHECK(safe.find("private-value") == std::string::npos);
    CHECK(publicText("http://alice:password@localhost/?token=abc", 1000)
          == "http://[REDACTED]@localhost/?token=[REDACTED]");
    CHECK(publicText("Cookie: value\nInstall safely", 1000) == "Cookie:[REDACTED] Install safely");
    CHECK(publicText("token = abc\nnext", 1000) == "token =[REDACTED] next");
    CHECK(publicText("api-key=zzz", 1000) == "api-key=[REDACTED]");
    CHECK(publicText("Password=zzz", 1000) == "Password=[REDACTED]");
    CHECK(publicText("proxy-authorization: Bearer x", 1000) == "proxy-authorization:[REDACTED]");
    CHECK(publicText("no secret here", 1000) == "no secret here");
    CHECK(publicText("\x1b[31mred\x1b[0m\r\nnext", 1000).find('\x1b') == std::string::npos);
    CHECK(publicText("\x1b]0;title" "\x07" "after", 1000) == "after");
    CHECK(publicText("123456", 3) == "123");
    CHECK(publicText("a  b\t\tc", 1000) == "a b c");
    CHECK(publicText("中文", 1) == "\xe4\xb8\xad"); // truncate by code point
    const wchar_t *profile = _wgetenv(L"USERPROFILE");
    if (profile && *profile) {
        const std::string suffix = "/file";
        const std::string combined = wideToUtf8(profile) + suffix;
        CHECK(publicText(combined, 1000).find(combined) == std::string::npos);
    }
}

TEST_CASE("strict JSON object parsing")
{
    const Json value = parseStrictJsonObject("{\"ok\": [1,true,null,{\"x\":\"y\"}]}", 1000, "JSON_BAD");
    CHECK(value.at("ok").size() == 4);
    const std::string invalid[] = {
        "", "[]", "null", "{} trailing", "{\"a\":1,\"a\":2}", "{\"a\":1,\"\\u0061\":2}",
        "{\"outer\":[{\"x\":1,\"x\":2}]}", "{\"a\":}", "{\"a\":1}//c",
        std::string("{\"x\":\"") + std::string("\xc0\x80", 2) + "\"}",
        std::string("{\"x\":\"") + std::string("\xed\xa0\x80", 3) + "\"}",
    };
    for (const std::string &bytes : invalid)
        CHECK(errorCode([&] { parseStrictJsonObject(bytes, 1000, "JSON_BAD"); }) == "JSON_BAD");
    const std::string nested = "{\"a\":" + std::string(70, '[') + "0" + std::string(70, ']') + "}";
    CHECK(errorCode([&] { parseStrictJsonObject(nested, 1000, "JSON_BAD"); }) == "JSON_BAD");
    CHECK(errorCode([&] { parseStrictJsonObject("{}", 1, "LIMIT"); }) == "LIMIT");
    CHECK(errorCode([&] { parseStrictJsonObject("{}", 0, "LIMIT"); }) == "LIMIT");
    // A UTF-8 BOM is tolerated exactly like the previous parser.
    CHECK(parseStrictJsonObject(std::string("\xef\xbb\xbf{}"), 100, "JSON_BAD") == Json::object());
    CHECK(jsonDump(Json{{"b", 1}, {"a", true}}) == R"({"a":true,"b":1})");
}

TEST_CASE("Windows path normalization")
{
    CHECK(normalizedWindowsPath(L"\\\\?\\C:\\Fixture\\App.exe") == L"c:/fixture/app.exe");
    CHECK(normalizedWindowsPath(L"\\\\?\\UNC\\server\\share\\App.exe") == L"//server/share/app.exe");
    CHECK(normalizedWindowsPath(L"C:\\a\\\\b/./c") == L"c:/a/b/c");
    CHECK(normalizedWindowsPath(L"C:/a/../b") == L"c:/b");
    CHECK(normalizedWindowsPath(L"C:\\..\\b") == L"c:/b");
    CHECK(normalizedWindowsPath(L"//server/share/../x") == L"//server/share/x");
    CHECK(normalizedWindowsPath(L"a/../../b") == L"../b");
    CHECK(normalizedWindowsPath(L"C:/FIXTURE/App.EXE") == normalizedWindowsPath(L"c:/fixture/app.exe"));
    CHECK(normalizedWindowsPath(utf8ToWide("C:\\Fixture\\App.exe")) == L"c:/fixture/app.exe");
}

TEST_CASE("path containment and canonical resolution")
{
    const std::filesystem::path root = L"C:\\Fixture\\App";
    CHECK(pathWithin(root, L"C:\\FIXTURE\\app\\child.exe"));
    CHECK(pathWithin(root, L"\\\\?\\C:\\Fixture\\App"));
    CHECK_FALSE(pathWithin(root, L"C:\\Fixture\\Application.exe"));
    CHECK_FALSE(pathWithin(root, L"D:\\Fixture\\App\\x.exe"));
    TemporaryDirectory temporary;
    const std::filesystem::path file = temporary.path / "sample.txt";
    { FILE *handle = _wfopen(file.c_str(), L"wb"); fputs("x", handle); fclose(handle); }
    REQUIRE(canonicalPathOf(file, false).has_value());
    CHECK(normalizedWindowsPath(canonicalPathOf(file, false)->wstring())
          == normalizedWindowsPath(file.wstring()));
    CHECK_FALSE(canonicalPathOf(file, true).has_value());
    CHECK_FALSE(canonicalPathOf(temporary.path / "missing.txt", false).has_value());
    REQUIRE(canonicalPathOf(temporary.path, true).has_value());
    CHECK(normalizedWindowsPath(canonicalPathOf(temporary.path, true)->wstring())
          == normalizedWindowsPath(temporary.path.wstring()));
    CHECK(knownFolder(FOLDERID_Profile).is_absolute());
}

TEST_CASE("cancellation flag and event")
{
    Cancellation cancellation;
    CHECK_FALSE(cancellation.cancelled());
    CHECK_NOTHROW(cancellation.check());
    cancellation.cancel();
    CHECK(cancellation.cancelled());
    CHECK_THROWS_AS(cancellation.check(), Error);
    CHECK(WaitForSingleObject(cancellation.eventHandle(), 0) == WAIT_OBJECT_0);
    const Cancellation copy = cancellation; // shared state
    CHECK(copy.cancelled());
}

TEST_CASE("environment blocks are case-insensitive and NUL-terminated")
{
    UnicodeEnvironment environment;
    environment.set(L"HTTP_PROXY", L"a");
    environment.set(L"http_proxy", L"b"); // replaces the first spelling
    CHECK(environment.value(L"HTTP_ProxY").value() == L"b");
    environment.remove(L"all_PROXY");
    environment.set(L"NO_PROXY", L"c");
    const std::vector<wchar_t> &block = environment.block();
    CHECK(block.size() >= 5);
    CHECK(block[block.size() - 1] == L'\0');
    CHECK(block[block.size() - 2] == L'\0');
    std::wstring flat(block.begin(), block.end());
    CHECK(flat.find(L"HTTP_PROXY=b") != std::wstring::npos);
    CHECK(flat.find(L"NO_PROXY=c") != std::wstring::npos);
    CHECK(flat.find(L"=") != std::wstring::npos);
    UnicodeEnvironment current = UnicodeEnvironment::current();
    CHECK((current.has(L"PATH") || current.has(L"path")));
    CHECK_FALSE(current.value(L"CPG_DEFINITELY_NOT_SET_x7").has_value());
    UnicodeEnvironment empty;
    CHECK(empty.block().size() >= 2);
    CHECK(empty.block()[0] == L'\0');
}

TEST_CASE("owned process runner enforces budgets")
{
    const std::filesystem::path self = currentExecutablePath();
    Cancellation alreadyCancelled;
    alreadyCancelled.cancel();
    CHECK(errorCode([&] {
        ProcessRequest request = selfRequest({L"--foundation-sleep"});
        request.timeout = std::chrono::milliseconds(1000);
        (void)runOwnedProcess(request, alreadyCancelled);
    }) == "LAUNCH_CANCELLED");
    CHECK(errorCode([&] {
        ProcessRequest request = selfRequest({L"--foundation-sleep"});
        request.timeout = std::chrono::milliseconds(100);
        (void)runOwnedProcess(request, {});
    }) == "HELPER_TIMEOUT");
    CHECK(errorCode([&] {
        ProcessRequest request = selfRequest({L"--foundation-sleep"});
        request.timeout = std::chrono::milliseconds(0);
        (void)runOwnedProcess(request, {});
    }) == "HELPER_BUDGET_INVALID");
    CHECK(errorCode([&] {
        ProcessRequest request = selfRequest({L"--foundation-sleep"});
        request.timeout = std::chrono::milliseconds(100);
        request.stdinBytes.assign(helperRequestLimit + 1, 'x');
        (void)runOwnedProcess(request, {});
    }) == "HELPER_BUDGET_INVALID");
    CHECK(errorCode([&] {
        ProcessRequest request = selfRequest({L"--foundation-output"});
        request.timeout = std::chrono::milliseconds(5000);
        request.stdoutLimit = 1024;
        (void)runOwnedProcess(request, {});
    }) == "HELPER_OUTPUT_LIMIT");
    CHECK(errorCode([&] {
        ProcessRequest request = selfRequest({L"--foundation-missing-program-test"});
        request.program = self.parent_path() / L"definitely-missing-cpg-child.exe";
        request.timeout = std::chrono::milliseconds(1000);
        (void)runOwnedProcess(request, {});
    }) == "HELPER_START_FAILED");
}

TEST_CASE("submission semantics turn post-write failures into outcome-unknown")
{
    const std::filesystem::path self = currentExecutablePath();
    ProcessRequest request;
    request.program = self;
    request.arguments = {L"--foundation-sleep"};
    request.environment = UnicodeEnvironment::current();
    request.timeout = std::chrono::milliseconds(120);
    request.stdinBytes = "complete request";
    request.activationSubmission = true;
    const auto failure = errorCode([&] { (void)runOwnedProcess(request, {}); });
    CHECK(failure == "APPX_ACTIVATION_OUTCOME_UNKNOWN");
}

TEST_CASE("cancellation mid-run and exit codes round-trip")
{
    ProcessRequest request;
    request.program = currentExecutablePath();
    request.arguments = {L"--foundation-exit", L"7"};
    request.environment = UnicodeEnvironment::current();
    request.timeout = std::chrono::milliseconds(5000);
    const ProcessResult result = runOwnedProcess(request, {});
    CHECK(result.exitCode == 7);
    CHECK(result.normalExit);

    Cancellation cancellation;
    std::jthread canceller([&cancellation] {
        Sleep(80);
        cancellation.cancel();
    });
    ProcessRequest sleepRequest = selfRequest({L"--foundation-sleep"});
    sleepRequest.timeout = std::chrono::milliseconds(5000);
    CHECK(errorCode([&] { (void)runOwnedProcess(sleepRequest, cancellation); })
          == "LAUNCH_CANCELLED");
    canceller.join();
}

TEST_CASE("stdin request bytes and environment reach the child")
{
    ProcessRequest request;
    request.program = currentExecutablePath();
    request.arguments = {L"--foundation-echo"};
    request.environment = UnicodeEnvironment::current();
    request.environment.set(L"CPG_FOUNDATION_MARKER", L"passed through \u4e2d");
    request.timeout = std::chrono::milliseconds(5000);
    request.stdinBytes = "request payload \u4e2d\u6587";
    const ProcessResult result = runOwnedProcess(request, {});
    CHECK(result.exitCode == 0);
    CHECK(result.out.find("request payload \u4e2d\u6587") != std::string::npos);
    CHECK(result.out.find("marker=passed through \u4e2d") != std::string::npos);
    CHECK(result.err.empty());
}

TEST_CASE("the owned child inherits only its standard handles")
{
    ProcessRequest request;
    request.program = currentExecutablePath();
    request.environment = UnicodeEnvironment::current();
    request.timeout = std::chrono::milliseconds(5000);

    // Control: a non-inheritable handle never crosses, on any system.
    const auto controlName = sentinelName();
    SECURITY_ATTRIBUTES private_{sizeof(SECURITY_ATTRIBUTES), nullptr, FALSE};
    HANDLE control = CreateEventW(&private_, TRUE, FALSE, controlName.c_str());
    REQUIRE(control != nullptr);
    UniqueHandle controlGuard(control);
    request.arguments = {L"--foundation-probe-handle",
                         std::to_wstring(reinterpret_cast<std::uintptr_t>(control)), controlName};
    ProcessResult result = runOwnedProcess(request, {});
    CHECK(result.normalExit);
    CHECK(result.exitCode == 0);
    CHECK(WaitForSingleObject(control, 0) == WAIT_TIMEOUT);

    // Every inheritable handle outside the explicit std-handle list must be
    // excluded. A leak is a product failure, never an environmental waiver.
    const auto eventName = sentinelName();
    SECURITY_ATTRIBUTES inheritable{sizeof(SECURITY_ATTRIBUTES), nullptr, TRUE};
    HANDLE event = CreateEventW(&inheritable, TRUE, FALSE, eventName.c_str());
    REQUIRE(event != nullptr);
    UniqueHandle probe(event);

    // Positive control reproduces the original missing HANDLE_LIST barrier.
    // The SAME helper must identify the inherited object and signal it.
    std::wstring command = L"\"" + request.program.wstring() + L"\" --foundation-probe-handle "
        + std::to_wstring(reinterpret_cast<std::uintptr_t>(event)) + L" " + eventName;
    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION info{};
    REQUIRE(CreateProcessW(request.program.c_str(), command.data(), nullptr, nullptr, TRUE,
                           CREATE_NO_WINDOW, nullptr, nullptr, &startup, &info));
    UniqueHandle leakyChild(info.hProcess), leakyThread(info.hThread);
    const DWORD childWait = WaitForSingleObject(leakyChild.get(), 5000);
    if (childWait != WAIT_OBJECT_0) {
        CHECK(TerminateProcess(leakyChild.get(), 99));
        CHECK(WaitForSingleObject(leakyChild.get(), 5000) == WAIT_OBJECT_0);
    }
    REQUIRE(childWait == WAIT_OBJECT_0);
    DWORD leakedExit = 0;
    REQUIRE(GetExitCodeProcess(leakyChild.get(), &leakedExit));
    CHECK(leakedExit == 7);
    CHECK(WaitForSingleObject(event, 0) == WAIT_OBJECT_0);
    REQUIRE(ResetEvent(event));

    request.arguments = {L"--foundation-probe-handle",
                         std::to_wstring(reinterpret_cast<std::uintptr_t>(event)), eventName};
    result = runOwnedProcess(request, {});
    CHECK(result.normalExit);
    CHECK(WaitForSingleObject(event, 0) == WAIT_TIMEOUT);
    CHECK(result.exitCode == 0);
}

TEST_CASE("handle probes distinguish unrelated local objects from the sentinel")
{
    const auto name = sentinelName();
    UniqueHandle sentinel(CreateEventW(nullptr, TRUE, FALSE, name.c_str()));
    REQUIRE(sentinel);
    for (const auto *kind : {L"pipe", L"event"}) {
        ProcessRequest request = selfRequest({L"--foundation-probe-local-object", kind, name});
        request.timeout = std::chrono::seconds(5);
        const auto result = runOwnedProcess(request, {});
        CHECK(result.normalExit);
        CHECK(result.exitCode == 0);
        CHECK(WaitForSingleObject(sentinel.get(), 0) == WAIT_TIMEOUT);
    }
}

TEST_CASE("a child that never reads stdin still honors the budget")
{
    ProcessRequest request = selfRequest({L"--foundation-never-read-stdin"});
    request.timeout = std::chrono::milliseconds(150);
    request.stdinBytes.assign(helperRequestLimit, 'x'); // beyond the pipe buffer
    const auto start = std::chrono::steady_clock::now();
    CHECK(errorCode([&] { (void)runOwnedProcess(request, {}); }) == "HELPER_TIMEOUT");
    CHECK(std::chrono::steady_clock::now() - start < std::chrono::seconds(3));
}

TEST_CASE("reader verdicts published after child exit remain authoritative")
{
    for (bool injectReadError : {false, true}) {
        UniqueHandle publish(CreateEventW(nullptr, TRUE, FALSE, nullptr));
        REQUIRE(publish);
        testing::ProcessHooks hooks;
        hooks.afterRead = [&](bool stdoutStream, HANDLE child, DWORD &error) {
            if (!stdoutStream) return;
            if (WaitForSingleObject(child, 5000) != WAIT_OBJECT_0
                || WaitForSingleObject(publish.get(), 5000) != WAIT_OBJECT_0) {
                error = ERROR_TIMEOUT;
                return;
            }
            if (injectReadError) error = ERROR_READ_FAULT;
        };
        hooks.beforeJoin = [&] { SetEvent(publish.get()); };
        testing::ProcessHookScope scope(hooks);
        ProcessRequest request = selfRequest({L"--foundation-small-output"});
        request.timeout = std::chrono::milliseconds(5000);
        request.stdoutLimit = 1024;
        CHECK(errorCode([&] { (void)runOwnedProcess(request, {}); })
              == (injectReadError ? "HELPER_IO_FAILED" : "HELPER_OUTPUT_LIMIT"));
    }
}

TEST_CASE("stdin partial writes complete and zero progress is an I/O failure")
{
    ProcessRequest request = selfRequest({L"--foundation-echo"});
    request.stdinBytes = "all request bytes must arrive";
    request.timeout = std::chrono::seconds(5);
    testing::ProcessHooks hooks;
    testing::ProcessHookScope scope(hooks);
    hooks.write = [](HANDLE pipe, const void *bytes, DWORD size, DWORD *written) {
        return WriteFile(pipe, bytes, std::min<DWORD>(size, 3), written, nullptr);
    };
    const auto result = runOwnedProcess(request, {});
    CHECK(result.submitted);
    CHECK(result.out.find(request.stdinBytes) != std::string::npos);
    hooks.write = [](HANDLE, const void *, DWORD, DWORD *written) {
        *written = 0;
        SetLastError(ERROR_SUCCESS); // no stale last-error may turn this into success
        return TRUE;
    };
    CHECK(errorCode([&] { (void)runOwnedProcess(request, {}); }) == "HELPER_IO_FAILED");
}

TEST_CASE("post-spawn setup failures reap the direct child and started threads")
{
    // Fail before any thread and at each subsequent thread construction.
    for (int failurePoint = -1; failurePoint < 3; ++failurePoint) {
        UniqueHandle observedChild;
        testing::ProcessHooks hooks;
        hooks.afterCreate = [&](HANDLE child) {
            HANDLE duplicate = nullptr;
            if (!DuplicateHandle(GetCurrentProcess(), child, GetCurrentProcess(), &duplicate,
                                 SYNCHRONIZE, FALSE, 0)) throw std::runtime_error("duplicate");
            observedChild.reset(duplicate);
            if (failurePoint == -1) throw std::bad_alloc();
        };
        hooks.beforeThread = [&](unsigned index) {
            if (static_cast<int>(index) == failurePoint) throw std::runtime_error("thread setup");
        };
        testing::ProcessHookScope scope(hooks);
        ProcessRequest request = selfRequest({L"--foundation-sleep"});
        request.stdinBytes = "test request";
        request.timeout = std::chrono::seconds(5);
        const auto started = std::chrono::steady_clock::now();
        CHECK(errorCode([&] { (void)runOwnedProcess(request, {}); }) == "HELPER_IO_FAILED");
        REQUIRE(observedChild);
        CHECK(WaitForSingleObject(observedChild.get(), 0) == WAIT_OBJECT_0);
        CHECK(std::chrono::steady_clock::now() - started < std::chrono::seconds(3));
    }
}

TEST_CASE("cancellation is checked before spawn and before submission")
{
    for (bool beforeSpawn : {true, false}) {
        Cancellation cancellation;
        bool spawned = false;
        bool written = false;
        testing::ProcessHooks hooks;
        hooks.beforeCreate = [&] { if (beforeSpawn) cancellation.cancel(); };
        hooks.afterCreate = [&](HANDLE) { spawned = true; };
        hooks.beforeThread = [&](unsigned index) { if (index == 1) cancellation.cancel(); };
        hooks.write = [&](HANDLE, const void *, DWORD, DWORD *) { written = true; return FALSE; };
        testing::ProcessHookScope scope(hooks);
        ProcessRequest request = selfRequest({L"--foundation-sleep"});
        request.stdinBytes = "activation request";
        request.activationSubmission = true;
        request.timeout = std::chrono::seconds(5);
        CHECK(errorCode([&] { (void)runOwnedProcess(request, cancellation); }) == "LAUNCH_CANCELLED");
        CHECK(spawned == !beforeSpawn);
        CHECK_FALSE(written);
    }
}

TEST_CASE("submission is decided after a final write racing cancellation settles")
{
    Cancellation cancellation;
    UniqueHandle settling(CreateEventW(nullptr, TRUE, FALSE, nullptr));
    REQUIRE(settling);
    testing::ProcessHooks hooks;
    hooks.afterWrite = [&] {
        cancellation.cancel();
        if (WaitForSingleObject(settling.get(), 5000) != WAIT_OBJECT_0)
            throw std::runtime_error("owner did not settle");
    };
    hooks.beforeJoin = [&] { SetEvent(settling.get()); };
    testing::ProcessHookScope scope(hooks);
    ProcessRequest request = selfRequest({L"--foundation-sleep"});
    request.stdinBytes = "complete activation request";
    request.activationSubmission = true;
    request.timeout = std::chrono::seconds(5);
    CHECK(errorCode([&] { (void)runOwnedProcess(request, cancellation); })
          == "APPX_ACTIVATION_OUTCOME_UNKNOWN");
}

TEST_CASE("post-spawn setup consumes the budget before request submission")
{
    bool written = false;
    testing::ProcessHooks hooks;
    hooks.afterCreate = [](HANDLE) { Sleep(120); };
    hooks.write = [&](HANDLE, const void *, DWORD, DWORD *) { written = true; return FALSE; };
    testing::ProcessHookScope scope(hooks);
    ProcessRequest request = selfRequest({L"--foundation-sleep"});
    request.stdinBytes = "activation request";
    request.activationSubmission = true;
    request.timeout = std::chrono::milliseconds(80);
    CHECK(errorCode([&] { (void)runOwnedProcess(request, {}); }) == "HELPER_TIMEOUT");
    CHECK_FALSE(written);
}

TEST_CASE("late cancellation retains a fully collected completion receipt")
{
    Cancellation cancellation;
    testing::ProcessHooks hooks;
    hooks.beforeVerdict = [&] { cancellation.cancel(); };
    testing::ProcessHookScope scope(hooks);
    ProcessRequest request = selfRequest({L"--foundation-echo"});
    request.stdinBytes = "valid receipt payload";
    request.activationSubmission = true;
    request.timeout = std::chrono::seconds(5);
    const auto result = runOwnedProcess(request, cancellation);
    CHECK(result.submitted);
    CHECK(result.exitCode == 0);
    CHECK(result.out.find(request.stdinBytes) != std::string::npos);
}

TEST_CASE("concurrent runners do not inherit each others standard endpoints")
{
    std::barrier sync(2);
    const auto name = L"Local\\CPG-foundation-concurrent-" + std::to_wstring(GetCurrentProcessId());
    UniqueHandle release(CreateEventW(nullptr, TRUE, FALSE, name.c_str()));
    REQUIRE(release);
    const auto invoke = [&](bool slow) {
        testing::ProcessHooks hooks;
        hooks.beforeCreate = [&] { sync.arrive_and_wait(); };
        // Retain all inheritable duplicates until BOTH children exist.
        hooks.afterCreate = [&](HANDLE) { sync.arrive_and_wait(); };
        testing::ProcessHookScope scope(hooks);
        ProcessRequest request = selfRequest(slow
            ? std::vector<std::wstring>{L"--foundation-wait-event", name}
            : std::vector<std::wstring>{L"--foundation-small-output"});
        request.timeout = std::chrono::seconds(5);
        return runOwnedProcess(request, {});
    };
    auto slow = std::async(std::launch::async, [&] { return invoke(true); });
    struct SignalOnExit { HANDLE event; ~SignalOnExit() { SetEvent(event); } } signal{release.get()};
    const auto quick = invoke(false);
    CHECK(quick.exitCode == 0);
    CHECK(quick.out == std::string(2048, 'x'));
    CHECK(slow.wait_for(std::chrono::milliseconds(0)) == std::future_status::timeout);
    SetEvent(release.get());
    CHECK(slow.get().out.empty());
}

TEST_CASE("stderr-only output is independently bounded")
{
    ProcessRequest request = selfRequest({L"--foundation-stderr-output"});
    request.timeout = std::chrono::seconds(5);
    request.stderrLimit = 1024;
    try {
        (void)runOwnedProcess(request, {});
        FAIL_CHECK("stderr overflow must fail");
    } catch (const Error &error) {
        INFO(error.message);
        CHECK(error.code == "HELPER_OUTPUT_LIMIT");
    }
}

TEST_CASE("cleanup verdict follows settled handles rather than racing action errors")
{
    for (int scenario = 0; scenario < 4; ++scenario) {
        INFO("scenario " << scenario);
        const auto exitName = sentinelName();
        UniqueHandle childMayExit(CreateEventW(nullptr, TRUE, FALSE, exitName.c_str()));
        UniqueHandle readerEntered(CreateEventW(nullptr, TRUE, FALSE, nullptr));
        UniqueHandle readerMayExit(CreateEventW(nullptr, TRUE, FALSE, nullptr));
        REQUIRE(childMayExit);
        REQUIRE(readerEntered);
        REQUIRE(readerMayExit);
        bool injected = false;
        unsigned processChecks = 0;
        std::jthread releaseThread;
        testing::ProcessHooks hooks;
        hooks.afterRead = [&](bool out, HANDLE, DWORD &error) {
            if (out) {
                SetEvent(readerEntered.get());
                if (WaitForSingleObject(readerMayExit.get(), 5000) != WAIT_OBJECT_0) error = ERROR_TIMEOUT;
            } else if (WaitForSingleObject(readerEntered.get(), 5000) != WAIT_OBJECT_0) error = ERROR_TIMEOUT;
        };
        if (scenario == 0) {
            hooks.beforeJoin = [&] { SetEvent(readerMayExit.get()); };
            hooks.terminate = [&](HANDLE, DWORD) {
                injected = true;
                SetLastError(ERROR_ACCESS_DENIED);
                return FALSE;
            };
            hooks.cleanupProcessWait = [&](HANDLE process) {
                // The first termination attempt must fail before the child is
                // allowed to finish, precisely reproducing the exit race.
                if (++processChecks >= 2) SetEvent(childMayExit.get());
                return WaitForSingleObject(process, 0);
            };
        } else if (scenario == 1) {
            hooks.cancelIo = [&](HANDLE) {
                injected = true;
                SetEvent(readerMayExit.get());
                SetLastError(ERROR_INVALID_HANDLE);
                return FALSE;
            };
        } else if (scenario == 2) {
            hooks.cleanupBudget = std::chrono::milliseconds(20);
            hooks.beforeJoin = [&] {
                releaseThread = std::jthread([&] { Sleep(80); SetEvent(readerMayExit.get()); });
            };
        } else {
            hooks.cleanupProcessWait = [&](HANDLE process) -> DWORD {
                if (!injected) {
                    injected = true;
                    SetEvent(readerMayExit.get());
                    SetLastError(ERROR_INVALID_HANDLE);
                    return WAIT_FAILED;
                }
                return WaitForSingleObject(process, 0);
            };
        }
        testing::ProcessHookScope scope(hooks);
        ProcessRequest request = selfRequest({L"--foundation-cleanup-race", exitName});
        request.timeout = std::chrono::seconds(5);
        request.stderrLimit = 1024;
        try {
            (void)runOwnedProcess(request, {});
            FAIL_CHECK("output overflow must fail");
        } catch (const Error &error) {
            INFO(error.message);
            CHECK(error.code == (scenario < 2 ? "HELPER_OUTPUT_LIMIT" : "HELPER_CLEANUP_FAILED"));
            if (scenario == 2) CHECK(error.message.find("stage=settle_budget") != std::string::npos);
            if (scenario == 3) CHECK(error.message.find("stage=process_wait") != std::string::npos);
        }
        if (scenario != 2) CHECK(injected);
    }
}

TEST_CASE("a descendant retaining output is not terminated and drain is bounded")
{
    const auto suffix = std::to_wstring(GetCurrentProcessId()) + L"-" + std::to_wstring(GetTickCount64());
    const auto aliveName = L"Local\\CPG-foundation-alive-" + suffix;
    const auto doneName = L"Local\\CPG-foundation-done-" + suffix;
    UniqueHandle alive(CreateEventW(nullptr, TRUE, FALSE, aliveName.c_str()));
    UniqueHandle done(CreateEventW(nullptr, TRUE, FALSE, doneName.c_str()));
    REQUIRE(alive);
    REQUIRE(done);
    ProcessRequest request = selfRequest({L"--foundation-spawn-descendant", aliveName, doneName});
    request.timeout = std::chrono::seconds(5);
    const auto start = std::chrono::steady_clock::now();
    CHECK(errorCode([&] { (void)runOwnedProcess(request, {}); }) == "HELPER_DRAIN_TIMEOUT");
    CHECK(std::chrono::steady_clock::now() - start < std::chrono::seconds(3));
    CHECK(WaitForSingleObject(alive.get(), 0) == WAIT_OBJECT_0);
    CHECK(WaitForSingleObject(done.get(), 0) == WAIT_TIMEOUT);
    CHECK(WaitForSingleObject(done.get(), 5000) == WAIT_OBJECT_0);
}

TEST_CASE("current environment excludes drive state and removes all proxy case variants")
{
    // The custom block is intentionally not made by UnicodeEnvironment: it
    // exercises import of valid Windows drive state and duplicate raw entries.
    const wchar_t raw[] = L"=C:=C:\\fixture\0HTTP_PROXY=a\0http_proxy=b\0Z_VAR=z\0";
    std::wstring command = L"\"" + currentExecutablePath().wstring() + L"\" --foundation-check-environment";
    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION info{};
    REQUIRE(CreateProcessW(currentExecutablePath().c_str(), command.data(), nullptr, nullptr,
                           FALSE, CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                           const_cast<wchar_t *>(raw), nullptr, &startup, &info));
    UniqueHandle process(info.hProcess), thread(info.hThread);
    REQUIRE(WaitForSingleObject(process.get(), 5000) == WAIT_OBJECT_0);
    DWORD exit = 99;
    REQUIRE(GetExitCodeProcess(process.get(), &exit));
    CHECK(exit == 0);
    UnicodeEnvironment environment;
    environment.set(L"\u00e9_proxy", L"value");
    CHECK(environment.value(L"\u00c9_PROXY") == L"value");
    environment.remove(L"\u00c9_PROXY");
    CHECK_FALSE(environment.has(L"\u00e9_proxy"));
    CHECK_THROWS_AS(environment.set(L"", L"invalid"), Error);
}

TEST_CASE("environment blocks are ordered for CreateProcess")
{
    UnicodeEnvironment environment;
    environment.set(L"z_var", L"1");
    environment.set(L"A_VAR", L"2");
    environment.set(L"m_var", L"3");
    environment.set(L"HTTP_PROXY", L"a");
    environment.set(L"http_proxy", L"b"); // case variant collapses into one entry
    const std::vector<wchar_t> &block = environment.block();
    std::vector<std::wstring> names;
    for (std::size_t index = 0; index + 1 < block.size() && block[index] != L'\0';) {
        const std::wstring entry(&block[index]);
        const std::size_t equal = entry.find(L'=');
        REQUIRE(equal != std::wstring::npos);
        names.push_back(entry.substr(0, equal));
        index += entry.size() + 1;
    }
    // Case-insensitive ordinal order, exactly as CreateProcessW requires,
    // with no duplicate names left behind.
    REQUIRE(names.size() == 4);
    CHECK(names == std::vector<std::wstring>({L"A_VAR", L"HTTP_PROXY", L"m_var", L"z_var"}));
    CHECK(block[block.size() - 2] == L'\0');
    CHECK(block.back() == L'\0');
}

int main(int argc, char **argv)
{
    // Hidden child modes exercised by the runner tests above. argv is ANSI;
    // the hidden arguments are pure ASCII so no wide parsing is needed.
    for (int index = 1; index < argc; ++index) {
        const std::string argument = argv[index];
        if (argument == "--foundation-sleep") { Sleep(10000); return 0; }
        if (argument == "--foundation-wait-event" && index + 1 < argc) {
            UniqueHandle event(OpenEventW(SYNCHRONIZE, FALSE, utf8ToWide(argv[index + 1]).c_str()));
            return event && WaitForSingleObject(event.get(), 5000) == WAIT_OBJECT_0 ? 0 : 16;
        }
        if (argument == "--foundation-check-environment") {
            auto env = UnicodeEnvironment::current();
            if (env.has(L"")) return 10;
            env.remove(L"HtTp_PrOxY");
            if (env.has(L"HTTP_PROXY")) return 11;
            const auto &block = env.block();
            for (const wchar_t *entry = block.data(); *entry; entry += wcslen(entry) + 1)
                if (*entry == L'=' || std::wstring_view(entry).find(L"PROXY") != std::wstring::npos
                    || std::wstring_view(entry).find(L"proxy") != std::wstring::npos) return 12;
            return 0;
        }
        if (argument == "--foundation-spawn-descendant" && index + 2 < argc) {
            const auto alive = utf8ToWide(argv[index + 1]);
            const auto done = utf8ToWide(argv[index + 2]);
            std::wstring command = L"\"" + currentExecutablePath().wstring()
                + L"\" --foundation-descendant " + alive + L" " + done;
            STARTUPINFOW startup{};
            startup.cb = sizeof(startup);
            startup.dwFlags = STARTF_USESTDHANDLES;
            startup.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
            startup.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
            startup.hStdError = GetStdHandle(STD_ERROR_HANDLE);
            PROCESS_INFORMATION info{};
            if (!CreateProcessW(currentExecutablePath().c_str(), command.data(), nullptr, nullptr,
                                TRUE, CREATE_NO_WINDOW, nullptr, nullptr, &startup, &info)) return 13;
            CloseHandle(info.hProcess);
            CloseHandle(info.hThread);
            UniqueHandle ready(OpenEventW(SYNCHRONIZE, FALSE, alive.c_str()));
            return ready && WaitForSingleObject(ready.get(), 5000) == WAIT_OBJECT_0 ? 0 : 14;
        }
        if (argument == "--foundation-descendant" && index + 2 < argc) {
            UniqueHandle alive(OpenEventW(EVENT_MODIFY_STATE, FALSE, utf8ToWide(argv[index + 1]).c_str()));
            UniqueHandle done(OpenEventW(EVENT_MODIFY_STATE, FALSE, utf8ToWide(argv[index + 2]).c_str()));
            if (!alive || !done) return 15;
            SetEvent(alive.get());
            Sleep(1800);
            SetEvent(done.get());
            return 0;
        }
        if (argument == "--foundation-output") {
            std::string flood(65536, 'x');
            fwrite(flood.data(), 1, flood.size(), stdout);
            fflush(stdout);
            return 0;
        }
        if (argument == "--foundation-exit") return 7;
        if (argument == "--foundation-echo") {
            std::string input;
            char buffer[4096];
            size_t got = 0;
            while ((got = fread(buffer, 1, sizeof buffer, stdin)) > 0) input.append(buffer, got);
            const std::string marker = wideToUtf8(_wgetenv(L"CPG_FOUNDATION_MARKER")
                                                      ? _wgetenv(L"CPG_FOUNDATION_MARKER")
                                                      : L"");
            printf("input=%s\nmarker=%s\n", input.c_str(), marker.c_str());
            fflush(stdout);
            return 0;
        }
        if (argument == "--foundation-never-read-stdin") { Sleep(10000); return 0; }
        if (argument == "--foundation-cleanup-race" && index + 1 < argc) {
            const std::string overflow(2048, 'x');
            DWORD written = 0;
            if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), "x", 1, &written, nullptr)
                || !WriteFile(GetStdHandle(STD_ERROR_HANDLE), overflow.data(),
                              static_cast<DWORD>(overflow.size()), &written, nullptr)) return 24;
            UniqueHandle gate(OpenEventW(SYNCHRONIZE, FALSE, utf8ToWide(argv[index + 1]).c_str()));
            return gate && WaitForSingleObject(gate.get(), 5000) == WAIT_OBJECT_0 ? 0 : 25;
        }
        if (argument == "--foundation-small-output" || argument == "--foundation-stderr-output") {
            const std::string output(2048, 'x');
            DWORD written = 0;
            return WriteFile(GetStdHandle(argument == "--foundation-small-output"
                                              ? STD_OUTPUT_HANDLE : STD_ERROR_HANDLE), output.data(),
                             static_cast<DWORD>(output.size()), &written, nullptr) ? 0 : 9;
        }
        if (argument == "--foundation-probe-local-object" && index + 2 < argc) {
            const std::string kind = argv[index + 1];
            UniqueHandle localEvent;
            HANDLE candidate = GetStdHandle(STD_OUTPUT_HANDLE);
            if (kind == "event") {
                localEvent.reset(CreateEventW(nullptr, TRUE, FALSE, nullptr));
                if (!localEvent || !SetEvent(localEvent.get())) return 20;
                candidate = localEvent.get();
            } else if (GetFileType(candidate) != FILE_TYPE_PIPE) return 21;
            DWORD flags = 0;
            if (!GetHandleInformation(candidate, &flags)) return 22;
            // The old numeric-only probe reports 2 for the valid local pipe,
            // or 7 for an unrelated event. Neither refers to the sentinel.
            if (kind == "pipe" && SetEvent(candidate)) return 23;
            return probeSentinel(candidate, utf8ToWide(argv[index + 2]));
        }
        if (argument == "--foundation-probe-handle" && index + 2 < argc) {
            // 0 = no matching object; 7 = exact sentinel leaked; other codes
            // are probe/setup errors and remain strict test failures.
            const long long value = _atoi64(argv[index + 1]);
            const HANDLE probed = reinterpret_cast<HANDLE>(static_cast<LONG_PTR>(value));
            return probeSentinel(probed, utf8ToWide(argv[index + 2]));
        }
    }
    return doctest::Context(argc, argv).run();
}
