#include "startup_mode.h"

#include "cli.h"
#include "cpg/text.h"
#include "tui_app.h"

#include <windows.h>
#include <shellapi.h>

#include <string>
#include <vector>

namespace {

// Bounded diagnostic output for rejected startups.
void writeStdError(const std::wstring &message)
{
    const std::string bytes = cpg::wideToUtf8(message) + "\n";
    const HANDLE error = GetStdHandle(STD_ERROR_HANDLE);
    if (error == INVALID_HANDLE_VALUE || error == nullptr) return;
    DWORD written = 0;
    WriteFile(error, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr);
}

std::filesystem::path configPathFrom(const cpg::StartupMode &mode)
{
    // Relative --config values resolve against the current directory before
    // the interactive frontend opens, mirroring the previous GUI behavior.
    if (mode.configPath.empty()) return {};
    std::error_code error;
    const std::filesystem::path absolute = std::filesystem::absolute(mode.configPath, error);
    return error ? std::filesystem::path(mode.configPath) : absolute;
}

std::vector<std::wstring> wideArguments()
{
    // Unicode-safe early read; the CLI parser sees the same wide command line,
    // keeping classification identical to what runCli will parse.
    int count = 0;
    std::vector<std::wstring> arguments;
    if (LPWSTR *wide = CommandLineToArgvW(GetCommandLineW(), &count)) {
        arguments.reserve(count > 1 ? static_cast<std::size_t>(count - 1) : 0);
        for (int index = 1; index < count; ++index)
            arguments.emplace_back(wide[index]);
        LocalFree(wide);
        return arguments;
    }
    return arguments;
}

} // namespace

int main(int argc, char *argv[])
{
    (void)argc;
    (void)argv;
    const std::vector<std::wstring> arguments = wideArguments();
    const cpg::StartupMode mode = cpg::classifyStartup(arguments);
    if (mode.shouldExit) {
        writeStdError(mode.errorMessage);
        return mode.exitCode;
    }
    cpg::setInteractiveConsoleEntry(&cpg::tui::runTui);
    if (mode.role == cpg::StartupRole::Gui) {
        // No-arg starts the interactive TUI (plain fallback through runTui
        // when VT output or an interactive console is unavailable).
        return cpg::tui::runTui(configPathFrom(mode), false);
    }
    return cpg::runCli(arguments);
}
