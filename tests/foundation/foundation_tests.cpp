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

#include <windows.h>

#include <cstdio>
#include <filesystem>
#include <string>
#include <thread>

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
    request.stdinBytes = "request payload 中文";
    const ProcessResult result = runOwnedProcess(request, {});
    CHECK(result.exitCode == 0);
    CHECK(result.out.find("request payload \u4e2d\u6587") != std::string::npos);
    CHECK(result.out.find("marker=passed through \u4e2d") != std::string::npos);
    CHECK(result.err.empty());
}

int main(int argc, char **argv)
{
    // Hidden child modes exercised by the runner tests above. argv is ANSI;
    // the hidden arguments are pure ASCII so no wide parsing is needed.
    for (int index = 1; index < argc; ++index) {
        const std::string argument = argv[index];
        if (argument == "--foundation-sleep") { Sleep(10000); return 0; }
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
    }
    return doctest::Context(argc, argv).run();
}
