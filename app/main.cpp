#include "startup_mode.h"

#include "cli.h"
#include "gui_entry.h"

#include <QApplication>
#include <QCoreApplication>
#include <QDir>
#include <QStringList>
#include <windows.h>
#include <shellapi.h>

namespace {

// Bounded diagnostic output for rejected startups. GUI-subsystem processes may
// have no console; a missing standard error handle is a clean nonzero exit.
void writeStdError(const QString &message) {
    const auto bytes = message.toUtf8() + '\n';
    const HANDLE error = GetStdHandle(STD_ERROR_HANDLE);
    if (error == INVALID_HANDLE_VALUE || error == nullptr) return;
    DWORD written = 0;
    WriteFile(error, bytes.constData(), static_cast<DWORD>(bytes.size()), &written, nullptr);
}

QStringList wideArguments(int argc, char *argv[]) {
    // Unicode-safe early read; Qt's own arguments() uses the same wide command
    // line, keeping classification identical to what the parser will see.
    int count = 0;
    if (LPWSTR *wide = CommandLineToArgvW(GetCommandLineW(), &count)) {
        QStringList arguments;
        arguments.reserve(count > 1 ? count - 1 : 0);
        for (int index = 1; index < count; ++index)
            arguments.append(QString::fromWCharArray(wide[index]));
        LocalFree(wide);
        return arguments;
    }
    QStringList arguments;
    for (int index = 1; index < argc; ++index)
        arguments.append(QString::fromLocal8Bit(argv[index]));
    return arguments;
}

} // namespace

int main(int argc, char *argv[]) {
    Q_INIT_RESOURCE(discovery);
    const cpg::StartupMode mode = cpg::classifyStartup(wideArguments(argc, argv));
    if (mode.shouldExit) {
        writeStdError(mode.errorMessage);
        return mode.exitCode;
    }
    if (mode.role == cpg::StartupRole::Gui) {
        QApplication app(argc, argv);
        QString configPath = mode.configPath;
        if (!configPath.isEmpty() && QDir::isRelativePath(configPath))
            configPath = QDir(QDir::currentPath()).absoluteFilePath(configPath);
        return guard::runGui(app, configPath, mode.smokeTest);
    }
    QCoreApplication app(argc, argv);
    return cpg::runCli(app);
}
