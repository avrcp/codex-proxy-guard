#include "cpg/process.h"
#include "cpg/error.h"
#include "cpg/win_handle.h"

#include <algorithm>
#include <atomic>
#include <array>
#include <thread>

#ifdef CPG_PROCESS_TESTING
#include "process_test_hooks.h"
namespace cpg::testing { thread_local ProcessHooks *processHooks = nullptr; }
#endif

namespace cpg {
namespace {

bool iequalsWide(std::wstring_view left, std::wstring_view right)
{
    if (left.size() != right.size()) return false;
    return CompareStringOrdinal(left.data(), static_cast<int>(left.size()),
                                right.data(), static_cast<int>(right.size()), TRUE) == CSTR_EQUAL;
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
        // Hidden drive-state entries (=C:=...) are deliberately excluded;
        // their first '=' is never an empty environment variable name.
        if (equal != std::wstring_view::npos && equal != 0) {
            const std::wstring key(text.substr(0, equal));
            const std::wstring value(text.substr(equal + 1));
            environment.set(key, value);
        }
        entry += text.size() + 1;
    }
    return environment;
}

void UnicodeEnvironment::set(std::wstring key, std::wstring value)
{
    if (key.empty() || key.find_first_of(L"=\0", 0, 2) != std::wstring::npos
        || value.find(L'\0') != std::wstring::npos)
        throw Error("ENVIRONMENT_INVALID", "Invalid environment entry");
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
    if (std::erase_if(entries_, [&](const auto &entry) { return iequalsWide(entry.first, key); }) != 0)
        block_.clear();
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
    using Clock = std::chrono::steady_clock;
    const auto started = Clock::now();
    cancel.check();
    if (request.timeout <= std::chrono::milliseconds::zero()
        || request.stdoutLimit == 0 || request.stderrLimit == 0
        || request.stdinBytes.size() > helperRequestLimit)
        throw Error("HELPER_BUDGET_INVALID", "Invalid helper input or budget");
    const auto deadline = started + request.timeout;
#ifdef CPG_PROCESS_TESTING
    testing::ProcessHooks *hooks = testing::processHooks;
#endif

    // Allocate pipes, attributes, buffers and thread owners before spawning.
    // Only the three duplicated child endpoints are inheritable.
    struct Pipe { UniqueHandle read; UniqueHandle write; };
    SECURITY_ATTRIBUTES noInherit{sizeof(SECURITY_ATTRIBUTES), nullptr, FALSE};
    const auto createPipe = [&] {
        Pipe pipe;
        HANDLE read = nullptr, write = nullptr;
        if (!CreatePipe(&read, &write, &noInherit, 0))
            throw Error("HELPER_IO_FAILED", "Cannot create the helper pipes.");
        pipe.read.reset(read);
        pipe.write.reset(write);
        return pipe;
    };
    const auto inheritableDuplicate = [](HANDLE source) {
        HANDLE duplicate = nullptr;
        if (!DuplicateHandle(GetCurrentProcess(), source, GetCurrentProcess(), &duplicate, 0,
                             TRUE, DUPLICATE_SAME_ACCESS))
            throw Error("HELPER_IO_FAILED", "Cannot prepare the helper pipe ends.");
        return UniqueHandle(duplicate);
    };
    Pipe stdinPipe = createPipe();
    Pipe stdoutPipe = createPipe();
    Pipe stderrPipe = createPipe();
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
    SIZE_T attributeSize = 0;
    InitializeProcThreadAttributeList(nullptr, 1, 0, &attributeSize);
    if (GetLastError() != ERROR_INSUFFICIENT_BUFFER || attributeSize == 0)
        throw Error("HELPER_START_FAILED", "Cannot size the helper startup attributes");
    std::vector<BYTE> attributeStorage(attributeSize, 0);
    auto *attributes = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(attributeStorage.data());
    if (!InitializeProcThreadAttributeList(attributes, 1, 0, &attributeSize))
        throw Error("HELPER_START_FAILED", "Cannot prepare the helper startup attributes");
    HANDLE inheritedHandles[3] = {childStdin.get(), childStdout.get(), childStderr.get()};
    struct AttributeListScope {
        LPPROC_THREAD_ATTRIBUTE_LIST value;
        ~AttributeListScope() { DeleteProcThreadAttributeList(value); }
    } attributeScope{attributes};
    if (!UpdateProcThreadAttribute(attributes, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                                   inheritedHandles, sizeof(inheritedHandles), nullptr, nullptr))
        throw Error("HELPER_START_FAILED", "Cannot restrict helper handle inheritance");
    STARTUPINFOEXW startup{};
    startup.StartupInfo.cb = sizeof(startup);
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = childStdin.get();
    startup.StartupInfo.hStdOutput = childStdout.get();
    startup.StartupInfo.hStdError = childStderr.get();
    startup.lpAttributeList = attributes;
    struct Reader {
        std::string bytes;
        std::atomic_bool overLimit{false};
        std::atomic<DWORD> error{ERROR_SUCCESS};
        bool eof = false; // only read after join
    } stdoutReader, stderrReader;
    stdoutReader.bytes.reserve(request.stdoutLimit);
    stderrReader.bytes.reserve(request.stderrLimit);
    std::atomic_bool stopping{false};
    std::atomic<DWORD> writeError{ERROR_SUCCESS};
    std::atomic_bool submitted{false};
    std::jthread stdoutThread, stderrThread, writerThread;
    PROCESS_INFORMATION info{};
    const DWORD flags = CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT
        | (request.createNoWindow ? CREATE_NO_WINDOW : 0);
#ifdef CPG_PROCESS_TESTING
    if (hooks && hooks->beforeCreate) hooks->beforeCreate();
#endif
    cancel.check();
    if (Clock::now() >= deadline)
        throw Error("HELPER_TIMEOUT", "Owned helper exceeded its setup budget");
    if (!CreateProcessW(program.c_str(), commandLine.data(), nullptr, nullptr, TRUE, flags,
                        const_cast<wchar_t *>(environmentBlock.data()), nullptr,
                        &startup.StartupInfo, &info))
        throw Error("HELPER_START_FAILED", "Cannot start owned helper");

    // Installed immediately after spawn, without an intervening allocation.
    // Every later exception retains ownership until child and threads settle.
    struct OwnedScope {
        UniqueHandle process;
        UniqueHandle primaryThread;
        std::atomic_bool &stopping;
        std::array<std::jthread *, 3> threads;
#ifdef CPG_PROCESS_TESTING
        testing::ProcessHooks *hooks;
#endif
        bool settled = false;
        bool cleanupFailed = false;
        bool terminationRequested = false;
        const char *cleanupStage = "none";
        DWORD cleanupError = ERROR_SUCCESS;
        const char *cleanupAction = "none";
        DWORD cleanupActionError = ERROR_SUCCESS;
        std::chrono::milliseconds cleanupElapsed{0};

        void recordFailure(const char *stage, DWORD error) noexcept
        {
            if (!cleanupFailed) { cleanupStage = stage; cleanupError = error; }
            cleanupFailed = true;
        }

        void recordActionError(const char *stage, DWORD error) noexcept
        {
            if (cleanupActionError == ERROR_SUCCESS) {
                cleanupAction = stage;
                cleanupActionError = error;
            }
        }

        void settle(bool abort) noexcept
        {
            if (settled) return;
            if (abort) stopping.store(true);
            const auto cleanupStarted = Clock::now();
            const auto cleanupDeadline = cleanupStarted
#ifdef CPG_PROCESS_TESTING
                + (hooks ? hooks->cleanupBudget : std::chrono::milliseconds(3000));
#else
                + std::chrono::seconds(3);
#endif
            bool terminateAttempted = false;
            for (;;) {
                const DWORD processWait =
#ifdef CPG_PROCESS_TESTING
                    hooks && hooks->cleanupProcessWait ? hooks->cleanupProcessWait(process.get()) :
#endif
                    WaitForSingleObject(process.get(), 0);
                bool complete = processWait == WAIT_OBJECT_0;
                if (processWait == WAIT_FAILED) recordFailure("process_wait", GetLastError());
                if (abort && !complete && !terminateAttempted) {
                    terminateAttempted = true;
                    terminationRequested = true;
                    const BOOL terminated =
#ifdef CPG_PROCESS_TESTING
                        hooks && hooks->terminate ? hooks->terminate(process.get(), EXIT_FAILURE) :
#endif
                        TerminateProcess(process.get(), EXIT_FAILURE);
                    if (!terminated) {
                        // The child can already be exiting while its handle
                        // is not signaled yet. Completion below, not this
                        // racing request's result, decides whether it settled.
                        recordActionError("terminate", GetLastError());
                    }
                }
                for (std::jthread *thread : threads) {
                    if (!thread->joinable()) continue;
                    const DWORD wait = WaitForSingleObject(thread->native_handle(), 0);
                    if (wait == WAIT_OBJECT_0) continue;
                    complete = false;
                    if (wait == WAIT_FAILED) recordFailure("thread_wait", GetLastError());
                    // A cancellation request is not completion. Repeat it to
                    // cover a thread entering I/O just after its stop check.
                    const BOOL cancelled = !abort ? TRUE :
#ifdef CPG_PROCESS_TESTING
                        hooks && hooks->cancelIo ? hooks->cancelIo(thread->native_handle()) :
#endif
                        CancelSynchronousIo(thread->native_handle());
                    if (!cancelled) {
                        const DWORD error = GetLastError();
                        if (error != ERROR_NOT_FOUND) recordActionError("cancel_io", error);
                    }
                }
                if (Clock::now() >= cleanupDeadline) {
                    recordFailure("settle_budget", ERROR_TIMEOUT);
                    stopping.store(true);
                    abort = true;
                    // An unresponsive kernel/driver cannot be made hard-real-
                    // time. Retain ownership and caller busy until it settles;
                    // never detach or manufacture an idle/success result.
                }
                if (complete) break;
                Sleep(5);
            }
            for (std::jthread *thread : threads)
                if (thread->joinable()) thread->join();
            cleanupElapsed = std::chrono::duration_cast<std::chrono::milliseconds>(Clock::now() - cleanupStarted);
            settled = true;
        }
        ~OwnedScope() { settle(true); }
    } owned{UniqueHandle(info.hProcess), UniqueHandle(info.hThread), stopping,
            {&stdoutThread, &stderrThread, &writerThread}
#ifdef CPG_PROCESS_TESTING
            , hooks
#endif
    };

    enum class Failure { None, Setup, Io, Output, Cancelled, Timeout, Drain };
    Failure failure = Failure::None;
    DWORD exitCode = 0;
    bool processExited = false;
    bool beforeJoinCalled = false;
    const auto beforeJoin = [&] {
        if (beforeJoinCalled) return;
        beforeJoinCalled = true;
#ifdef CPG_PROCESS_TESTING
        if (hooks && hooks->beforeJoin) hooks->beforeJoin();
#endif
    };
    const auto readerLoop = [&](HANDLE pipe, std::size_t limit, Reader &reader, bool isStdout) {
#ifndef CPG_PROCESS_TESTING
        (void)isStdout;
#endif
        try {
            std::array<char, 4096> buffer{};
            while (!stopping.load()) {
                DWORD got = 0;
                if (!ReadFile(pipe, buffer.data(), static_cast<DWORD>(buffer.size()), &got, nullptr)) {
                    const DWORD error = GetLastError();
                    if (error == ERROR_BROKEN_PIPE || error == ERROR_NO_DATA) reader.eof = true;
                    else if (!(error == ERROR_OPERATION_ABORTED && stopping.load()))
                        reader.error.store(error);
                    return;
                }
                if (got == 0) { reader.eof = true; return; }
#ifdef CPG_PROCESS_TESTING
                DWORD injectedError = ERROR_SUCCESS;
                if (hooks && hooks->afterRead)
                    hooks->afterRead(isStdout, owned.process.get(), injectedError);
                if (injectedError != ERROR_SUCCESS) { reader.error.store(injectedError); return; }
#endif
                if (got > limit - reader.bytes.size()) { reader.overLimit.store(true); return; }
                reader.bytes.append(buffer.data(), got);
            }
        } catch (...) { reader.error.store(ERROR_NOT_ENOUGH_MEMORY); }
    };
    const auto writerLoop = [&] {
        try {
            std::size_t written = 0;
            while (written < request.stdinBytes.size() && !stopping.load() && !cancel.cancelled()) {
                const DWORD chunk = static_cast<DWORD>(std::min<std::size_t>(
                    4096, request.stdinBytes.size() - written));
                DWORD count = 0;
#ifdef CPG_PROCESS_TESTING
                const BOOL ok = hooks && hooks->write
                    ? hooks->write(stdinPipe.write.get(), request.stdinBytes.data() + written,
                                   chunk, &count)
                    : WriteFile(stdinPipe.write.get(), request.stdinBytes.data() + written,
                                chunk, &count, nullptr);
#else
                const BOOL ok = WriteFile(stdinPipe.write.get(), request.stdinBytes.data() + written,
                                         chunk, &count, nullptr);
#endif
                const DWORD error = ok ? ERROR_SUCCESS : GetLastError();
#ifdef CPG_PROCESS_TESTING
                if (hooks && hooks->afterWrite) hooks->afterWrite();
#endif
                if (!ok) {
                    if (!stopping.load()) writeError.store(error ? error : ERROR_WRITE_FAULT);
                    break;
                }
                if (count == 0 || count > chunk) { writeError.store(ERROR_WRITE_FAULT); break; }
                written += count;
            }
            // A successful final write remains submitted even if cancellation
            // raced with its completion or the owner has started settling.
            if (written == request.stdinBytes.size()) submitted.store(true);
        } catch (...) { writeError.store(ERROR_WRITE_FAULT); }
        stdinPipe.write.reset();
    };

    try {
#ifdef CPG_PROCESS_TESTING
        if (hooks && hooks->afterCreate) hooks->afterCreate(owned.process.get());
#endif
        childStdin.reset(); childStdout.reset(); childStderr.reset();
        stdinPipe.read.reset(); stdoutPipe.write.reset(); stderrPipe.write.reset();
#ifdef CPG_PROCESS_TESTING
        if (hooks && hooks->beforeThread) hooks->beforeThread(0);
#endif
        stdoutThread = std::jthread(readerLoop, stdoutPipe.read.get(), request.stdoutLimit,
                                   std::ref(stdoutReader), true);
#ifdef CPG_PROCESS_TESTING
        if (hooks && hooks->beforeThread) hooks->beforeThread(1);
#endif
        stderrThread = std::jthread(readerLoop, stderrPipe.read.get(), request.stderrLimit,
                                   std::ref(stderrReader), false);
        cancel.check(); // actual submission boundary, after reader setup
        if (Clock::now() >= deadline)
            throw Error("HELPER_TIMEOUT", "Owned helper exceeded its setup budget");
        if (!request.stdinBytes.empty()) {
#ifdef CPG_PROCESS_TESTING
            if (hooks && hooks->beforeThread) hooks->beforeThread(2);
#endif
            writerThread = std::jthread(writerLoop);
        } else stdinPipe.write.reset();

        auto drainDeadline = deadline;
        for (;;) {
            if (!processExited) {
                const DWORD wait = WaitForSingleObject(owned.process.get(), 0);
                if (wait == WAIT_FAILED) { failure = Failure::Io; break; }
                if (wait == WAIT_OBJECT_0) {
                    processExited = true;
                    if (!GetExitCodeProcess(owned.process.get(), &exitCode)) {
                        failure = Failure::Io;
                        break;
                    }
                    drainDeadline = Clock::now() + std::chrono::milliseconds(500);
                    beforeJoin();
                }
            }
            std::array<HANDLE, 5> waits{};
            DWORD waitCount = 0;
            if (!processExited) waits[waitCount++] = owned.process.get();
            for (std::jthread *thread : owned.threads) {
                if (!thread->joinable()) continue;
                const DWORD wait = WaitForSingleObject(thread->native_handle(), 0);
                if (wait == WAIT_FAILED) { failure = Failure::Io; break; }
                if (wait != WAIT_OBJECT_0) waits[waitCount++] = thread->native_handle();
            }
            if (failure != Failure::None || waitCount == 0) break;
            if (stdoutReader.overLimit.load() || stderrReader.overLimit.load()) {
                failure = Failure::Output; break;
            }
            if (stdoutReader.error.load() || stderrReader.error.load() || writeError.load()) {
                failure = Failure::Io; break;
            }
            if (cancel.cancelled()) { failure = Failure::Cancelled; break; }
            if (Clock::now() >= deadline) { failure = Failure::Timeout; break; }
            if (processExited && Clock::now() >= drainDeadline) { failure = Failure::Drain; break; }
            if (cancel.eventHandle()) waits[waitCount++] = cancel.eventHandle();
            if (WaitForMultipleObjects(waitCount, waits.data(), FALSE, 10) == WAIT_FAILED) {
                failure = Failure::Io; break;
            }
        }
    } catch (const Error &error) {
        failure = error.code == "LAUNCH_CANCELLED" ? Failure::Cancelled
            : error.code == "HELPER_TIMEOUT" ? Failure::Timeout : Failure::Setup;
    } catch (...) { failure = Failure::Setup; }

    // Also close retained endpoints if a post-spawn setup step threw.
    childStdin.reset(); childStdout.reset(); childStderr.reset();
    stdinPipe.read.reset(); stdoutPipe.write.reset(); stderrPipe.write.reset();
    try { beforeJoin(); } catch (...) { failure = Failure::Setup; }
    owned.settle(failure != Failure::None);
    stdinPipe.write.reset(); stdoutPipe.read.reset(); stderrPipe.read.reset();
#ifdef CPG_PROCESS_TESTING
    if (hooks && hooks->beforeVerdict) hooks->beforeVerdict();
#endif

    // The unique verdict is made only after all child and I/O facts settle.
    ProcessResult result;
    result.submitted = submitted.load();
    if (!GetExitCodeProcess(owned.process.get(), &exitCode)) failure = Failure::Io;
    result.normalExit = exitCode < 0x80000000;
    result.exitCode = result.normalExit ? static_cast<int>(exitCode) : -1;
    const auto fail = [&](const char *code, const char *message) -> void {
        if (request.activationSubmission && result.submitted)
            throw Error("APPX_ACTIVATION_OUTCOME_UNKNOWN",
                        std::string("Activation request was submitted; refresh Desktop state before retrying. ")
                            + message);
        throw Error(code, message);
    };
    if (owned.cleanupFailed) {
        const std::string message = std::string("Owned helper cleanup failed: stage=")
            + owned.cleanupStage + "; native_error=" + std::to_string(owned.cleanupError)
            + "; action=" + owned.cleanupAction
            + "; action_error=" + std::to_string(owned.cleanupActionError)
            + "; elapsed_ms=" + std::to_string(owned.cleanupElapsed.count());
        fail("HELPER_CLEANUP_FAILED", message.c_str());
    }
    if (stdoutReader.overLimit.load() || stderrReader.overLimit.load())
        fail("HELPER_OUTPUT_LIMIT", "Owned helper exceeded its output budget");
    if (writeError.load() || stdoutReader.error.load() || stderrReader.error.load())
        fail("HELPER_IO_FAILED", "Cannot complete the owned helper I/O");
    if (failure == Failure::None && !request.stdinBytes.empty() && !result.submitted)
        failure = cancel.cancelled() ? Failure::Cancelled : Failure::Io;
    // Fully collected natural completion remains known if cancellation raced
    // with it. The caller still validates the returned completion receipt.
    if ((failure == Failure::Cancelled || failure == Failure::Timeout)
        && !owned.terminationRequested && stdoutReader.eof && stderrReader.eof
        && (request.stdinBytes.empty() || result.submitted)) failure = Failure::None;
    switch (failure) {
    case Failure::None: break;
    case Failure::Setup: fail("HELPER_IO_FAILED", "Cannot prepare the owned helper I/O"); break;
    case Failure::Io: fail("HELPER_IO_FAILED", "Cannot observe the owned helper completion"); break;
    case Failure::Output: fail("HELPER_OUTPUT_LIMIT", "Owned helper exceeded its output budget"); break;
    case Failure::Cancelled: fail("LAUNCH_CANCELLED", "Operation cancelled"); break;
    case Failure::Timeout: fail("HELPER_TIMEOUT", "Owned helper exceeded its time budget"); break;
    case Failure::Drain: fail("HELPER_DRAIN_TIMEOUT", "Owned helper output did not close within its drain budget"); break;
    }
    result.out = std::move(stdoutReader.bytes);
    result.err = std::move(stderrReader.bytes);
    return result;
}

} // namespace cpg
