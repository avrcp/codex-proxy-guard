#pragma once
#include "input.h"
#include <optional>
#include <string>
#include <string_view>

namespace cpg { class ConsoleControlScope; }
namespace cpg::tui {
namespace detail {
enum class PlainReadStage { Ready, Changed, Cancelled };
// Optional internal observer of completed reader transitions. It does not
// supply input or alter cancellation; the production entry leaves it null.
struct PlainReadObserver {
    virtual ~PlainReadObserver() = default;
    virtual void progressed(PlainReadStage stage, std::size_t units) = 0;
};
}
enum class PlainLineAction { None, Changed, Submitted, EndOfInput, Cancelled };
// The bounded UTF-16 editor shared by the real console reader and tests.
class PlainLineEditor {
public:
    PlainLineAction handle(const Key &key);
    const std::wstring &value() const { return value_; }
    void clear() { value_.clear(); pendingHigh_ = 0; }
private:
    void append(wchar_t character);
    std::wstring value_;
    wchar_t pendingHigh_ = 0;
};
struct PlainInputEvent {
    enum class Kind { Key, Cancel, Close };
    Kind kind = Kind::Key;
    cpg::tui::Key key;
};
// The reader owns input mode only while reading a line. next() must wake on
// console-control events as well as input; it never touches the engine.
struct PlainConsoleInput {
    virtual ~PlainConsoleInput() = default;
    virtual bool getMode(DWORD &mode) = 0;
    virtual bool setMode(DWORD mode) = 0;
    virtual PlainInputEvent next() = 0;
    virtual void echo(std::wstring_view previous, std::wstring_view current) = 0;
    virtual void newline() = 0;
};
std::optional<std::wstring> readPlainConsoleLine(PlainConsoleInput &input, detail::PlainReadObserver *observer = nullptr);
std::optional<std::wstring> readNativePlainConsoleLine(ConsoleControlScope &controls, detail::PlainReadObserver *observer = nullptr);
namespace detail {
std::optional<std::string> readRedirectedPlainLine(HANDLE input, ConsoleControlScope &controls, PlainReadObserver *observer = nullptr);
}
} // namespace cpg::tui
