#include "startup_mode.h"

namespace cpg {
namespace {

StartupMode rejected(const QString &message) {
    StartupMode mode;
    mode.role = StartupRole::Cli;
    mode.shouldExit = true;
    mode.exitCode = 1;
    mode.errorMessage = QStringLiteral("CLI_INVALID: ") + message;
    return mode;
}

QString bounded(const QString &value) {
    const QString flattened = value.simplified();
    return flattened.size() > 64 ? flattened.left(64) + QStringLiteral("...") : flattened;
}

const QStringList &knownCommands() {
    static const QStringList commands{
        QStringLiteral("bridge"), QStringLiteral("internal-activate-package"),
        QStringLiteral("build-info"), QStringLiteral("launch"),
        QStringLiteral("init-config"), QStringLiteral("config-path"),
        QStringLiteral("console"), QStringLiteral("licenses")};
    return commands;
}

} // namespace

StartupMode classifyStartup(const QStringList &arguments) {
    bool configSeen = false;
    bool configPending = false;
    bool smokeTest = false;
    bool guiWords = false;
    bool cliWords = false;
    QString configValue;
    QStringList positional;
    bool afterSeparator = false;
    for (const QString &argument : arguments) {
        if (configPending) {
            // A value token is consumed verbatim, even when it starts with '-'.
            configValue = argument;
            configPending = false;
            continue;
        }
        if (!afterSeparator && argument == QLatin1String("--")) {
            afterSeparator = true;
            continue;
        }
        if (!afterSeparator && argument.startsWith(QLatin1Char('-')) && argument.size() > 1) {
            if (argument == QLatin1String("--config")) {
                if (configSeen) return rejected(QStringLiteral("The configuration option may be given only once."));
                configSeen = true;
                configPending = true;
                continue;
            }
            if (argument.startsWith(QLatin1String("--config="))) {
                if (configSeen) return rejected(QStringLiteral("The configuration option may be given only once."));
                configSeen = true;
                configValue = argument.mid(QStringLiteral("--config=").size());
                continue;
            }
            if (argument == QLatin1String("--smoke-test")) {
                smokeTest = true;
                guiWords = true;
                continue;
            }
            // Any other option belongs to the headless parser vocabulary
            // (--build-info, --help, --version, --json, --force, ...). The
            // parser itself rejects unknown spellings and command mismatches;
            // classification only needs the vocabulary boundary.
            cliWords = true;
            continue;
        }
        positional.append(argument);
    }
    if (configPending) return rejected(QStringLiteral("The configuration option requires a path value."));
    if (!positional.isEmpty()) {
        if (guiWords) return rejected(QStringLiteral("The smoke test cannot be combined with a command."));
        if (!knownCommands().contains(positional.first()))
            return rejected(QStringLiteral("Unknown command: ") + bounded(positional.first()));
        StartupMode mode;
        mode.role = StartupRole::Cli;
        return mode;
    }
    if (guiWords && cliWords)
        return rejected(QStringLiteral("The smoke test cannot be combined with command-line options."));
    StartupMode mode;
    mode.role = cliWords ? StartupRole::Cli : StartupRole::Gui;
    mode.smokeTest = smokeTest;
    if (mode.role == StartupRole::Gui) mode.configPath = configValue;
    return mode;
}

} // namespace cpg
