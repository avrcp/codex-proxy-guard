#pragma once

// Shared helpers for the doctest engine suites: deterministic temporary
// directories, hex fixtures, bounded error capture and a raw pipe child used
// by the bridge transport tests.
#ifndef _CRT_SECURE_NO_WARNINGS
#define _CRT_SECURE_NO_WARNINGS
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include "cpg/error.h"
#include "cpg/path.h"
#include "cpg/process.h"
#include "cpg/text.h"

#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <functional>
#include <iostream>
#include <memory>
#include <sstream>
#include <string>
#include <system_error>
#include <vector>

namespace cpgtest {

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
    ~TemporaryDirectory()
    {
        std::error_code ignored;
        std::filesystem::remove_all(path, ignored);
    }
    std::filesystem::path file(const std::wstring &name) const { return path / name; }
};

inline std::string hexString(const char *hex)
{
    const auto digit = [](char value) -> int {
        if (value >= '0' && value <= '9') return value - '0';
        if (value >= 'a' && value <= 'f') return value - 'a' + 10;
        if (value >= 'A' && value <= 'F') return value - 'A' + 10;
        return -1;
    };
    std::string bytes;
    for (std::size_t index = 0; hex[index] && hex[index + 1]; index += 2)
        bytes.push_back(static_cast<char>((digit(hex[index]) << 4) | digit(hex[index + 1])));
    return bytes;
}

inline void writeBytes(const std::filesystem::path &path, const std::string &bytes)
{
    std::ofstream stream(path, std::ios::binary | std::ios::trunc);
    stream.write(bytes.data(), static_cast<std::streamsize>(bytes.size()));
    if (!stream) throw std::runtime_error("fixture write failed");
}

inline std::string readBytes(const std::filesystem::path &path)
{
    std::ifstream stream(path, std::ios::binary);
    std::ostringstream buffer;
    buffer << stream.rdbuf();
    return buffer.str();
}

inline std::string errorCode(const std::function<void()> &operation)
{
    try { operation(); } catch (const cpg::Error &error) { return error.code; }
    return {};
}

// A raw child with two unbuffered pipes and manual write control, used where
// the OwnedProcessRunner's single-shot stdin contract is intentionally too
// coarse (streamed frames, partial writes, stalled readers).
struct PipeProcess {
    HANDLE inputRead = nullptr, inputWrite = nullptr;
    HANDLE outputRead = nullptr, outputWrite = nullptr;
    HANDLE process = nullptr, thread = nullptr;
    ~PipeProcess()
    {
        if (process && WaitForSingleObject(process, 0) == WAIT_TIMEOUT) {
            TerminateProcess(process, 99);
            WaitForSingleObject(process, 5000);
        }
        for (auto handle : {inputRead, inputWrite, outputRead, outputWrite, process, thread})
            if (handle) CloseHandle(handle);
    }
    bool start(const std::filesystem::path &program, const std::vector<std::wstring> &arguments)
    {
        SECURITY_ATTRIBUTES security{sizeof(SECURITY_ATTRIBUTES), nullptr, TRUE};
        if (!CreatePipe(&inputRead, &inputWrite, &security, 0)) return false;
        if (!CreatePipe(&outputRead, &outputWrite, &security, 0)) return false;
        SetHandleInformation(inputWrite, HANDLE_FLAG_INHERIT, 0);
        SetHandleInformation(outputRead, HANDLE_FLAG_INHERIT, 0);
        std::wstring command = L"\"" + program.wstring() + L"\"";
        for (const std::wstring &argument : arguments) command += L" \"" + argument + L"\"";
        STARTUPINFOW startup{};
        startup.cb = sizeof(startup);
        startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdInput = inputRead;
        startup.hStdOutput = outputWrite;
        startup.hStdError = outputWrite;
        PROCESS_INFORMATION information{};
        if (!CreateProcessW(program.c_str(), command.data(), nullptr, nullptr, TRUE,
                            CREATE_NO_WINDOW, nullptr, nullptr, &startup, &information))
            return false;
        process = information.hProcess;
        thread = information.hThread;
        CloseHandle(inputRead);
        inputRead = nullptr;
        CloseHandle(outputWrite);
        outputWrite = nullptr;
        return true;
    }
    bool writeAll(const std::string &bytes)
    {
        DWORD written = 0;
        return WriteFile(inputWrite, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr)
            && written == static_cast<DWORD>(bytes.size());
    }
    std::string readAll()
    {
        std::string output;
        char buffer[4096];
        for (;;) {
            DWORD got = 0;
            if (!ReadFile(outputRead, buffer, sizeof buffer, &got, nullptr) || got == 0) break;
            output.append(buffer, got);
        }
        return output;
    }
    DWORD waitExit(DWORD milliseconds)
    {
        if (WaitForSingleObject(process, milliseconds) != WAIT_OBJECT_0) return STILL_ACTIVE;
        DWORD code = 0;
        return GetExitCodeProcess(process, &code) ? code : STILL_ACTIVE;
    }
};

inline void printLine(const std::string &text) { std::cout << text << "\n"; }

// doctest consumes argv-style UTF-8 arguments; wide-char mains funnel through
// this shim. Test binaries needing hidden child modes keep a custom main and
// call this for the default path.
inline int runDoctestWide(int argc, wchar_t **argv)
{
    std::vector<std::unique_ptr<char[]>> buffers;
    std::vector<char *> narrow;
    for (int index = 0; index < argc; ++index) {
        std::string bytes = cpg::wideToUtf8(argv[index]);
        buffers.push_back(std::make_unique<char[]>(bytes.size() + 1));
        std::memcpy(buffers.back().get(), bytes.data(), bytes.size());
        buffers.back()[bytes.size()] = '\0';
        narrow.push_back(buffers.back().get());
    }
    int narrowArgc = static_cast<int>(narrow.size());
    return doctest::Context(narrowArgc, narrow.data()).run();
}

#define CPG_TEST_MAIN_WIDE int main(int argc, wchar_t **argv) { return cpgtest::runDoctestWide(argc, argv); }

// Preferred entry: plain ANSI main (doctest parses only its own options
// through argv) with the wide command line re-derived where fixtures need it.
inline int runDoctestMain(int argc, char **argv)
{
    return doctest::Context(argc, argv).run();
}

#define CPG_TEST_MAIN int main(int argc, char **argv) { return cpgtest::runDoctestMain(argc, argv); }

} // namespace cpgtest
