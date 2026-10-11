#include "cli_console.h"
#include "cpg/build_info.h"
#include "core.h"
#include "launch.h"
#include "platform.h"
#include "session.h"
#include "cpg/text.h"

#include <atomic>
#include <cctype>
#include <exception>
#include <iostream>
#include <string>

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

namespace cpg::tui {
namespace {

std::atomic_bool consoleOperation{false};
Cancellation consoleCancellation;
BOOL WINAPI interruptConsole(DWORD signal)
{
    if ((signal == CTRL_C_EVENT || signal == CTRL_BREAK_EVENT) && consoleOperation.load()) {
        consoleCancellation.cancel();
        return TRUE;
    }
    return FALSE;
}

struct ConsoleOperation {
    ConsoleOperation()
    {
        consoleCancellation = Cancellation{};
        consoleOperation.store(true);
    }
    ~ConsoleOperation() { consoleOperation.store(false); }
};

void print(const std::string &text) { std::cout << text << std::endl; }
void printJson(const Json &object) { print(jsonDump(object)); }

std::string asciiTrimmed(std::string_view value)
{
    std::size_t begin = 0, end = value.size();
    while (begin < end && std::isspace(static_cast<unsigned char>(value[begin]))) ++begin;
    while (end > begin && std::isspace(static_cast<unsigned char>(value[end - 1]))) --end;
    return std::string(value.substr(begin, end - begin));
}

std::string toLower(std::string value)
{
    for (char &character : value)
        character = static_cast<char>(std::tolower(static_cast<unsigned char>(character)));
    return value;
}

std::string line(const std::string &prompt)
{
    print(prompt);
    std::string value;
    char character = 0;
    while (std::cin.get(character) && character != '\n') {
        if (value.size() >= 4096) throw Error("INPUT_INVALID", "Input exceeds its limit.");
        value.push_back(character);
    }
    if (!std::cin && value.empty()) throw Error("CANCELLED", "Input closed.");
    return asciiTrimmed(value);
}

int console(const std::filesystem::path &path)
{
    print(std::string("Codex Proxy Guard ") + productVersion() + " - local proxy launcher");
    for (;;) {
        Config config;
        bool ready = false;
        try {
            config = Config::loadOrCreate(path);
            ready = true;
        } catch (const Error &e) {
            print(e.code + ": " + e.message);
        }
        print("Proxy: " + config.proxyUrl() + " | backend block: "
              + (config.manageBackend ? "authorized" : "off"));
        const std::string action = toLower(line(
            "[L] Launch  [E] Edit proxy  [B] Backend authorization  [R] Repair launch  [S] Refresh  [Q] Quit"));
        if (action == "q") return 0;
        try {
            if (action == "s") {
                if (!ready) continue;
                const ConsoleOperation operation;
                const auto desktop = discoverDesktop(config, consoleCancellation);
                print(desktop.displayName + " | " + desktopProcessState(desktop).state + " | "
                      + (desktop.registered ? inspectProxyEnv(config) : "process environment"));
            } else if (action == "e") {
                const std::string host = line("Loopback HTTP/Mixed host:");
                const std::string portText = line("Port (1-65535):");
                int port = 0;
                try { port = std::stoi(portText); }
                catch (const std::exception &) { throw Error("CONFIG_INVALID", "Invalid proxy port."); }
                updateProxy(path, config, !ready, host, port);
            } else if (action == "b" && ready) {
                const std::filesystem::path home = consentHome(config);
                print((config.manageBackend
                           ? "Remove Guard's unchanged proxy block from: "
                           : "Authorize HTTP_PROXY/HTTPS_PROXY/NO_PROXY block for later Codex processes in: ")
                      + wideToUtf8(home.wstring()));
                if (line("Type YES to confirm this single operation:") == "YES")
                    updateConsent(path, config, !config.manageBackend, home);
            } else if ((action == "l" || action == "r") && ready) {
                const bool repair = action == "r";
                if (repair && line("Repair stops the shared Codex daemon once and may interrupt other clients' tasks. Type YES to confirm:") != "YES")
                    continue;
                const ConsoleOperation operation;
                printJson(launchPipeline(config, path, {repair, false}, consoleCancellation));
            }
        } catch (const Error &e) { print(e.code + ": " + e.message); }
    }
}

} // namespace

int runPlainConsole(const std::filesystem::path &configPath)
{
    SetConsoleCtrlHandler(interruptConsole, TRUE);
    return console(configPath);
}

} // namespace cpg::tui
