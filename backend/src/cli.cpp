#include "cli.h"
#include "command_line.h"
#include "cpg/console_control.h"
#include <future>
#include "core.h"
#include "platform.h"
#include "launch.h"
#include "cpg/build_info.h"
#include "cpg/json.h"
#include "cpg/text.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <exception>
#include <iostream>
#include <map>
#include <optional>
#include <set>

// Generated at configure time: the sorted embedded notice set.
#include "licenses_manifest.h"

namespace {

int (*interactiveEntry)(const std::filesystem::path &, bool) = nullptr;

using cpg::jsonDump;
using cpg::productVersion;
using cpg::wideToUtf8;

void print(const std::string &text) { std::cout << text << std::endl; }
void printJson(const cpg::Json &object) { print(jsonDump(object)); }

// Stream one embedded license resource to stdout. License text can exceed the
// worker frame budget, so it is written directly to stdout.
bool writeResource(int identifier, HANDLE output) {
    const HRSRC resource = FindResourceW(nullptr, MAKEINTRESOURCEW(identifier), L"NOTICES");
    if (!resource) return false;
    const HGLOBAL loaded = LoadResource(nullptr, resource);
    if (!loaded) return false;
    const void *data = LockResource(loaded);
    const DWORD size = SizeofResource(nullptr, resource);
    if (!data || !size) return false;
    DWORD written = 0;
    return WriteFile(output, data, size, &written, nullptr) && written == size;
}

int licenses() {
    const char separator[] = "\n-----\n\n";
    const HANDLE output = GetStdHandle(STD_OUTPUT_HANDLE);
    DWORD written = 0;
    const auto emitChunk = [output, &written](const char *data, size_t size) {
        return WriteFile(output, data, static_cast<DWORD>(size), &written, nullptr)
            && written == static_cast<DWORD>(size);
    };
    for (const auto &entry : cpg::cpgLicensesManifest()) {
        const std::string header = "===== " + entry.name + " =====\n";
        if (!emitChunk(header.data(), header.size())) return 1;
        if (!writeResource(entry.id, output)) return 1;
        if (!emitChunk(separator, sizeof separator - 1)) return 1;
    }
    return 0;
}

const char *usageText()
{
    return "Usage: CodexProxyGuard [options] [command]\n"
           "Launch Desktop through a loopback HTTP/Mixed proxy.\n"
           "\n"
           "Options:\n"
           "  -h, -?, --help      Displays help on commandline options.\n"
           "  -v, --version       Displays version information.\n"
           "  --config <path>     Guard configuration file.\n"
           "\n"
           "Commands:\n"
           "  launch                   Launch Desktop\n"
           "    --json                   Print launch receipt as JSON\n"
           "    --refresh-codex-daemon   Single-use authorization to stop the shared daemon\n"
           "    --activation-only        Registered activation diagnostic\n"
           "  init-config              Write the default configuration\n"
           "    --force                  Replace the Guard configuration\n"
           "    --proxy-host <host>      Loopback proxy host\n"
           "    --proxy-port <port>      Proxy port\n"
           "  config-path              Print the effective configuration path\n"
           "  build-info               Print embedded build provenance\n"
           "  licenses                 Stream the embedded offline notices\n"
           "  console                  Interactive console (--plain for line mode)\n"
           "  internal-activate-package  Activation worker (internal)\n";
}

} // namespace

void cpg::setInteractiveConsoleEntry(int (*entry)(const std::filesystem::path &, bool))
{
    interactiveEntry = entry;
}

// The owner thread translates console signals to cancellation; the operation
// and any owned helpers settle before this scope releases its handler.
cpg::Json cpg::runConsoleOperation(const std::function<Json(const Cancellation &)> &operation)
{
    ConsoleControlScope control;
    Cancellation cancel;
    auto task = std::async(std::launch::async, [&] { return operation(cancel); });
    while (task.wait_for(std::chrono::milliseconds(10)) != std::future_status::ready) {
        if (control.takeCancel() || control.takeClose()) cancel.cancel();
    }
    return task.get();
}

int cpg::runCli(const std::vector<std::wstring> &arguments) {
    bool jsonOutput = false;
    try {
        const ParsedCommand command = parseCommandLine(arguments);
        jsonOutput = command.json;
        switch (command.kind) {
        case CommandKind::Help: std::cout << usageText(); return 0;
        case CommandKind::Version: print(std::string("codex-proxy-guard ") + productVersion()); return 0;
        case CommandKind::InternalActivatePackage: return activationWorker();
        case CommandKind::BuildInfo:
            printJson(Json{{"version", productVersion()}, {"commit", buildCommit()},
                           {"dirty", buildDirty()}, {"language", "C++20"}});
            return 0;
        case CommandKind::Licenses: return licenses();
        default: break;
        }
        const auto configPath = resolveConfigPath(command, &Config::defaultPath);
        if (command.kind == CommandKind::ConfigPath) { print(wideToUtf8(configPath.wstring())); return 0; }
        if (command.kind == CommandKind::InitConfig) {
            Config config;
            if (command.proxyHost) config.host = *command.proxyHost;
            if (command.proxyPort) config.port = *command.proxyPort;
            initializeConfig(configPath, config, command.force);
            print("Configuration saved.");
            return 0;
        }
        if (command.kind == CommandKind::Launch) {
            const auto config = Config::loadOrCreate(configPath);
            const Json receipt = runConsoleOperation([&](const Cancellation &cancel) {
                return launchPipeline(config, configPath, {command.repair, command.activationOnly}, cancel);
            });
            // A valid settled receipt remains true even if Ctrl+C arrived late.
            if (jsonOutput) printJson(receipt);
            else {
                print("Desktop activation: PID " + std::to_string(receipt.value("pid", std::uint64_t{0}))
                    + "; instance=" + receipt.value("instance", "unknown")
                    + "; proxy=" + receipt.value("proxy_delivery", "unknown")
                    + "; backend config=" + receipt.value("backend_proxy_config", "unknown")
                    + "; daemon=" + receipt.value("daemon_preparation", "skipped"));
            }
            return 0;
        }
        if (!interactiveEntry) throw Error("CLI_INVALID", "The interactive console is unavailable in this build.");
        return interactiveEntry(configPath, command.plain);
    } catch (const Error &error) {
        if (jsonOutput) printJson(Json{{"error", error.json()}});
        else std::cerr << error.code << ": " << error.message << std::endl;
        return 1;
    } catch (...) {
        const Error error("ENGINE_TASK_FAILED", "The operation failed.");
        if (jsonOutput) printJson(Json{{"error", error.json()}});
        else std::cerr << error.code << ": " << error.message << std::endl;
        return 1;
    }
}
