#pragma once

#include <QString>
#include <QStringList>

namespace cpg {

enum class StartupRole { Gui, Cli };

struct StartupMode {
    StartupRole role = StartupRole::Gui;
    // Gui role only: --config value to forward to the bridge child process.
    QString configPath;
    bool smokeTest = false;
    // Classification failure: print errorMessage to standard error and exit
    // with exitCode without constructing any Application object.
    bool shouldExit = false;
    int exitCode = 0;
    QString errorMessage;
};

// Classify the raw argument vector (excluding argv[0]) into the process role.
// Parsing is structured: option values are consumed as values and never
// matched as role words, so a --config path containing "bridge" cannot flip
// the role. Every spelling the full CLI parser accepts (--config=path
// included) is recognized; anything else is delegated to the parser or
// rejected here with a nonzero exit.
StartupMode classifyStartup(const QStringList &arguments);

}
