#pragma once

#include <atomic>
#include <memory>

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

namespace cpg {

struct Error;

// Cooperative cancellation shared across threads. The manual-reset Win32
// event lets cancellation join WaitForMultipleObjects() waits (console input,
// child processes) instead of relying on polling alone; the atomic flag keeps
// cheap checks lock-free. Copying shares the state (mirrors the previous
// shared-flag semantics).
class Cancellation {
public:
    Cancellation();

    void cancel() const noexcept;
    bool cancelled() const noexcept;
    // Throws Error("LAUNCH_CANCELLED", "Operation cancelled.") when cancelled.
    void check() const;
    // Win32 event handle (signalled after cancel()); null before
    // initialization or when event creation failed — callers waiting on it
    // must treat null as "poll the flag".
    HANDLE eventHandle() const noexcept;

private:
    struct State;
    std::shared_ptr<State> state_;
};

} // namespace cpg
