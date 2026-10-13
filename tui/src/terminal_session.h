#pragma once
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <string>
#include <string_view>

namespace cpg::tui {
// Narrow Win32 seam lets acquisition failures and restoration be verified.
struct TerminalApi {
    virtual ~TerminalApi() = default;
    virtual HANDLE standardHandle(DWORD id) = 0;
    virtual bool getMode(HANDLE handle, DWORD &mode) = 0;
    virtual bool setMode(HANDLE handle, DWORD mode) = 0;
    virtual bool getCursor(HANDLE handle, CONSOLE_CURSOR_INFO &cursor) = 0;
    virtual bool setCursor(HANDLE handle, const CONSOLE_CURSOR_INFO &cursor) = 0;
    virtual bool write(HANDLE handle, std::wstring_view text) = 0;
};
class TerminalSession {
public:
    TerminalSession();
    explicit TerminalSession(TerminalApi &api) : api_(api) {}
    static bool isInteractiveConsole();
    bool enable();
    ~TerminalSession();
    TerminalSession(const TerminalSession &) = delete;
    TerminalSession &operator=(const TerminalSession &) = delete;
    bool enterAlternateScreen();
    void leaveAlternateScreen();
    void restore() noexcept;
    bool alternateActive() const { return alternate_; }
    HANDLE input() const { return input_; }
    HANDLE output() const { return output_; }
private:
    TerminalApi &api_;
    HANDLE input_ = nullptr, output_ = nullptr;
    DWORD inputMode_ = 0, outputMode_ = 0;
    CONSOLE_CURSOR_INFO cursor_{};
    bool inputChanged_ = false, outputChanged_ = false, cursorChanged_ = false;
    bool alternate_ = false, enabled_ = false;
};
} // namespace cpg::tui
