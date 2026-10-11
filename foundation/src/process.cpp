#include "cpg/process.h"
#include "cpg/error.h"
#include "cpg/win_handle.h"

#include <atomic>
#include <thread>

namespace cpg {
namespace {

wchar_t asciiLower(wchar_t value)
{
    return value >= L'A' && value <= L'Z' ? value - L'A' + L'a' : value;
}

bool iequalsWide(std::wstring_view left, std::wstring_view right)
{
    if (left.size() != right.size()) return false;
    for (std::size_t index = 0; index < left.size(); ++index)
        if (asciiLower(left[index]) != asciiLower(right[index])) return false;
    return true;
}

// Canonical Windows command-line quoting: 2n backslashes before a quote (or
// at the end of the argument) become 2n+1, and the argument is always
// wrapped in quotes so empty/space-containing values stay one argument.
void appendQuoted(std::wstring &command, std::wstring_view argument)
{
    command.push_back(L'"');
    std::size_t backslashes = 0;
    for (const wchar_t value : argument) {
        if (value == L'\\') {
            ++backslashes;
            continue;
        }
        if (value == L'"') command.append(backslashes * 2 + 1, L'\\');
        else command.append(backslashes, L'\\');
        backslashes = 0;
        command.push_back(value);
    }
    command.append(backslashes * 2, L'\\');
    command.push_back(L'"');
}

} // namespace

UnicodeEnvironment UnicodeEnvironment::current()
{
    UnicodeEnvironment environment;
    const wchar_t *block = GetEnvironmentStringsW();
    if (!block) throw Error("ENVIRONMENT_UNAVAILABLE", "Cannot read the process environment.");
    struct FreeStrings { wchar_t *block; ~FreeStrings() { FreeEnvironmentStringsW(block); } } freeStrings{const_cast<wchar_t *>(block)};
    for (const wchar_t *entry = block; *entry != L'\0'; ) {
        const std::wstring_view text(entry);
        const std::size_t equal = text.find(L'=');
        if (equal != std::wstring_view::npos) {
            const std::wstring key(text.substr(0, equal));
            const std::wstring value(text.substr(equal + 1));
            // Entries of the form "=C:=..." are hidden per-drive state.
            if (key.empty() || key[0] != L'=') environment.entries_.emplace_back(key, value);
        }
        entry += text.size() + 1;
    }
    return environment;
}

void UnicodeEnvironment::set(std::wstring key, std::wstring value)
{
    for (auto &entry : entries_) {
        if (iequalsWide(entry.first, key)) {
            entry.second = std::move(value);
            block_.clear();
            return;
        }
    }
    entries_.emplace_back(std::move(key), std::move(value));
    block_.clear();
}

void UnicodeEnvironment::remove(std::wstring_view key)
{
    for (auto entry = entries_.begin(); entry != entries_.end(); ++entry) {
        if (iequalsWide(entry->first, key)) {
            entries_.erase(entry);
            block_.clear();
            return;
        }
    }
}

bool UnicodeEnvironment::has(std::wstring_view key) const
{
    return value(key).has_value();
}

std::optional<std::wstring> UnicodeEnvironment::value(std::wstring_view key) const
{
    for (const auto &entry : entries_)
        if (iequalsWide(entry.first, key)) return entry.second;
    return std::nullopt;
}

const std::vector<wchar_t> &UnicodeEnvironment::block() const
{
    if (block_.empty()) {
        for (const auto &entry : entries_) {
            block_.insert(block_.end(), entry.first.begin(), entry.first.end());
            block_.push_back(L'=');
            block_.insert(block_.end(), entry.second.begin(), entry.second.end());
            block_.push_back(L'\0');
        }
        // One entry-terminating NUL per entry plus one final NUL; an empty
        // environment therefore keeps a valid double-NUL block.
        block_.push_back(L'\0');
        if (block_.size() == 1) block_.push_back(L'\0');
    }
    return block_;
}

std::filesystem::path currentExecutablePath()
{
    std::wstring path(MAX_PATH, L'\0');
    for (;;) {
        const DWORD size = GetModuleFileNameW(nullptr, path.data(), static_cast<DWORD>(path.size()));
        if (size > 0 && size < path.size()) {
            path.resize(size);
            return std::filesystem::path(path);
        }
        if (size == 0)
            throw Error("EXECUTABLE_PATH_UNAVAILABLE", "Cannot resolve the running executable.");
        path.resize(path.size() * 2);
    }
}

ProcessResult runOwnedProcess(const ProcessRequest &request, const Cancellation &cancel)
{
    cancel.check();
    if (request.timeout <= std::chrono::milliseconds::zero())
        throw Error("HELPER_BUDGET_INVALID", "Invalid helper input or budget");
    if (request.stdoutLimit == 0 || request.stderrLimit == 0
        || request.stdinBytes.size() > helperRequestLimit)
        throw Error("HELPER_BUDGET_INVALID", "Invalid helper input or budget");

    struct Pipe {
        UniqueHandle read;
        UniqueHandle write;
    };
    SECURITY_ATTRIBUTES security{sizeof(SECURITY_ATTRIBUTES), nullptr, TRUE};
    auto createPipe = [&] {
        Pipe pipe;
        HANDLE read = nullptr, write = nullptr;
        if (!CreatePipe(&read, &write, &security, 0))
            throw Error("HELPER_IO_FAILED", "Cannot create the helper pipes.");
        pipe.read.reset(read);
        pipe.write.reset(write);
        return pipe;
    };
    Pipe stdinPipe;   // parent writes -> child reads
    Pipe stdoutPipe;  // child writes -> parent reads
    Pipe stderrPipe;
    stdinPipe = createPipe();
    stdoutPipe = createPipe();
    stderrPipe = createPipe();
    SetHandleInformation(stdinPipe.write.get(), HANDLE_FLAG_INHERIT, 0);
    SetHandleInformation(stdoutPipe.read.get(), HANDLE_FLAG_INHERIT, 0);
    SetHandleInformation(stderrPipe.read.get(), HANDLE_FLAG_INHERIT, 0);

    const std::wstring program = request.program.wstring();
    std::wstring commandLine;
    appendQuoted(commandLine, program);
    for (const std::wstring &argument : request.arguments) {
        commandLine.push_back(L' ');
        appendQuoted(commandLine, argument);
    }
    const std::vector<wchar_t> &environmentBlock = request.environment.block();

    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdInput = stdinPipe.read.get();
    startup.hStdOutput = stdoutPipe.write.get();
    startup.hStdError = stderrPipe.write.get();
    PROCESS_INFORMATION processInfo{};
    const DWORD flags = CREATE_UNICODE_ENVIRONMENT
        | (request.createNoWindow ? CREATE_NO_WINDOW : 0);
    if (!CreateProcessW(program.c_str(), commandLine.data(), nullptr, nullptr, TRUE, flags,
                        const_cast<wchar_t *>(environmentBlock.data()), nullptr, &startup,
                        &processInfo))
        throw Error("HELPER_START_FAILED", "Cannot start owned helper");
    UniqueHandle processHandle(processInfo.hProcess);
    UniqueHandle threadHandle(processInfo.hThread);
    stdinPipe.read.reset();
    stdoutPipe.write.reset();
    stderrPipe.write.reset();

    ProcessResult result;
    struct Reader {
        std::string bytes;
        std::atomic_bool overLimit{false};
    } stdoutReader, stderrReader;

    const auto started = std::chrono::steady_clock::now();
    const auto timedOut = [&] {
        return std::chrono::steady_clock::now() - started >= request.timeout;
    };

    // Terminate and reap only this owned direct child, never a process tree.
    const auto abort = [&](const char *code, const std::string &message) {
        TerminateProcess(processHandle.get(), EXIT_FAILURE);
        WaitForSingleObject(processHandle.get(), 3000);
        stdinPipe.write.reset();
        if (request.activationSubmission && result.submitted)
            throw Error("APPX_ACTIVATION_OUTCOME_UNKNOWN",
                        "Activation request was submitted; refresh Desktop state before retrying. "
                            + message);
        throw Error(code, message);
    };
    const auto checkState = [&] {
        if (stdoutReader.overLimit.load() || stderrReader.overLimit.load())
            abort("HELPER_OUTPUT_LIMIT", "Owned helper exceeded its output budget");
        if (cancel.cancelled()) abort("LAUNCH_CANCELLED", "Operation cancelled");
        if (timedOut()) abort("HELPER_TIMEOUT", "Owned helper exceeded its time budget");
    };

    auto readerLoop = [](HANDLE pipe, std::size_t limit, Reader &reader) {
        std::string buffer(4096, '\0');
        for (;;) {
            DWORD got = 0;
            if (!ReadFile(pipe, buffer.data(), static_cast<DWORD>(buffer.size()), &got, nullptr))
                return; // broken pipe: the child side is gone
            if (got == 0) return;
            if (reader.bytes.size() + got > limit) {
                reader.overLimit.store(true);
                return;
            }
            reader.bytes.append(buffer.data(), got);
        }
    };
    std::jthread stdoutThread(readerLoop, stdoutPipe.read.get(), request.stdoutLimit,
                              std::ref(stdoutReader));
    std::jthread stderrThread(readerLoop, stderrPipe.read.get(), request.stderrLimit,
                              std::ref(stderrReader));

    try {
        if (!request.stdinBytes.empty()) {
            std::size_t written = 0;
            while (written < request.stdinBytes.size()) {
                checkState();
                const std::size_t remaining = request.stdinBytes.size() - written;
                const std::size_t chunk = remaining < 4096 ? remaining : 4096;
                DWORD count = 0;
                if (!WriteFile(stdinPipe.write.get(), request.stdinBytes.data() + written,
                               static_cast<DWORD>(chunk), &count, nullptr)
                    || count != chunk)
                    abort("HELPER_IO_FAILED", "Cannot deliver complete helper request");
                written += count;
            }
            result.submitted = true;
        }
        stdinPipe.write.reset(); // EOF for the child, like closeWriteChannel()

        HANDLE handles[2] = {processHandle.get(), cancel.eventHandle()};
        const DWORD handleCount = cancel.eventHandle() ? 2 : 1;
        for (;;) {
            const DWORD wait = WaitForMultipleObjects(handleCount, handles, FALSE, 25);
            if (wait == WAIT_OBJECT_0) break; // process exited
            checkState();
        }
        DWORD code = 0;
        if (!GetExitCodeProcess(processHandle.get(), &code))
            abort("HELPER_IO_FAILED", "Cannot observe the helper exit");
        result.normalExit = code < 0x80000000; // NTSTATUS fatal exits are abnormal
        result.exitCode = result.normalExit ? static_cast<int>(code) : -1;
        checkState(); // a cancel/timeout observed after exit still reports
    } catch (...) {
        stdinPipe.write.reset();
        // Unblock readers still parked in ReadFile; the terminated child
        // closes the write side, but CancelIoEx bounds any straggler.
        CancelIoEx(stdoutPipe.read.get(), nullptr);
        CancelIoEx(stderrPipe.read.get(), nullptr);
        stdoutThread.join();
        stderrThread.join();
        stdoutPipe.read.reset();
        stderrPipe.read.reset();
        throw;
    }
    stdoutThread.join();
    stderrThread.join();
    stdoutPipe.read.reset();
    stderrPipe.read.reset();
    result.out = std::move(stdoutReader.bytes);
    result.err = std::move(stderrReader.bytes);
    return result;
}

} // namespace cpg
