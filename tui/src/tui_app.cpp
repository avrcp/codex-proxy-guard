#include "tui_app.h"
#include "cpg/build_info.h"
#include "clipboard.h"
#include "cli_console.h"
#include "input.h"
#include "screen.h"
#include "session.h"
#include "terminal_session.h"
#include "cpg/text.h"
#include "tui_state.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <atomic>
#include <cstdio>
#include <cwchar>
#include <exception>
#include <iostream>
#include <string>

namespace cpg::tui {
namespace {

constexpr int minimumWidth = 72;
constexpr int minimumHeight = 22;
constexpr DWORD renderSliceMs = 100;

std::atomic_bool interruptSeen{false};
BOOL WINAPI interruptHandler(DWORD signal)
{
    if (signal == CTRL_C_EVENT || signal == CTRL_BREAK_EVENT) {
        interruptSeen.store(true);
        return TRUE;
    }
    return FALSE;
}

struct ConsoleSize {
    int width = 0;
    int height = 0;
    bool operator!=(const ConsoleSize &other) const
    {
        return width != other.width || height != other.height;
    }
};

ConsoleSize consoleSize(HANDLE output)
{
    CONSOLE_SCREEN_BUFFER_INFO info{};
    if (!GetConsoleScreenBufferInfo(output, &info)) return {};
    return {info.srWindow.Right - info.srWindow.Left + 1,
            info.srWindow.Bottom - info.srWindow.Top + 1};
}

// Manual §50: the clipboard diagnostic carries only version/commit, the error
// code, public desktop fields and the coverage state — never .env content,
// tokens or the full USERPROFILE (the message was already publicText()-ed).
std::wstring diagnosticText(const TuiState &state)
{
    const Json &snapshot = state.snapshot();
    std::string text = "Codex Proxy Guard diagnostic\n";
    text += std::string("version: ") + productVersion() + "\n";
    text += std::string("commit: ") + buildCommit() + "\n";
    if (snapshot.contains("desktop") && snapshot.at("desktop").contains("state")) {
        const Json &desktop = snapshot.at("desktop");
        for (const char *key : {"state", "product", "display_name", "package_version",
                                "architecture", "application_id", "runtime_kind"})
            if (desktop.contains(key) && desktop.at(key).is_string())
                text += std::string("desktop.") + key + ": " + desktop.at(key).get<std::string>() + "\n";
    }
    if (snapshot.contains("coverage") && snapshot.at("coverage").contains("state")
        && snapshot.at("coverage").at("state").is_string())
        text += "coverage: " + snapshot.at("coverage").at("state").get<std::string>() + "\n";
    return utf8ToWide(text);
}

// Same missing-key contract as tui_state's at(): an empty object, never null
// (nlohmann's value() throws on a null receiver and the TUI must render
// before the first session snapshot arrives).
const Json &snapshotField(const Json &snapshot, const char *key)
{
    static const Json empty = Json::object();
    const auto entry = snapshot.find(key);
    return entry == snapshot.end() ? empty : *entry;
}

int runInteractive(const std::filesystem::path &configPath)
{
    TerminalSession terminal;
    if (!terminal.enable()) return runPlainConsole(configPath);
    EngineSession session(configPath);
    TuiState state;
    Screen screen;

    std::string alternateScreen = "auto";
    try {
        const Config config = Config::loadOrCreate(configPath);
        alternateScreen = config.alternateScreen;
    } catch (const Error &) {
        // An unreadable configuration still renders the repair view.
    }
    ConsoleSize size = consoleSize(terminal.output());
    if (alternateScreen == "always") {
        if (!terminal.enterAlternateScreen()) return runPlainConsole(configPath);
    } else if (alternateScreen == "auto"
               && size.width >= minimumWidth && size.height >= minimumHeight) {
        terminal.enterAlternateScreen();
    }

    SetConsoleCtrlHandler(interruptHandler, TRUE);
    session.requestRefresh();
    int spinner = 0;
    bool quit = false;
    while (!quit) {
        const ConsoleSize current = consoleSize(terminal.output());
        const bool tooSmall = current.width < minimumWidth || current.height < minimumHeight;
        if (tooSmall) {
            screen.resize(std::max(current.width, 24), std::max(current.height, 6));
            screen.clear();
            screen.text(1, 1, L"Window too small for the TUI.", StyleYellow);
            screen.text(1, 2, L"Enlarge to at least 72x22. Q or Esc quits.", StyleDim);
        } else {
            screen.resize(current.width, current.height);
            state.draw(screen);
        }
        const std::wstring frame = renderFrame(screen);
        DWORD written = 0;
        WriteConsoleW(terminal.output(), frame.data(), static_cast<DWORD>(frame.size()),
                      &written, nullptr);

        InputBatch batch;
        if (readInputBatch(terminal.input(), batch, renderSliceMs)) {
            for (const Key &key : batch.keys) {
                if (tooSmall) {
                    if (key.unicode == L'Q' || key.unicode == L'q'
                        || key.virtualKey == VK_ESCAPE) quit = true;
                    continue;
                }
                const Json &snapshot = state.snapshot();
                const TuiState::Action action = state.key(key);
                try {
                    switch (action.kind) {
                    case TuiState::Action::Kind::Quit:
                        quit = true;
                        break;
                    case TuiState::Action::Kind::Refresh:
                        session.requestRefresh();
                        break;
                    case TuiState::Action::Kind::Launch:
                        if (snapshotField(snapshot, "actions").value("can_launch", false))
                            session.requestLaunch(false, "");
                        break;
                    case TuiState::Action::Kind::OpenEditor: {
                        const Json &proxy = snapshotField(snapshot, "proxy");
                        state.setEditorSeed(utf8ToWide(proxy.value("host", "127.0.0.1")),
                                            std::to_wstring(proxy.value("port", 10808)));
                        break;
                    }
                    case TuiState::Action::Kind::OpenConsent: {
                        const Json &coverage = snapshotField(snapshot, "coverage");
                        state.setConfirmHome(utf8ToWide(
                            coverage.value("candidate_home", coverage.value("authorized_home", ""))));
                        break;
                    }
                    case TuiState::Action::Kind::OpenRepair:
                        break;
                    case TuiState::Action::Kind::SaveProxy:
                        session.requestSetProxy(wideToUtf8(action.host), action.port);
                        state.clearModal();
                        break;
                    case TuiState::Action::Kind::ConfirmConsent:
                        session.requestSetBackendConsent(action.enableConsent, action.token);
                        state.clearModal();
                        break;
                    case TuiState::Action::Kind::ConfirmRepair:
                        session.requestLaunch(true, action.token);
                        state.clearModal();
                        break;
                    case TuiState::Action::Kind::Cancel:
                        session.cancel();
                        break;
                    case TuiState::Action::Kind::CopyDiagnostic:
                        if (copyDiagnosticToClipboard(diagnosticText(state))) {
                            state.setNotice("Diagnostic copied");
                        } else {
                            const Error failure("CLIPBOARD_FAILED",
                                                "Cannot copy the diagnostic to the clipboard.");
                            state.setOperationOutcome(&failure);
                        }
                        break;
                    case TuiState::Action::Kind::None:
                        break;
                    }
                } catch (const Error &error) {
                    state.setOperationOutcome(&error);
                }
            }
        }
        if (interruptSeen.exchange(false)) session.cancel();
        switch (session.poll()) {
        case EngineSession::Event::SnapshotArrived:
            state.setSnapshot(session.snapshot());
            break;
        case EngineSession::Event::OperationFinished: {
            state.setSnapshot(session.snapshot());
            const auto error = session.operationError();
            state.setOperationOutcome(error ? &*error : nullptr);
            break;
        }
        case EngineSession::Event::MutationFailed: {
            const auto error = session.mutationError();
            if (error) state.setOperationOutcome(&*error);
            break;
        }
        case EngineSession::Event::None:
            break;
        }
        state.setBusy(session.busy(), session.cancellingAfterTimeout()
            ? "Cancelling timed-out operation" : session.launching() ? "Launching Desktop" : "Updating");
        state.setSpinnerPhase(spinner++);
    }
    SetConsoleCtrlHandler(interruptHandler, FALSE);
    return 0;
}

} // namespace

int runTui(const std::filesystem::path &configPath, bool plainOverride)
{
    const bool interactive = TerminalSession::isInteractiveConsole();
    if (plainOverride || !interactive)
        return runPlainConsole(configPath);
    try {
        return runInteractive(configPath);
    } catch (const std::exception &error) {
        // TerminalSession's RAII teardown already restored the console; one
        // redacted public line on stderr is the whole diagnostic story. No
        // persistent crash log is written.
        std::cerr << "TUI_FAILED: " << publicText(error.what(), 200) << std::endl;
        return 1;
    } catch (...) {
        std::cerr << "TUI_FAILED: unknown error" << std::endl;
        return 1;
    }
}

} // namespace cpg::tui
