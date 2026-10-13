#include "cli_console.h"
#include "cpg/build_info.h"
#include "cpg/console_control.h"
#include "cpg/text.h"
#include "tui_controller.h"
#include "plain_input.h"
#include "display_width.h"
#include <cctype>
#include <cwctype>
#include <iostream>
#include <optional>

namespace cpg::tui {
namespace {
void print(std::ostream &output, const std::string &text)
{
    DWORD mode = 0;
    if (&output == &std::cout && GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &mode)) {
        const auto wide = utf8ToWide(text + "\n"); DWORD written = 0;
        if (!WriteConsoleW(GetStdHandle(STD_OUTPUT_HANDLE), wide.data(), static_cast<DWORD>(wide.size()), &written, nullptr)
            || written != wide.size()) throw Error("CONSOLE_OUTPUT_FAILED", "Cannot write console output.");
    } else {
        output << text << std::endl;
        if (!output) throw Error("CONSOLE_OUTPUT_FAILED", "Cannot write console output.");
    }
}
std::string trimmed(std::string_view value)
{
    std::size_t first = 0, last = value.size();
    while (first < last && std::isspace(static_cast<unsigned char>(value[first]))) ++first;
    while (last > first && std::isspace(static_cast<unsigned char>(value[last - 1]))) --last;
    return std::string(value.substr(first, last - first));
}
class NativePlainInput final : public PlainConsoleInput {
public:
    explicit NativePlainInput(ConsoleControlScope &controls) : controls_(controls) {
        DWORD mode = 0;
        echoEnabled_ = GetConsoleMode(output_, &mode) != FALSE;
    }
    bool getMode(DWORD &mode) override { return GetConsoleMode(input_, &mode) != FALSE; }
    bool setMode(DWORD mode) override { return SetConsoleMode(input_, mode) != FALSE; }
    PlainInputEvent next() override {
        const HANDLE handles[]{controls_.signalEvent(), input_};
        for (;;) {
            if (controls_.takeClose()) return {PlainInputEvent::Kind::Close, {}};
            if (controls_.takeCancel()) return {PlainInputEvent::Kind::Cancel, {}};
            if (repeats_ > 0) { --repeats_; return {PlainInputEvent::Kind::Key, repeated_}; }
            const DWORD wait = WaitForMultipleObjects(2, handles, FALSE, INFINITE);
            if (wait == WAIT_OBJECT_0) continue;
            if (wait != WAIT_OBJECT_0 + 1) throw Error("CONSOLE_INPUT_FAILED", "Console input wait failed.");
            INPUT_RECORD record{}; DWORD count = 0;
            // One record avoids consuming keys beyond Enter into the next prompt.
            if (!ReadConsoleInputW(input_, &record, 1, &count) || count != 1)
                throw Error("CONSOLE_INPUT_FAILED", "Console key input failed.");
            const auto batch = normalizeRecords(&record, 1);
            if (batch.keys.empty()) continue;
            repeated_ = batch.keys.front();
            repeats_ = record.Event.KeyEvent.wRepeatCount > 0
                ? static_cast<WORD>(record.Event.KeyEvent.wRepeatCount - 1) : 0;
            return {PlainInputEvent::Kind::Key, repeated_};
        }
    }
    void echo(std::wstring_view previous, std::wstring_view current) override {
        if (!echoEnabled_ || previous == current) return;
        CONSOLE_SCREEN_BUFFER_INFO info{};
        if (!GetConsoleScreenBufferInfo(output_, &info) || info.dwSize.X <= 0)
            throw Error("CONSOLE_OUTPUT_FAILED", "Cannot read console cursor for line editing.");
        const int cells = displayWidth(previous);
        const int position = static_cast<int>(info.dwCursorPosition.Y) * info.dwSize.X + info.dwCursorPosition.X;
        if (position < cells) throw Error("CONSOLE_OUTPUT_FAILED", "The edited line scrolled outside the console buffer.");
        const int start = position - cells;
        const COORD origin{static_cast<SHORT>(start % info.dwSize.X), static_cast<SHORT>(start / info.dwSize.X)};
        if (!SetConsoleCursorPosition(output_, origin)) throw Error("CONSOLE_OUTPUT_FAILED", "Cannot position the line editor.");
        write(std::wstring(static_cast<std::size_t>(cells), L' '));
        if (!SetConsoleCursorPosition(output_, origin)) throw Error("CONSOLE_OUTPUT_FAILED", "Cannot restore the line editor cursor.");
        write(current);
    }
    void newline() override { if (echoEnabled_) write(L"\r\n"); }
private:
    void write(std::wstring_view text) {
        if (text.empty()) return;
        DWORD written = 0;
        if (!WriteConsoleW(output_, text.data(), static_cast<DWORD>(text.size()), &written, nullptr)
            || written != text.size()) throw Error("CONSOLE_OUTPUT_FAILED", "Cannot echo console input.");
    }
    ConsoleControlScope &controls_;
    HANDLE input_ = GetStdHandle(STD_INPUT_HANDLE), output_ = GetStdHandle(STD_OUTPUT_HANDLE);
    bool echoEnabled_ = false;
    WORD repeats_ = 0;
    Key repeated_;
};
std::optional<std::string> line(std::istream &input, std::ostream &output, ConsoleControlScope &controls,
    const std::string &prompt, detail::PlainReadObserver *observer)
{
    print(output, prompt);
    DWORD mode = 0;
    if (&input == &std::cin && GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mode)) {
        const auto value = readNativePlainConsoleLine(controls, observer);
        return value ? std::optional<std::string>(trimmed(wideToUtf8(*value))) : std::nullopt;
    }
    if (&input == &std::cin) {
        const auto value = detail::readRedirectedPlainLine(GetStdHandle(STD_INPUT_HANDLE), controls, observer);
        return value ? std::optional<std::string>(trimmed(*value)) : std::nullopt;
    }
    std::string value; char ch = 0;
    while (input.get(ch)) {
        if (ch == '\n') return trimmed(value);
        if (value.size() >= 4096) throw Error("INPUT_INVALID", "Input exceeds its limit.");
        value.push_back(ch);
    }
    if (input.eof()) return value.empty() ? std::nullopt : std::optional<std::string>(trimmed(value));
    throw Error("CONSOLE_INPUT_FAILED", "Cannot read console input.");
}
Key shortcut(wchar_t ch)
{
    Key key; key.unicode = ch; key.virtualKey = static_cast<WORD>(std::towupper(ch)); return key;
}
Key enter() { Key key; key.virtualKey = VK_RETURN; return key; }
void wait(TuiController &controller, ConsoleControlScope &controls)
{
    while (controller.busy()) {
        if (controls.takeCancel()) controller.cancel();
        if (controls.takeClose()) controller.requestClose();
        controller.poll();
        if (controller.busy()) Sleep(10);
    }
}
}
std::optional<std::wstring> readNativePlainConsoleLine(ConsoleControlScope &controls, detail::PlainReadObserver *observer)
{
    NativePlainInput input(controls);
    return readPlainConsoleLine(input, observer);
}
int runPlainConsoleSession(EngineSession &session, std::istream &input, std::ostream &output, detail::PlainReadObserver *observer)
{
    ConsoleControlScope controls;
    // This function may be given an externally owned session. Even on an
    // input/output exception, reap its operation before acknowledging close.
    struct SettleBeforeControlRelease {
        EngineSession &session;
        ~SettleBeforeControlRelease() {
            session.cancel();
            while (session.busy()) {
                try { session.poll(); } catch (...) { /* poll can throw after reaping; retain the original exception. */ }
                if (session.busy()) Sleep(1);
            }
        }
    } settle{session};
    TuiState state;
    TuiController controller(session, state);
    print(output, std::string("Codex Proxy Guard ") + productVersion() + " - local proxy launcher");
    controller.refresh(); wait(controller, controls);
    while (!controller.shouldExit()) {
        const auto view = session.view();
        print(output, "Proxy: " + view.proxy.url + " | Process: " + view.process.state + " | Coverage: " + view.coverage.state);
        if (view.error) print(output, view.error->code + ": " + view.error->message);
        const auto action = line(input, output, controls, "[L] Launch  [E] Edit proxy  [B] Backend authorization  [D] Repair launch  [R] Refresh  [Q] Quit", observer);
        // The native reader consumes cancellation internally. An empty result
        // is EOF or close, including when close also set the cancel signal.
        if (!action) return 0;
        if (controls.takeClose()) { controller.requestClose(); break; }
        controls.takeCancel(); // idle Ctrl+C is a no-op, preserving proposals
        if (action->empty()) continue;
        if (action->size() != 1) { print(output, "Choose one listed command."); continue; }
        const wchar_t command = static_cast<wchar_t>(std::tolower(static_cast<unsigned char>((*action)[0])));
        const bool retryDraft = command == L'e' && state.modal() == Modal::ProxyEditor;
        if (!retryDraft) { state.clearModal(); controller.handleKey(shortcut(command)); }
        if (command == L'e' && state.modal() == Modal::ProxyEditor) {
            const auto host = line(input, output, controls, "Loopback HTTP/Mixed host [" + wideToUtf8(state.editorHost()) + "] (Enter keeps draft):", observer); if (!host) return 0;
            const auto port = line(input, output, controls, "Port [" + wideToUtf8(state.editorPort()) + "] (Enter keeps draft):", observer); if (!port) return 0;
            state.setEditorSeed(host->empty() ? state.editorHost() : utf8ToWide(*host), port->empty() ? state.editorPort() : utf8ToWide(*port));
            controller.handleKey(enter());
            if (!state.editorError().empty()) print(output, wideToUtf8(state.editorError()));
        } else if ((command == L'b' && state.modal() == Modal::Consent) || (command == L'd' && state.modal() == Modal::Repair)) {
            if (command == L'b' && state.consentProposal()) {
                const auto &proposal = *state.consentProposal();
                print(output, std::string(proposal.enabled ? "Authorize Guard proxy block for later Codex processes in: " : "Remove Guard's unchanged proxy block from: ") + wideToUtf8(proposal.home.wstring()));
            } else print(output, "Repair stops the shared Codex daemon once and may interrupt other clients' tasks on the same Home.");
            const auto confirm = line(input, output, controls, "Type YES to confirm this single operation (anything else cancels):", observer); if (!confirm) return 0;
            if (*confirm == "YES") {
                for (const wchar_t ch : std::wstring_view(L"YES")) controller.handleKey(shortcut(ch));
                controller.handleKey(enter());
            } else state.clearModal();
        }
        wait(controller, controls);
        if (state.modal() == Modal::ErrorResult || state.modal() == Modal::LaunchResult) print(output, state.diagnosticText());
        else if (!state.editorError().empty()) print(output, wideToUtf8(state.editorError()));
        else if (!session.view().actions.canLaunch && command == L'l') print(output, session.view().actions.blockedReason);
        if (!state.notice().empty()) print(output, state.notice());
    }
    return 0;
}
int runPlainConsole(const std::filesystem::path &configPath)
{
    try { EngineSession session(configPath); return runPlainConsoleSession(session, std::cin, std::cout); }
    catch (const Error &error) { std::cerr << error.code << ": " << publicText(error.message, 4096) << std::endl; return 1; }
}
} // namespace cpg::tui
