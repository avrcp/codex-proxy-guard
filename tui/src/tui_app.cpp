#include "tui_app.h"
#include "cli_console.h"
#include "input.h"
#include "screen.h"
#include "session.h"
#include "terminal_session.h"
#include "interactive_terminal_scope.h"
#include "tui_controller.h"
#include "cpg/console_control.h"
#include "cpg/text.h"
#include <exception>
#include <iostream>
namespace cpg::tui {
namespace {
constexpr int minimumWidth = 72, minimumHeight = 22;
constexpr DWORD renderSliceMs = 100;
// The sentinel exits the entire VT scope before the caller enters plain mode.
constexpr int plainFallback = -2;
int runInteractive(const std::filesystem::path &configPath)
{
    detail::InteractiveTerminalScope terminalScope;
    auto &terminal = terminalScope.terminal;
    auto &controls = terminalScope.controls;
    if (!terminal.enable()) return plainFallback;
    std::string alternateScreen = "auto";
    try { alternateScreen = Config::loadOrCreate(configPath).alternateScreen; }
    catch (const Error &) {}
    CONSOLE_SCREEN_BUFFER_INFO initial{};
    if (!GetConsoleScreenBufferInfo(terminal.output(), &initial)) return plainFallback;
    const bool initialRoom = initial.srWindow.Right - initial.srWindow.Left + 1 >= minimumWidth
        && initial.srWindow.Bottom - initial.srWindow.Top + 1 >= minimumHeight;
    if ((alternateScreen == "always" || (alternateScreen == "auto" && initialRoom))
        && !terminal.enterAlternateScreen()) return plainFallback;
    EngineSession session(configPath);
    TuiState state;
    TuiController controller(session, state);
    controller.refresh();
    Screen screen;
    int spinner = 0;
    while (!controller.shouldExit()) {
        CONSOLE_SCREEN_BUFFER_INFO size{};
        if (!GetConsoleScreenBufferInfo(terminal.output(), &size)) throw Error("CONSOLE_OUTPUT_FAILED", "Cannot read console viewport.");
        const int width = size.srWindow.Right - size.srWindow.Left + 1;
        const int height = size.srWindow.Bottom - size.srWindow.Top + 1;
        const bool tooSmall = width < minimumWidth || height < minimumHeight;
        screen.resize(width, height);
        if (tooSmall) {
            screen.clear();
            if (state.modal() == Modal::QuitWhileBusy)
                screen.text(0, 0, L"Cancel and quit? Y yes / Esc keep running", StyleYellow);
            else {
                screen.text(0, 0, L"Enlarge to at least 72x22.", StyleYellow);
                screen.text(0, 1, controller.busy() ? L"Q asks to cancel and quit; Ctrl+C cancels." : L"Q quits.", StyleDim);
            }
        } else state.draw(screen);
        const auto frame = renderFrame(screen);
        DWORD written = 0;
        if (!WriteConsoleW(terminal.output(), frame.data(), static_cast<DWORD>(frame.size()), &written, nullptr)
            || written != frame.size()) throw Error("CONSOLE_OUTPUT_FAILED", "Console frame write failed.");
        InputBatch batch;
        if (readInputBatch(terminal.input(), batch, renderSliceMs)) {
            for (const auto &key : batch.keys) {
                if (tooSmall && state.modal() != Modal::QuitWhileBusy && !key.control
                    && key.unicode != L'q' && key.unicode != L'Q') continue;
                controller.handleKey(key);
                if (controller.exitRequested()) break;
            }
        }
        if (controls.takeCancel()) controller.cancel();
        if (controls.takeClose()) controller.requestClose();
        controller.poll(); state.setSpinnerPhase(spinner++);
    }
    return 0;
}
}
int runTui(const std::filesystem::path &configPath, bool plainOverride)
{
    try {
        if (plainOverride || !TerminalSession::isInteractiveConsole()) return runPlainConsole(configPath);
        const int result = runInteractive(configPath);
        return result == plainFallback ? runPlainConsole(configPath) : result;
    } catch (const std::exception &error) {
        std::cerr << "TUI_FAILED: " << publicText(error.what(), 200) << std::endl; return 1;
    } catch (...) { std::cerr << "TUI_FAILED: unknown error" << std::endl; return 1; }
}
} // namespace cpg::tui
