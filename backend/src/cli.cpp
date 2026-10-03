#include "cli.h"
#include "bridge.h"
#include <QCommandLineParser>
#include <QDirIterator>
#include <QFile>
#include <QJsonDocument>
#include <QTextStream>
#include <iostream>
#include <utility>
#include <windows.h>

namespace {
std::atomic_bool consoleOperation{false};
cpg::Cancellation consoleCancellation;
BOOL WINAPI interruptConsole(DWORD signal) {
    if ((signal == CTRL_C_EVENT || signal == CTRL_BREAK_EVENT) && consoleOperation.load()) {
        consoleCancellation.cancel(); return TRUE;
    }
    return FALSE;
}
struct ConsoleOperation {
    ConsoleOperation() { consoleCancellation.flag->store(false); consoleOperation.store(true); }
    ~ConsoleOperation() { consoleOperation.store(false); }
};
void print(const QString &text) { std::cout << text.toUtf8().constData() << std::endl; }
void print(const QJsonObject &object) { print(QString::fromUtf8(QJsonDocument(object).toJson(QJsonDocument::Compact))); }
QString line(const QString &prompt) {
    print(prompt); std::string value; char character = 0;
    while (std::cin.get(character) && character != '\n') {
        if (value.size() >= 4096) throw cpg::Error("INPUT_INVALID", "Input exceeds its limit.");
        value.push_back(character);
    }
    if (!std::cin && value.empty()) throw cpg::Error("CANCELLED", "Input closed.");
    return QString::fromUtf8(value).trimmed();
}
int console(const QString &path) {
    print("Codex Proxy Guard " CPG_PRODUCT_VERSION " — local proxy launcher");
    for (;;) {
        cpg::Config config; bool ready = false;
        try { config = cpg::Config::loadOrCreate(path); ready = true; }
        catch (const cpg::Error &e) { print(e.code + ": " + e.message); }
        print("Proxy: " + config.proxyUrl() + " | backend block: " + (config.manageBackend ? "authorized" : "off"));
        const auto action = line("[L] Launch  [E] Edit proxy  [B] Backend authorization  [R] Repair launch  [S] Refresh  [Q] Quit").toLower();
        if (action == "q") return 0;
        try {
            if (action == "s") {
                if (!ready) continue;
                const ConsoleOperation operation;
                const auto desktop = cpg::discoverDesktop(config, consoleCancellation);
                print(desktop.displayName + " | " + cpg::desktopProcessState(desktop).state + " | " + (desktop.registered ? cpg::inspectProxyEnv(config) : "process environment"));
            } else if (action == "e") {
                const auto host = line("Loopback HTTP/Mixed host:");
                bool valid = false; const int port = line("Port (1–65535):").toInt(&valid);
                if (!valid) throw cpg::Error("CONFIG_INVALID", "Invalid proxy port.");
                cpg::updateProxy(path, config, !ready, host, port);
            } else if (action == "b" && ready) {
                const auto home = cpg::consentHome(config);
                print((config.manageBackend ? "Remove Guard's unchanged proxy block from: " : "Authorize HTTP_PROXY/HTTPS_PROXY/NO_PROXY block for later Codex processes in: ") + home);
                if (line("Type YES to confirm this single operation:") == "YES") cpg::updateConsent(path, config, !config.manageBackend, home);
            } else if ((action == "l" || action == "r") && ready) {
                const bool repair = action == "r";
                if (repair && line("Repair stops the shared Codex daemon once and may interrupt other clients' tasks. Type YES to confirm:") != "YES") continue;
                const ConsoleOperation operation;
                print(cpg::launchPipeline(config, path, {repair, false}, consoleCancellation));
            }
        } catch (const cpg::Error &e) { print(e.code + ": " + e.message); }
    }
}
// Stream one embedded license resource to stdout. License text can exceed the
// bridge frame budget, so it never goes through the NDJSON protocol writer.
bool writeResource(const QString &resourcePath) {
    QFile file(resourcePath);
    if (!file.open(QIODevice::ReadOnly)) return false;
    char buffer[16384];
    const HANDLE output = GetStdHandle(STD_OUTPUT_HANDLE);
    while (const qint64 read = file.read(buffer, sizeof(buffer))) {
        if (read < 0) return false;
        DWORD written = 0;
        if (!WriteFile(output, buffer, static_cast<DWORD>(read), &written, nullptr)
            || written != static_cast<DWORD>(read))
            return false;
    }
    return true;
}
int licenses() {
    const char separator[] = "\n-----\n\n";
    const HANDLE output = GetStdHandle(STD_OUTPUT_HANDLE);
    DWORD written = 0;
    const auto emitChunk = [output, &written](const char *data, size_t size) {
        return WriteFile(output, data, static_cast<DWORD>(size), &written, nullptr)
            && written == static_cast<DWORD>(size);
    };
    // Deterministic sorted order over the root notices plus every embedded
    // Qt and third-party license text, preserving resource-relative paths.
    QStringList entries{QStringLiteral("LICENSE"), QStringLiteral("THIRD_PARTY_NOTICES.md")};
    QDirIterator iterator(QStringLiteral(":/licenses/Qt"), QDir::Files, QDirIterator::Subdirectories);
    const QString prefix = QStringLiteral(":/licenses/");
    while (iterator.hasNext())
        entries.append(iterator.next().mid(prefix.size()));
    entries.sort();
    for (const QString &name : std::as_const(entries)) {
        const QString header = QStringLiteral("===== ") + name + QStringLiteral(" =====\n");
        const auto headerBytes = header.toUtf8();
        if (!emitChunk(headerBytes.constData(), static_cast<size_t>(headerBytes.size()))) return 1;
        if (!writeResource(prefix + name)) return 1;
        if (!emitChunk(separator, sizeof(separator) - 1)) return 1;
    }
    return 0;
}
}

