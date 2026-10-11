#include "terminal_session.h"

namespace cpg::tui {

bool TerminalSession::isInteractiveConsole()
{
    const HANDLE input = GetStdHandle(STD_INPUT_HANDLE);
    const HANDLE output = GetStdHandle(STD_OUTPUT_HANDLE);
    return input != nullptr && input != INVALID_HANDLE_VALUE
        && output != nullptr && output != INVALID_HANDLE_VALUE
        && GetFileType(input) == FILE_TYPE_CHAR
        && GetFileType(output) == FILE_TYPE_CHAR;
}

bool TerminalSession::enable()
{
    input_ = GetStdHandle(STD_INPUT_HANDLE);
    output_ = GetStdHandle(STD_OUTPUT_HANDLE);
    if (input_ == nullptr || input_ == INVALID_HANDLE_VALUE
        || output_ == nullptr || output_ == INVALID_HANDLE_VALUE)
        return false;
    if (!GetConsoleMode(input_, &inputMode_) || !GetConsoleMode(output_, &outputMode_))
        return false;
    DWORD titleLength = GetConsoleTitleW(nullptr, 0);
    if (titleLength > 0 && titleLength < 8192) {
        title_.resize(titleLength);
        const DWORD written = GetConsoleTitleW(title_.data(), titleLength);
        title_.resize(written);
    }
    DWORD outputMode = outputMode_ | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
    if (!SetConsoleMode(output_, outputMode)) return false;
    DWORD inputMode = inputMode_ | ENABLE_WINDOW_INPUT;
    // Line/echo input must be off; processed input stays on so Ctrl+C reaches
    // the control handler while the TUI also reads it as a key event.
    inputMode &= ~static_cast<DWORD>(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);
    if (!SetConsoleMode(input_, inputMode)) {
        SetConsoleMode(output_, outputMode_);
        return false;
    }
    // Hide the hardware cursor while the renderer owns the screen.
    DWORD written = 0;
    WriteConsoleW(output_, L"\x1b[?25l", 6, &written, nullptr);
    restored_ = false;
    return true;
}

TerminalSession::~TerminalSession()
{
    if (input_ == nullptr || output_ == nullptr) return;
    DWORD written = 0;
    WriteConsoleW(output_, L"\x1b[?25h", 6, &written, nullptr);
    leaveAlternateScreen();
    SetConsoleMode(input_, inputMode_);
    SetConsoleMode(output_, outputMode_);
    if (!title_.empty()) SetConsoleTitleW(title_.c_str());
    // Park the cursor at the last line so the shell prompt lands cleanly.
    CONSOLE_SCREEN_BUFFER_INFO info{};
    if (GetConsoleScreenBufferInfo(output_, &info)) {
        const COORD home{0, info.dwCursorPosition.Y < info.dwSize.Y - 1
                             ? info.dwCursorPosition.Y : info.dwSize.Y - 1};
        SetConsoleCursorPosition(output_, home);
    }
}

bool TerminalSession::enterAlternateScreen()
{
    if (alternate_) return true;
    DWORD written = 0;
    if (!WriteConsoleW(output_, L"\x1b[?1049h", 8, &written, nullptr) || written != 8)
        return false;
    alternate_ = true;
    return true;
}

void TerminalSession::leaveAlternateScreen()
{
    if (!alternate_) return;
    DWORD written = 0;
    WriteConsoleW(output_, L"\x1b[?1049l", 8, &written, nullptr);
    alternate_ = false;
}

} // namespace cpg::tui
