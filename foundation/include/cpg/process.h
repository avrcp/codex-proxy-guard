#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include "cancellation.h"

#include <chrono>
#include <cstddef>
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace cpg {

// 16 KiB bounded request budget shared by the activation worker protocol.
inline constexpr std::size_t helperRequestLimit = 16 * 1024;

// Case-insensitive Windows environment block builder. Names fold to their
// first spelling; `block()` returns the double-NUL-terminated UTF-16 block
// sorted by name (CompareStringOrdinal, case-insensitive) exactly as
// CreateProcessW requires for custom environment blocks
// (CREATE_UNICODE_ENVIRONMENT).
class UnicodeEnvironment {
public:
    UnicodeEnvironment() = default;
    static UnicodeEnvironment current();

    void set(std::wstring key, std::wstring value);
    void remove(std::wstring_view key);
    bool has(std::wstring_view key) const;
    std::optional<std::wstring> value(std::wstring_view key) const;
    const std::vector<wchar_t> &block() const;

private:
    std::vector<std::pair<std::wstring, std::wstring>> entries_;
    mutable std::vector<wchar_t> block_;
};

struct ProcessRequest {
    std::filesystem::path program;      // absolute executable, no PATH search
    std::vector<std::wstring> arguments;
    std::string stdinBytes;             // written fully before submission is claimed
    UnicodeEnvironment environment;     // default-constructed: empty environment
    std::chrono::milliseconds timeout{std::chrono::milliseconds::zero()};
    std::size_t stdoutLimit = 64 * 1024;
    std::size_t stderrLimit = 64 * 1024;
    bool createNoWindow = true;
    // When true, any failure after the complete stdin request was accepted
    // reports APPX_ACTIVATION_OUTCOME_UNKNOWN instead of the plain cause:
    // the request may already be in effect and must not be retried blindly.
    bool activationSubmission = false;
};

struct ProcessResult {
    std::string out;
    std::string err;
    int exitCode = -1;      // -1 when the child terminated abnormally
    bool normalExit = false;
    bool submitted = false; // every stdin byte was delivered
};

// Bounded, cancellable execution of a Guard-owned direct child. Program path
// and arguments are passed separately (no cmd.exe, no shell, no PowerShell
// wrapper); stdout/stderr are drained concurrently with per-stream limits;
// setup and run share the request budget, post-exit drain is limited to 500 ms
// within that budget, and cleanup is checked against a separate 3 s budget.
// The caller remains busy until child and I/O threads actually settle, even
// if an unresponsive OS exceeds the cleanup budget (HELPER_CLEANUP_FAILED).
// TerminateProcess is only applied to this child, never to a process tree.
ProcessResult runOwnedProcess(const ProcessRequest &request, const Cancellation &cancel);

// Absolute path of the running executable (never inferred from argv[0]).
std::filesystem::path currentExecutablePath();

} // namespace cpg