int cpg::runCli(QCoreApplication &app) {
    SetConsoleCtrlHandler(interruptConsole, TRUE);
    QCoreApplication::setApplicationName("codex-proxy-guard");
    QCoreApplication::setApplicationVersion(CPG_PRODUCT_VERSION);
    QCommandLineParser parser;
    parser.setApplicationDescription("Launch Desktop through a loopback HTTP/Mixed proxy.");
    parser.addHelpOption(); parser.addVersionOption();
    parser.addOption({"config", "Guard configuration file.", "path"});
    parser.addOption({"build-info", "Print embedded build provenance and exit."});
    parser.addOption({"json", "Print launch receipt as JSON."});
    parser.addOption({"refresh-codex-daemon", "Single-use authorization to stop the shared daemon before launch; may interrupt other clients."});
    parser.addOption({"activation-only", "Registered activation diagnostic without proxy arguments or Home file writes."});
    parser.addOption({"force", "Replace the Guard configuration."});
    parser.addOption({"proxy-host", "Loopback proxy host.", "host"});
    parser.addOption({"proxy-port", "Proxy port.", "port"});
    parser.addPositionalArgument("command", "launch, init-config, config-path, build-info, console, licenses, bridge; omitted: interactive console.", "[command]");
    parser.process(app);
    try {
        const auto args = parser.positionalArguments();
        if (args.size() > 1) throw cpg::Error("CLI_INVALID", "Expected a single command.");
        const auto command = args.value(0);
        const QStringList launchFlags{"json", "refresh-codex-daemon", "activation-only"};
        const QStringList initFlags{"force", "proxy-host", "proxy-port"};
        for (const auto &flag : launchFlags) if (parser.isSet(flag) && command != "launch") throw cpg::Error("CLI_INVALID", "Launch options require launch.");
        for (const auto &flag : initFlags) if (parser.isSet(flag) && command != "init-config") throw cpg::Error("CLI_INVALID", "Configuration options require init-config.");
        // The worker and provenance query have no configuration dependency.
        // In particular, a worker must not resolve a user profile before reading
        // the parent's bounded activation request.
        if (command == "internal-activate-package") {
            if (parser.isSet("config")) throw cpg::Error("CLI_INVALID", "The activation worker does not accept configuration.");
            return cpg::activationWorker();
        }
        if (command == "build-info" || parser.isSet("build-info")) {
            print(QJsonObject{{"version", CPG_PRODUCT_VERSION}, {"commit", CPG_BUILD_COMMIT}, {"dirty", QString(CPG_BUILD_DIRTY) == "true"}, {"language", "C++20"}, {"qt_version", QT_VERSION_STR}, {"protocol_version", 1},
                // Compatibility spellings for the former GUI --build-info output.
                {"product_version", CPG_PRODUCT_VERSION}, {"git_commit", CPG_BUILD_COMMIT}, {"git_dirty", QString(CPG_BUILD_DIRTY) == "true"}});
            return 0;
        }
        if (command == "licenses") return licenses();
        const auto path = parser.isSet("config") ? parser.value("config") : cpg::Config::defaultPath();
        if (command == "bridge") return cpg::runBridge(path);
        if (command == "config-path") { print(path); return 0; }
        if (command == "init-config") {
            cpg::Config config;
            if (parser.isSet("proxy-host")) config.host = parser.value("proxy-host");
            if (parser.isSet("proxy-port")) {
                bool valid = false; config.port = parser.value("proxy-port").toInt(&valid);
                if (!valid) throw cpg::Error("CONFIG_INVALID", "Invalid proxy port.");
            }
            cpg::initializeConfig(path, config, parser.isSet("force")); print("Configuration saved."); return 0;
        }
        if (command == "launch") {
            if (parser.isSet("refresh-codex-daemon") && parser.isSet("activation-only")) throw cpg::Error("CLI_INVALID", "Repair and activation-only cannot be combined.");
            const auto config = cpg::Config::loadOrCreate(path);
            const ConsoleOperation operation;
            print(cpg::launchPipeline(config, path, {parser.isSet("refresh-codex-daemon"), parser.isSet("activation-only")}, consoleCancellation)); return 0;
        }
        if (command.isEmpty() || command == "console") return console(path);
        throw cpg::Error("CLI_INVALID", "Unknown command.");
    } catch (const cpg::Error &e) {
        std::cerr << e.code.toUtf8().constData() << ": " << e.message.toUtf8().constData() << std::endl; return 1;
    } catch (...) {
        std::cerr << "ENGINE_TASK_FAILED: The operation failed." << std::endl; return 1;
    }
}
