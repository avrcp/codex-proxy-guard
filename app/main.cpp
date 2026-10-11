#include "startup_mode.h"

#include "cli.h"
#include "cpg/text.h"

#ifdef CPG_ENABLE_QT_GUI
#include "gui_entry.h"
#endif

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
    if (mode.role == cpg::StartupRole::Gui) {
#ifdef CPG_ENABLE_QT_GUI
        return guard::runGui(mode.configPath, mode.smokeTest);
#else
        // No-Qt interim build: the interactive frontend is the console until
        // the TUI lands; headless roles parse the same arguments.
        return cpg::runCli(arguments);
#endif
    }
    return cpg::runCli(arguments);
}
