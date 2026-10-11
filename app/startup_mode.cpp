#include "startup_mode.h"

#include <cwctype>

namespace cpg {
namespace {

StartupMode rejected(const std::wstring &message)
{
    StartupMode mode;
    mode.role = StartupRole::Cli;
    mode.shouldExit = true;
    mode.exitCode = 1;
    mode.errorMessage = L"CLI_INVALID: " + message;
    return mode;
}

std::wstring bounded(const std::wstring &value)
{
    // Flatten whitespace and cap the echoed token so a hostile argument
    // cannot flood the diagnostic line.
    std::wstring flattened;
    bool pending = false;
    for (const wchar_t character : value) {
        if (std::iswspace(character)) { pending = true; continue; }
        if (pending && !flattened.empty()) flattened.push_back(L' ');
        pending = false;
        flattened.push_back(character);
    }
    if (flattened.size() > 64) return flattened.substr(0, 64) + L"...";
    return flattened;
}

bool equalsW(const std::wstring &left, const wchar_t *right)
{
    return left == right;
}

const std::vector<const wchar_t *> &knownCommands()
{
    static const std::vector<const wchar_t *> commands{
        L"internal-activate-package",
        L"build-info", L"launch",
        L"init-config", L"config-path",
        L"console", L"licenses"};
    return commands;
}

} // namespace

StartupMode classifyStartup(const std::vector<std::wstring> &arguments) {
    bool configSeen = false;
    bool configPending = false;
    bool cliWords = false;
    std::wstring configValue;
    std::vector<std::wstring> positional;
    bool afterSeparator = false;
    for (const std::wstring &argument : arguments) {
        if (configPending) {
            // A value token is consumed verbatim, even when it starts with '-'.
            configValue = argument;
            configPending = false;
            continue;
        }
        if (!afterSeparator && equalsW(argument, L"--")) {
            afterSeparator = true;
            continue;
        }
        if (!afterSeparator && argument.size() > 1 && argument[0] == L'-') {
            if (equalsW(argument, L"--config")) {
                if (configSeen) return rejected(L"The configuration option may be given only once.");
                configSeen = true;
                configPending = true;
                continue;
            }
            if (argument.rfind(L"--config=", 0) == 0) {
                if (configSeen) return rejected(L"The configuration option may be given only once.");
                configSeen = true;
                configValue = argument.substr(9);
                continue;
            }
            // Any other option belongs to the headless parser vocabulary
            // (--build-info, --help, --version, --json, --force, ...). The
            // parser itself rejects unknown spellings and command mismatches;
            // classification only needs the vocabulary boundary.
            cliWords = true;
            continue;
        }
        positional.push_back(argument);
    }
    if (configPending) return rejected(L"The configuration option requires a path value.");
    if (!positional.empty()) {
        bool known = false;
        for (const wchar_t *command : knownCommands())
            if (equalsW(positional.front(), command)) { known = true; break; }
        if (!known) return rejected(L"Unknown command: " + bounded(positional.front()));
        StartupMode mode;
        mode.role = StartupRole::Cli;
        return mode;
    }
    StartupMode mode;
    mode.role = cliWords ? StartupRole::Cli : StartupRole::Gui;
    if (mode.role == StartupRole::Gui) mode.configPath = configValue;
    return mode;
}

} // namespace cpg
