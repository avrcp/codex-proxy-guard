#include "terminal_session.h"
namespace cpg::tui {
namespace {
class NativeTerminalApi final : public TerminalApi {
public:
    HANDLE standardHandle(DWORD id) override { return GetStdHandle(id); }
    bool getMode(HANDLE handle, DWORD &mode) override { return GetConsoleMode(handle, &mode) != FALSE; }
    bool setMode(HANDLE handle, DWORD mode) override { return SetConsoleMode(handle, mode) != FALSE; }
    bool getCursor(HANDLE handle, CONSOLE_CURSOR_INFO &cursor) override { return GetConsoleCursorInfo(handle, &cursor) != FALSE; }
    bool setCursor(HANDLE handle, const CONSOLE_CURSOR_INFO &cursor) override { return SetConsoleCursorInfo(handle, &cursor) != FALSE; }
    bool write(HANDLE handle, std::wstring_view text) override {
        DWORD written = 0;
        return WriteConsoleW(handle, text.data(), static_cast<DWORD>(text.size()), &written, nullptr)
            && written == text.size();
    }
};
NativeTerminalApi nativeApi;
}
TerminalSession::TerminalSession() : api_(nativeApi) {}
bool TerminalSession::isInteractiveConsole()
{
    DWORD inputMode = 0, outputMode = 0;
    return GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &inputMode)
        && GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &outputMode);
}
bool TerminalSession::enable()
{
    if (enabled_) return true;
    input_ = api_.standardHandle(STD_INPUT_HANDLE);
    output_ = api_.standardHandle(STD_OUTPUT_HANDLE);
    if (!api_.getMode(input_, inputMode_) || !api_.getMode(output_, outputMode_)
        || !api_.getCursor(output_, cursor_)) return false;
    if (!api_.setMode(output_, outputMode_ | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING)) return false;
    outputChanged_ = true;
    const DWORD inputMode = (inputMode_ | ENABLE_WINDOW_INPUT | ENABLE_PROCESSED_INPUT | ENABLE_EXTENDED_FLAGS)
        & ~static_cast<DWORD>(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_QUICK_EDIT_MODE);
    if (!api_.setMode(input_, inputMode)) { restore(); return false; }
    inputChanged_ = true;
    CONSOLE_CURSOR_INFO hidden = cursor_;
    hidden.bVisible = FALSE;
    if (!api_.setCursor(output_, hidden)) { restore(); return false; }
    cursorChanged_ = true;
    enabled_ = true;
    return true;
}
TerminalSession::~TerminalSession() { restore(); }
void TerminalSession::restore() noexcept
{
    leaveAlternateScreen();
    if (cursorChanged_) { api_.setCursor(output_, cursor_); cursorChanged_ = false; }
    if (inputChanged_) { api_.setMode(input_, inputMode_); inputChanged_ = false; }
    if (outputChanged_) { api_.setMode(output_, outputMode_); outputChanged_ = false; }
    enabled_ = false;
}
bool TerminalSession::enterAlternateScreen()
{
    if (!enabled_) return false;
    if (alternate_) return true;
    // Even a partial write may have entered the alternate buffer.
    alternate_ = true;
    if (!api_.write(output_, L"\x1b[?1049h")) { restore(); return false; }
    return true;
}
void TerminalSession::leaveAlternateScreen()
{
    if (!alternate_) return;
    api_.write(output_, L"\x1b[?1049l");
    alternate_ = false;
}
} // namespace cpg::tui
