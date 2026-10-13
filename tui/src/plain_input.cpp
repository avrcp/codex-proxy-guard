#include "plain_input.h"
#include "cpg/error.h"
#include "cpg/console_control.h"

namespace cpg::tui {
namespace {
constexpr std::size_t maximumLineUnits = 4096;
bool high(wchar_t character) { return character >= 0xd800 && character <= 0xdbff; }
bool low(wchar_t character) { return character >= 0xdc00 && character <= 0xdfff; }
class InputModeScope {
public:
    explicit InputModeScope(PlainConsoleInput &input) : input_(input) {
        if (!input_.getMode(original_)) throw Error("CONSOLE_INPUT_FAILED", "Cannot read console input mode.");
        const DWORD desired = (original_ | ENABLE_PROCESSED_INPUT | ENABLE_EXTENDED_FLAGS)
            & ~static_cast<DWORD>(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_QUICK_EDIT_MODE);
        if (desired != original_) {
            if (!input_.setMode(desired)) throw Error("CONSOLE_INPUT_FAILED", "Cannot enable console key input.");
            changed_ = true;
        }
    }
    ~InputModeScope() { if (changed_) input_.setMode(original_); }
private:
    PlainConsoleInput &input_;
    DWORD original_ = 0;
    bool changed_ = false;
};
}
void PlainLineEditor::append(wchar_t character)
{
    if (value_.size() >= maximumLineUnits) throw Error("INPUT_INVALID", "Input exceeds its limit.");
    value_.push_back(character);
}
PlainLineAction PlainLineEditor::handle(const Key &key)
{
    if (key.control && (key.unicode == 0x03 || key.virtualKey == VK_CANCEL)) {
        clear(); return PlainLineAction::Cancelled;
    }
    if ((key.control && key.virtualKey == 'Z') || key.unicode == 0x1a) {
        if (value_.empty() && !pendingHigh_) return PlainLineAction::EndOfInput;
        return PlainLineAction::None;
    }
    if (key.virtualKey == VK_RETURN || key.unicode == L'\r' || key.unicode == L'\n') {
        if (pendingHigh_) { append(0xfffd); pendingHigh_ = 0; }
        return PlainLineAction::Submitted;
    }
    if (key.virtualKey == VK_BACK) {
        if (pendingHigh_) { pendingHigh_ = 0; return PlainLineAction::None; }
        if (value_.empty()) return PlainLineAction::None;
        const bool pair = value_.size() > 1 && low(value_.back()) && high(value_[value_.size() - 2]);
        value_.resize(value_.size() - (pair ? 2 : 1));
        return PlainLineAction::Changed;
    }
    if (key.unicode < 0x20 || key.unicode == 0x7f) return PlainLineAction::None;
    bool replacedPending = false;
    if (pendingHigh_) {
        if (low(key.unicode)) {
            if (value_.size() > maximumLineUnits - 2) throw Error("INPUT_INVALID", "Input exceeds its limit.");
            append(pendingHigh_); append(key.unicode); pendingHigh_ = 0;
            return PlainLineAction::Changed;
        }
        append(0xfffd); pendingHigh_ = 0; replacedPending = true;
    }
    if (high(key.unicode)) { pendingHigh_ = key.unicode; return replacedPending ? PlainLineAction::Changed : PlainLineAction::None; }
    append(low(key.unicode) ? static_cast<wchar_t>(0xfffd) : key.unicode);
    return PlainLineAction::Changed;
}
std::optional<std::wstring> readPlainConsoleLine(PlainConsoleInput &input)
{
    InputModeScope modes(input);
    PlainLineEditor editor;
    for (;;) {
        const auto event = input.next();
        if (event.kind == PlainInputEvent::Kind::Close) return {};
        const auto previous = editor.value();
        if (event.kind == PlainInputEvent::Kind::Cancel) {
            editor.clear(); input.echo(previous, editor.value()); continue;
        }
        const auto action = editor.handle(event.key);
        if (action == PlainLineAction::EndOfInput) { input.newline(); return {}; }
        if (action == PlainLineAction::Submitted) {
            input.echo(previous, editor.value()); input.newline(); return editor.value();
        }
        if (action == PlainLineAction::Changed || action == PlainLineAction::Cancelled)
            input.echo(previous, editor.value());
    }
}
namespace detail {
std::optional<std::string> readRedirectedPlainLine(HANDLE handle, ConsoleControlScope &controls)
{
    const DWORD type = GetFileType(handle);
    // Actual console input uses the event reader. Other character devices,
    // including NUL and serial ports, are unsupported: their synchronous reads
    // need not be bounded or wake when a console control event arrives.
    if (type != FILE_TYPE_PIPE && type != FILE_TYPE_DISK)
        throw Error("CONSOLE_INPUT_FAILED", "Redirected standard input must be a pipe or disk file.");
    std::string value;
    for (;;) {
        if (controls.takeClose()) return {};
        if (controls.takeCancel()) { value.clear(); continue; }
        if (type == FILE_TYPE_PIPE) {
            DWORD available = 0;
            if (!PeekNamedPipe(handle, nullptr, 0, nullptr, &available, nullptr)) {
                const DWORD error = GetLastError();
                if (error == ERROR_BROKEN_PIPE || error == ERROR_PIPE_NOT_CONNECTED)
                    return value.empty() ? std::nullopt : std::optional<std::string>(value);
                throw Error("CONSOLE_INPUT_FAILED", "Cannot inspect redirected input.");
            }
            if (available == 0) {
                const DWORD wait = WaitForSingleObject(controls.signalEvent(), 20);
                if (wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT)
                    throw Error("CONSOLE_INPUT_FAILED", "Redirected input wait failed.");
                continue;
            }
        }
        char character = 0; DWORD count = 0;
        if (!ReadFile(handle, &character, 1, &count, nullptr)) {
            if (GetLastError() != ERROR_BROKEN_PIPE)
                throw Error("CONSOLE_INPUT_FAILED", "Cannot read redirected input.");
            count = 0;
        }
        if (count == 0) return value.empty() ? std::nullopt : std::optional<std::string>(value);
        if (character == '\n') return value;
        if (value.size() >= 4096) throw Error("INPUT_INVALID", "Input exceeds its limit.");
        value.push_back(character);
    }
}
} // namespace detail
} // namespace cpg::tui
