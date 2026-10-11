#include "cpg/process.h"
#include "cpg/error.h"
#include "cpg/win_handle.h"

#include <algorithm>
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
        // CreateProcessW requires a custom environment block to be sorted by
        // variable name with a case-insensitive ordinal comparison. Duplicate
        // names (case variants that survived from the current environment)
        // collapse to their first occurrence, matching set()'s semantics.
        using Entry = const std::pair<std::wstring, std::wstring>;
        std::vector<Entry *> sorted;
        sorted.reserve(entries_.size());
        for (const auto &entry : entries_) sorted.push_back(&entry);
        const auto ordinalLess = [](Entry *left, Entry *right) {
            const int order = CompareStringOrdinal(left->first.c_str(), -1,
                                                   right->first.c_str(), -1, TRUE);
            if (order != 0) return order == CSTR_LESS_THAN;
            // Unreachable for valid strings; kept as a deterministic fallback.
            return left->first < right->first;
        };
        std::stable_sort(sorted.begin(), sorted.end(), ordinalLess);
        Entry *previous = nullptr;
        for (Entry *entry : sorted) {
            if (previous) {
                const int order = CompareStringOrdinal(previous->first.c_str(), -1,
                                                       entry->first.c_str(), -1, TRUE);
                if (order == CSTR_EQUAL) continue; // same name, different spelling
            }
            block_.insert(block_.end(), entry->first.begin(), entry->first.end());
            block_.push_back(L'=');
            block_.insert(block_.end(), entry->second.begin(), entry->second.end());
            block_.push_back(L'\0');
            previous = entry;
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
    // Pipes are created NON-inheritable; the three child-side ends are then
    // duplicated into inheritable handles, which are the only inheritable
    // handles Guard ever holds at spawn time. The explicit
    // PROC_THREAD_ATTRIBUTE_HANDLE_LIST below restricts inheritance to those
    // three on systems that honor it — and on systems whose CreateProcess
    // hook strips the attribute list, there is simply nothing else left to
    // leak.
    SECURITY_ATTRIBUTES noInherit{sizeof(SECURITY_ATTRIBUTES), nullptr, FALSE};
    auto createPipe = [&] {
        Pipe pipe;
        HANDLE read = nullptr, write = nullptr;
        if (!CreatePipe(&read, &write, &noInherit, 0))
            throw Error("HELPER_IO_FAILED", "Cannot create the helper pipes.");
        pipe.read.reset(read);
        pipe.write.reset(write);
        return pipe;
    };
    auto inheritableDuplicate = [](HANDLE source) {
        HANDLE duplicate = nullptr;
        if (!DuplicateHandle(GetCurrentProcess(), source, GetCurrentProcess(), &duplicate, 0,
                             TRUE, DUPLICATE_SAME_ACCESS))
            throw Error("HELPER_IO_FAILED", "Cannot prepare the helper pipe ends.");
        return UniqueHandle(duplicate);
    };
    Pipe stdinPipe;   // parent writes -> child reads
    Pipe stdoutPipe;  // child writes -> parent reads
    Pipe stderrPipe;
    stdinPipe = createPipe();
    stdoutPipe = createPipe();
    stderrPipe = createPipe();
    UniqueHandle childStdin = inheritableDuplicate(stdinPipe.read.get());
    UniqueHandle childStdout = inheritableDuplicate(stdoutPipe.write.get());
    UniqueHandle childStderr = inheritableDuplicate(stderrPipe.write.get());

    const std::wstring program = request.program.wstring();
    std::wstring commandLine;
    appendQuoted(commandLine, program);
    for (const std::wstring &argument : request.arguments) {
        commandLine.push_back(L' ');
        appendQuoted(commandLine, argument);
    }
    const std::vector<wchar_t> &environmentBlock = request.environment.block();

    // Explicit inheritance: only the three duplicated child-side pipe handles
    // may cross into the child (PROC_THREAD_ATTRIBUTE_HANDLE_LIST).
    // bInheritHandles stays TRUE because the API requires it.
    SIZE_T attributeSize = 0;
    InitializeProcThreadAttributeList(nullptr, 1, 0, &attributeSize);
    if (GetLastError() != ERROR_INSUFFICIENT_BUFFER || attributeSize == 0)
        throw Error("HELPER_START_FAILED", "Cannot size the helper startup attributes");
    std::vector<BYTE> attributeStorage(attributeSize, 0);
    LPPROC_THREAD_ATTRIBUTE_LIST attributes =
        reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(attributeStorage.data());
    if (!InitializeProcThreadAttributeList(attributes, 1, 0, &attributeSize))
        throw Error("HELPER_START_FAILED", "Cannot prepare the helper startup attributes");
    struct AttributeListScope {
        LPPROC_THREAD_ATTRIBUTE_LIST value;
        ~AttributeListScope() { DeleteProcThreadAttributeList(value); }
    } attributeScope{attributes};
    HANDLE inheritedHandles[3] = {childStdin.get(), childStdout.get(), childStderr.get()};
    if (!UpdateProcThreadAttribute(attributes, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                                   inheritedHandles, sizeof(inheritedHandles), nullptr, nullptr))
        throw Error("HELPER_START_FAILED", "Cannot restrict helper handle inheritance");

    STARTUPINFOEXW startup{};
    startup.StartupInfo.cb = sizeof(startup);
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = childStdin.get();
    startup.StartupInfo.hStdOutput = childStdout.get();
    startup.StartupInfo.hStdError = childStderr.get();
    PROCESS_INFORMATION processInfo{};
    const DWORD flags = CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT
        | (request.createNoWindow ? CREATE_NO_WINDOW : 0);
    if (!CreateProcessW(program.c_str(), commandLine.data(), nullptr, nullptr, TRUE, flags,
                        const_cast<wchar_t *>(environmentBlock.data()), nullptr,
                        &startup.StartupInfo, &processInfo))
        throw Error("HELPER_START_FAILED", "Cannot start owned helper");
    UniqueHandle processHandle(processInfo.hProcess);
    UniqueHandle threadHandle(processInfo.hThread);
    childStdin.reset();
    childStdout.reset();
    childStderr.reset();
    stdinPipe.read.reset();
    stdoutPipe.write.reset();
    stderrPipe.write.reset();

    ProcessResult result;
    struct Reader {
        std::string bytes;
        std::atomic_bool overLimit{false};
        std::atomic<DWORD> readError{0};
    } stdoutReader, stderrReader;
    std::atomic_bool aborting{false};
    std::atomic<DWORD> writeError{0};
    std::atomic_bool submittedFlag{false};

    const auto started = std::chrono::steady_clock::now();
    const auto timedOut = [&] {
        return std::chrono::steady_clock::now() - started >= request.timeout;
    };
    // A submitted activation request may already be in effect: every failure
    // after the complete stdin request was delivered reports the outcome as
    // unknown instead of the plain cause, so it is never retried blindly.
    const auto submissionAware = [&](const char *code, const std::string &message) {
        if (request.activationSubmission && submittedFlag.load())
            return Error("APPX_ACTIVATION_OUTCOME_UNKNOWN",
                         "Activation request was submitted; refresh Desktop state before "
                         "retrying. " + message);
        return Error(code, message);
    };

    auto readerLoop = [](HANDLE pipe, std::size_t limit, Reader &reader,
                         const std::atomic_bool &aborting) {
        std::string buffer(4096, '\0');
        for (;;) {
            DWORD got = 0;
            if (!ReadFile(pipe, buffer.data(), static_cast<DWORD>(buffer.size()), &got, nullptr)) {
                const DWORD error = GetLastError();
                // A closed child side is the normal end of stream; an abort
                // we asked for is normal cleanup. Anything else is a failure.
                if (error != ERROR_BROKEN_PIPE && error != ERROR_NO_DATA
                    && !(error == ERROR_OPERATION_ABORTED && aborting.load()))
                    reader.readError.store(error);
                return;
            }
            if (got == 0) return;
            if (reader.bytes.size() + got > limit) {
                reader.overLimit.store(true);
                return;
            }
            reader.bytes.append(buffer.data(), got);
        }
    };
    std::jthread stdoutThread(readerLoop, stdoutPipe.read.get(), request.stdoutLimit,
                              std::ref(stdoutReader), std::cref(aborting));
    std::jthread stderrThread(readerLoop, stderrPipe.read.get(), request.stderrLimit,
                              std::ref(stderrReader), std::cref(aborting));

    // The stdin request is written by a dedicated thread so the orchestrating
    // thread never blocks in WriteFile: a child that refuses to read stdin
    // cannot stall the timeout/cancellation machinery. The writer owns the
    // parent's write end and closes it when done — that EOF is what lets the
    // child terminate its stdin read loop.
    UniqueHandle writerDone(CreateEventW(nullptr, TRUE, FALSE, nullptr));
    if (!writerDone) {
        aborting.store(true);
        CancelIoEx(stdoutPipe.read.get(), nullptr);
        CancelIoEx(stderrPipe.read.get(), nullptr);
        throw Error("HELPER_IO_FAILED", "Cannot create the writer event.");
    }
    const std::string &stdinBytes = request.stdinBytes;
    const auto writerLoop = [&](UniqueHandle *writeEnd) {
        const HANDLE target = writeEnd->get();
        std::size_t written = 0;
        while (written < stdinBytes.size()) {
            const std::size_t remaining = stdinBytes.size() - written;
            const std::size_t chunk = remaining < 4096 ? remaining : 4096;
            DWORD count = 0;
            if (!WriteFile(target, stdinBytes.data() + written, static_cast<DWORD>(chunk),
                           &count, nullptr)
                || count != chunk) {
                writeError.store(GetLastError());
                break;
            }
            written += count;
        }
        if (written >= stdinBytes.size()) submittedFlag.store(true);
        writeEnd->reset(); // EOF for the child, like closeWriteChannel()
        SetEvent(writerDone.get());
    };
    std::jthread writerThread;
    if (!stdinBytes.empty())
        writerThread = std::jthread(writerLoop, &stdinPipe.write);
    else
        stdinPipe.write.reset(); // no request: close immediately for EOF

    // Terminate and reap only this owned direct child, never a process tree.
    const auto abort = [&](const char *code, const std::string &message) {
        aborting.store(true);
        TerminateProcess(processHandle.get(), EXIT_FAILURE);
        WaitForSingleObject(processHandle.get(), 3000);
        if (writerThread.joinable())
            CancelSynchronousIo(writerThread.native_handle()); // unblock a parked WriteFile
        throw submissionAware(code, message);
    };
    const auto checkState = [&] {
        if (stdoutReader.overLimit.load() || stderrReader.overLimit.load())
            abort("HELPER_OUTPUT_LIMIT", "Owned helper exceeded its output budget");
        if (cancel.cancelled()) abort("LAUNCH_CANCELLED", "Operation cancelled");
        if (timedOut()) abort("HELPER_TIMEOUT", "Owned helper exceeded its time budget");
    };

    try {
        bool writerFinished = stdinBytes.empty();
        bool processExited = false;
        while (!processExited) {
            HANDLE handles[3] = {processHandle.get(), cancel.eventHandle(), writerDone.get()};
            const DWORD handleCount = writerFinished
                ? (cancel.eventHandle() ? 2u : 1u) : 3u;
            const DWORD wait = WaitForMultipleObjects(handleCount, handles, FALSE, 25);
            if (wait == WAIT_OBJECT_0) processExited = true;
            else if (!writerFinished && wait == WAIT_OBJECT_0 + 2) writerFinished = true;
            else checkState();
        }
        DWORD code = 0;
        if (!GetExitCodeProcess(processHandle.get(), &code))
            abort("HELPER_IO_FAILED", "Cannot observe the helper exit");
        result.normalExit = code < 0x80000000; // NTSTATUS fatal exits are abnormal
        result.exitCode = result.normalExit ? static_cast<int>(code) : -1;
        // The writer either completed or failed on the dead child's closed
        // pipe; only after both joins is the verdict authoritative.
        if (writerThread.joinable()) writerThread.join();
        // Authoritative post-join checks: a reader may still have delivered
        // the over-limit chunk (or a read/write error) between the process
        // exit and the joins above, and a fast-exiting child must never pass
        // as a normal result because of that race.
        if (stdoutReader.overLimit.load() || stderrReader.overLimit.load())
            throw submissionAware("HELPER_OUTPUT_LIMIT", "Owned helper exceeded its output budget");
        if (writeError.load() != 0)
            throw submissionAware("HELPER_IO_FAILED", "Cannot deliver complete helper request");
        if (stdoutReader.readError.load() != 0 || stderrReader.readError.load() != 0)
            throw submissionAware("HELPER_IO_FAILED", "Cannot read the helper output");
        if (cancel.cancelled()) throw submissionAware("LAUNCH_CANCELLED", "Operation cancelled");
        if (timedOut()) throw submissionAware("HELPER_TIMEOUT", "Owned helper exceeded its time budget");
    } catch (...) {
        aborting.store(true);
        if (writerThread.joinable()) {
            CancelSynchronousIo(writerThread.native_handle());
            writerThread.join();
        }
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
    result.submitted = submittedFlag.load();
    result.out = std::move(stdoutReader.bytes);
    result.err = std::move(stderrReader.bytes);
    return result;
}

} // namespace cpg
