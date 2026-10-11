#pragma once

#include <string>
#include <vector>

namespace cpg {

enum class StartupRole { Gui, Cli };

struct StartupMode {
    StartupRole role = StartupRole::Gui;
    // Gui role only: --config value to forward to the interactive frontend.
    std::wstring configPath;
    bool smokeTest = false;
    // Classification failure: print errorMessage to standard error and exit
    // with exitCode without entering any role.
    bool shouldExit = false;
    int exitCode = 0;
    std::wstring errorMessage;
};

// Classify the raw argument vector (excluding argv[0]) into the process role.
// Parsing is structured: option values are consumed as values and never
// matched as role words, so a --config path containing "bridge" cannot flip
// the role. Every spelling the full CLI parser accepts (--config=path
// included) is recognized; anything else is delegated to the parser or
// rejected here with a nonzero exit.
StartupMode classifyStartup(const std::vector<std::wstring> &arguments);

} // namespace cpg
