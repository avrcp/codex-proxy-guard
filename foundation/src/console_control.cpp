#include "cpg/console_control.h"
#include "cpg/error.h"
#include <atomic>

namespace {
constexpr unsigned cancelBit = 1, closeBit = 2;
constexpr DWORD closeBudgetMs = 3500;
std::atomic_bool claimed{false};
std::atomic_bool active{false};
std::atomic_uint pending{0};
std::atomic_uint callbacks{0};

// Deliberately retained until process termination. An OS control callback may
// already be scheduled when SetConsoleCtrlHandler unregisters the function;
// it must never see a closed or reused kernel handle during owner teardown.
HANDLE signalHandle = nullptr;
HANDLE completionHandle = nullptr;

BOOL WINAPI controlHandler(DWORD event)
{
    callbacks.fetch_add(1, std::memory_order_acq_rel);
    struct CallbackScope {
        ~CallbackScope() { callbacks.fetch_sub(1, std::memory_order_release); }
    } callbackScope;
    if (!active.load(std::memory_order_acquire)) return FALSE;
    unsigned bits = 0;
    switch (event) {
    case CTRL_C_EVENT:
    case CTRL_BREAK_EVENT:
        bits = cancelBit;
        break;
    case CTRL_CLOSE_EVENT:
    case CTRL_LOGOFF_EVENT:
    case CTRL_SHUTDOWN_EVENT:
        bits = cancelBit | closeBit;
        break;
    default: return FALSE;
    }
    pending.fetch_or(bits, std::memory_order_release);
    SetEvent(signalHandle);
    // Returning TRUE for CLOSE terminates the process. Keep this callback
    // alive while the owner cancels and settles its foreground operation.
    // This thread performs no engine work, allocation, file or console I/O.
    if ((bits & closeBit) != 0)
        WaitForSingleObject(completionHandle, closeBudgetMs);
    return TRUE;
}

bool take(unsigned bit) noexcept
{
    const bool requested = (pending.fetch_and(~bit, std::memory_order_acq_rel) & bit) != 0;
    // Reset first, then recheck. A producer before the reset is visible in
    // pending; a producer after the recheck sets the event itself. Thus taking
    // Cancel cannot swallow Close or lose a signal arriving during consumption.
    ResetEvent(signalHandle);
    if (pending.load(std::memory_order_acquire) != 0) SetEvent(signalHandle);
    return requested;
}
}

namespace cpg {
ConsoleControlScope::ConsoleControlScope()
{
    bool expected = false;
    if (!claimed.compare_exchange_strong(expected, true, std::memory_order_acq_rel))
        throw Error("CONSOLE_CONTROL_FAILED", "A console control scope is already active.");
    try {
        // A prior scope has already signalled completion before relinquishing
        // its claim. Do not reset that acknowledgement under a live callback.
        const auto until = GetTickCount64() + closeBudgetMs;
        while (callbacks.load(std::memory_order_acquire) != 0) {
            if (GetTickCount64() >= until)
                throw Error("CONSOLE_CONTROL_FAILED", "A previous console callback is still settling.");
            Sleep(1);
        }
        if (!signalHandle) signalHandle = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        if (!completionHandle) completionHandle = CreateEventW(nullptr, TRUE, TRUE, nullptr);
        if (!signalHandle || !completionHandle)
            throw Error("CONSOLE_CONTROL_FAILED", "Could not create console cancellation events.");
        pending.store(0, std::memory_order_release);
        if (!ResetEvent(signalHandle) || !ResetEvent(completionHandle))
            throw Error("CONSOLE_CONTROL_FAILED", "Could not prepare console cancellation events.");
        active.store(true, std::memory_order_release);
        if (!SetConsoleCtrlHandler(controlHandler, TRUE))
            throw Error("CONSOLE_CONTROL_FAILED", "Could not register console cancellation.");
    } catch (...) {
        active.store(false, std::memory_order_release);
        if (completionHandle) SetEvent(completionHandle);
        claimed.store(false, std::memory_order_release);
        throw;
    }
}
ConsoleControlScope::~ConsoleControlScope()
{
    active.store(false, std::memory_order_release);
    // The owner must have reaped its operation before reaching this scope.
    // Wake CLOSE callbacks before unregistering; callbacks never wait on owner
    // storage, and the kernel events remain valid after this object disappears.
    SetEvent(completionHandle);
    SetConsoleCtrlHandler(controlHandler, FALSE);
    claimed.store(false, std::memory_order_release);
}
bool ConsoleControlScope::takeCancel() noexcept { return take(cancelBit); }
bool ConsoleControlScope::takeClose() noexcept { return take(closeBit); }
HANDLE ConsoleControlScope::signalEvent() const noexcept { return signalHandle; }
}
