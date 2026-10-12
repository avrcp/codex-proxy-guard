#include "cli.h"
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
// bridge frame budget, so it never goes through the NDJSON protocol writer.
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
           "  --build-info        Print embedded build provenance and exit.\n"
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
           "  console                  Interactive line console (--plain for the plain variant)\n"
           "  internal-activate-package  Activation worker (internal)\n";
}

struct ParsedCli {
    std::wstring command;
    std::map<std::wstring, std::wstring> values;
    std::set<std::wstring> flags;
};

const wchar_t *wideValueOptions[] = {L"config", L"proxy-host", L"proxy-port"};
const wchar_t *flagOptions[] = {L"build-info", L"json", L"refresh-codex-daemon", L"activation-only", L"force", L"plain"};

[[noreturn]] void cliFail(const std::string &message)
{
    throw cpg::Error("CLI_INVALID", message);
}

} // namespace

void cpg::setInteractiveConsoleEntry(int (*entry)(const std::filesystem::path &, bool))
{
    interactiveEntry = entry;
}

int cpg::runCli(const std::vector<std::wstring> &arguments) {
    ParsedCli parsed;
    std::optional<std::wstring> pending;
    bool afterSeparator = false;
    try {
        for (const std::wstring &argument : arguments) {
            if (pending) {
                parsed.values[*pending] = argument;
                pending.reset();
                continue;
            }
            if (!afterSeparator && argument == L"--") { afterSeparator = true; continue; }
            if (!afterSeparator && argument.size() > 1 && argument[0] == L'-') {
                if (argument == L"-h" || argument == L"-?") {
                    std::cout << usageText();
                    return 0;
                }
                if (argument == L"-v") {
                    std::cout << "codex-proxy-guard " << productVersion() << std::endl;
                    return 0;
                }
                if (argument.size() > 1 && argument[0] == L'-' && argument[1] != L'-')
                    cliFail("Unknown option.");
                std::wstring name = argument.substr(2);
                std::optional<std::wstring> inlineValue;
                const std::size_t equal = name.find(L'=');
                if (equal != std::wstring::npos) {
                    inlineValue = name.substr(equal + 1);
                    name = name.substr(0, equal);
                }
                if (name == L"help") {
                    std::cout << usageText();
                    return 0;
                }
                if (name == L"version") {
                    std::cout << "codex-proxy-guard " << productVersion() << std::endl;
                    return 0;
                }
                bool matched = false;
                for (const wchar_t *candidate : wideValueOptions) {
                    if (name != candidate) continue;
                    if (inlineValue) parsed.values[name] = *inlineValue;
                    else pending = name;
                    matched = true;
                    break;
                }
                if (!matched) {
                    for (const wchar_t *candidate : flagOptions) {
                        if (name != candidate) continue;
                        if (inlineValue)
                            cliFail("Option '" + wideToUtf8(name) + "' does not take a value.");
                        parsed.flags.insert(name);
                        matched = true;
                        break;
                    }
                }
                if (!matched) cliFail("Unknown option.");
                continue;
            }
            if (!parsed.command.empty()) cliFail("Expected a single command.");
            parsed.command = argument;
        }
        if (pending) cliFail("The configuration option requires a path value.");
        const std::wstring &command = parsed.command;
        const auto flagSet = [&](const wchar_t *name) {
            return parsed.flags.count(name) > 0 || parsed.values.count(name) > 0;
        };
        const auto valueOf = [&](const wchar_t *name) -> std::wstring {
            const auto entry = parsed.values.find(name);
            return entry == parsed.values.end() ? std::wstring() : entry->second;
        };
        for (const wchar_t *flag : {L"json", L"refresh-codex-daemon", L"activation-only"})
            if (flagSet(flag) && command != L"launch") cliFail("Launch options require launch.");
        for (const wchar_t *flag : {L"force", L"proxy-host", L"proxy-port"})
            if (flagSet(flag) && command != L"init-config") cliFail("Configuration options require init-config.");
        // --plain selects the plain console variant of the interactive mode;
        // silently ignoring it on headless commands hides typos.
        if (flagSet(L"plain") && !command.empty() && command != L"console")
            cliFail("The plain flag is only valid without a command or with console.");
        if (parsed.values.count(L"config") && valueOf(L"config").empty())
            cliFail("The configuration path must not be empty.");
        // The worker and provenance query have no configuration dependency.
        // In particular, a worker must not resolve a user profile before reading
        // the parent's bounded activation request.
        if (command == L"internal-activate-package") {
            if (parsed.values.count(L"config")) cliFail("The activation worker does not accept configuration.");
            return activationWorker();
        }
        if (command == L"build-info" || flagSet(L"build-info")) {
            printJson(Json{{"version", productVersion()}, {"commit", buildCommit()},
                           {"dirty", buildDirty()}, {"language", "C++20"}, {"protocol_version", 1},
                           // Compatibility spellings for the former GUI --build-info output.
                           {"product_version", productVersion()}, {"git_commit", buildCommit()},
                           {"git_dirty", buildDirty()}});
            return 0;
        }
        if (command == L"licenses") return licenses();
        const std::filesystem::path configPath = parsed.values.count(L"config")
            ? std::filesystem::path(valueOf(L"config"))
            : Config::defaultPath();
        if (command == L"config-path") { print(wideToUtf8(configPath.wstring())); return 0; }
        if (command == L"init-config") {
            Config config;
            if (parsed.values.count(L"proxy-host")) config.host = wideToUtf8(valueOf(L"proxy-host"));
            if (parsed.values.count(L"proxy-port")) {
                int port = 0;
                try { port = std::stoi(wideToUtf8(valueOf(L"proxy-port"))); }
                catch (const std::exception &) { throw Error("CONFIG_INVALID", "Invalid proxy port."); }
                config.port = port;
            }
            initializeConfig(configPath, config, parsed.flags.count(L"force") > 0);
            print("Configuration saved.");
            return 0;
        }
        if (command == L"launch") {
            if (flagSet(L"refresh-codex-daemon") && flagSet(L"activation-only"))
                cliFail("Repair and activation-only cannot be combined.");
            const auto config = Config::loadOrCreate(configPath);
            printJson(launchPipeline(config, configPath,
                                     {flagSet(L"refresh-codex-daemon"), flagSet(L"activation-only")},
                                     Cancellation{}));
            return 0;
        }
        if (command.empty() || command == L"console") {
            if (!interactiveEntry)
                throw Error("CLI_INVALID", "The interactive console is unavailable in this build.");
            const bool plain = parsed.flags.count(L"plain") > 0;
            return interactiveEntry(configPath, plain);
        }
        cliFail("Unknown command.");
    } catch (const Error &e) {
        std::cerr << e.code << ": " << e.message << std::endl;
        return 1;
    } catch (...) {
        std::cerr << "ENGINE_TASK_FAILED: The operation failed." << std::endl;
        return 1;
    }
}
