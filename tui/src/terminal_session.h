#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <string>

namespace cpg::tui {

// RAII owner of the console state: input/output modes, virtual-terminal
// processing, window title and alternate-screen occupancy are restored on
// destruction even when the TUI exits through an exception.
class TerminalSession {
public:
    TerminalSession() = default;
    static bool isInteractiveConsole();
    // Saves the current modes, enables ENABLE_VIRTUAL_TERMINAL_PROCESSING on
    // the output and ENABLE_WINDOW_INPUT on the input. Returns false when VT
    // processing cannot be enabled (the caller falls back to plain console).
    bool enable();
    ~TerminalSession();

    TerminalSession(const TerminalSession &) = delete;
    TerminalSession &operator=(const TerminalSession &) = delete;

    bool enterAlternateScreen();
    void leaveAlternateScreen();
    bool alternateActive() const { return alternate_; }
    HANDLE input() const { return input_; }
    HANDLE output() const { return output_; }

private:
    HANDLE input_ = nullptr;
    HANDLE output_ = nullptr;
    DWORD inputMode_ = 0;
    DWORD outputMode_ = 0;
    bool restored_ = false;
    bool alternate_ = false;
    std::wstring title_;
};

} // namespace cpg::tui
