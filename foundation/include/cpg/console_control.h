#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

namespace cpg {
// One scope per process, owned by the foreground loop. It must outlive all
// engine/helper settlement: destruction acknowledges a console-close request.
// The callback uses only process-lifetime atomics/events, never a session.
class ConsoleControlScope {
public:
    ConsoleControlScope();
    ~ConsoleControlScope();
    ConsoleControlScope(const ConsoleControlScope &) = delete;
    ConsoleControlScope &operator=(const ConsoleControlScope &) = delete;
    bool takeCancel() noexcept;
    bool takeClose() noexcept;
    // Manual-reset wake signal. Consuming either request preserves the other
    // and any concurrent request; callers must not reset or close this handle.
    HANDLE signalEvent() const noexcept;
};
}
