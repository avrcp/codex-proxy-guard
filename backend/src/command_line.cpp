#include "command_line.h"
#include "cpg/error.h"
#include "cpg/port.h"
#include "cpg/text.h"
#include <map>
#include <set>

namespace cpg {
namespace {
[[noreturn]] void invalid(const char *message) { throw Error("CLI_INVALID", message); }
}
ParsedCommand parseCommandLine(const std::vector<std::wstring> &arguments)
{
    ParsedCommand parsed;
    std::map<std::wstring, std::wstring> values;
    std::set<std::wstring> flags;
    std::optional<std::wstring> command;
    bool options = true;
    for (std::size_t index = 0; index < arguments.size(); ++index) {
        const auto &argument = arguments[index];
        if (options && argument == L"--") { options = false; continue; }
        if (options && !argument.empty() && argument.front() == L'-') {
            std::wstring name;
            if (argument == L"-h" || argument == L"-?") name = L"help";
            else if (argument == L"-v") name = L"version";
            else if (argument.starts_with(L"--")) name = argument.substr(2);
            else invalid("Unknown option.");
            std::optional<std::wstring> value;
            if (const auto equal = name.find(L'='); equal != std::wstring::npos) {
                value = name.substr(equal + 1);
                name.resize(equal);
            }
            if (name == L"config" || name == L"proxy-host" || name == L"proxy-port") {
                if (values.contains(name)) invalid("A value option may be specified only once.");
                if (!value) {
                    if (++index == arguments.size()) invalid("The option requires a value.");
                    value = arguments[index];
                    if (!value->empty() && value->front() == L'-')
                        invalid("The option requires a value; use --name=value for a value beginning with '-'.");
                }
                if (value->empty()) invalid("Option values must not be empty.");
                values.emplace(name, *value);
            } else if (name == L"plain" || name == L"json" || name == L"refresh-codex-daemon"
                || name == L"activation-only" || name == L"force" || name == L"help" || name == L"version") {
                if (value) invalid("The flag does not accept a value.");
                if (!flags.insert(name).second) invalid("A flag may be specified only once.");
            } else invalid("Unknown option.");
        } else {
            if (command) invalid("Expected one command.");
            command = argument;
        }
    }
    if (command) {
        if (*command == L"console") parsed.kind = CommandKind::Interactive;
        else if (*command == L"launch") parsed.kind = CommandKind::Launch;
        else if (*command == L"init-config") parsed.kind = CommandKind::InitConfig;
        else if (*command == L"config-path") parsed.kind = CommandKind::ConfigPath;
        else if (*command == L"build-info") parsed.kind = CommandKind::BuildInfo;
        else if (*command == L"licenses") parsed.kind = CommandKind::Licenses;
        else if (*command == L"internal-activate-package") parsed.kind = CommandKind::InternalActivatePackage;
        else invalid("Unknown command.");
    }
    if (flags.contains(L"help") || flags.contains(L"version")) {
        if (command || flags.size() != 1 || !values.empty()) invalid("Information flags cannot be combined with other operations.");
        parsed.kind = flags.contains(L"help") ? CommandKind::Help : CommandKind::Version;
    }
    parsed.plain = flags.contains(L"plain");
    parsed.json = flags.contains(L"json");
    parsed.repair = flags.contains(L"refresh-codex-daemon");
    parsed.activationOnly = flags.contains(L"activation-only");
    parsed.force = flags.contains(L"force");
    if (parsed.plain && parsed.kind != CommandKind::Interactive) invalid("The plain flag requires interactive mode.");
    if ((parsed.json || parsed.repair || parsed.activationOnly) && parsed.kind != CommandKind::Launch) invalid("Launch options require launch.");
    if (parsed.repair && parsed.activationOnly) invalid("Repair and activation-only cannot be combined.");
    if ((parsed.force || values.contains(L"proxy-host") || values.contains(L"proxy-port"))
        && parsed.kind != CommandKind::InitConfig) invalid("Configuration options require init-config.");
    if (values.contains(L"config")) parsed.configOverride = std::filesystem::path(values.at(L"config"));
    if (parsed.kind == CommandKind::InternalActivatePackage && parsed.configOverride) invalid("The worker does not accept configuration.");
    if (values.contains(L"proxy-host")) parsed.proxyHost = wideToUtf8(values.at(L"proxy-host"));
    if (values.contains(L"proxy-port")) {
        try { parsed.proxyPort = parsePortStrict(wideToUtf8(values.at(L"proxy-port"))); }
        catch (const Error &) { invalid("Proxy port must be ASCII digits in the range 1-65535."); }
    }
    return parsed;
}

std::filesystem::path resolveConfigPath(const ParsedCommand &command,
    const std::function<std::filesystem::path()> &defaultResolver)
{
    const auto path = command.configOverride ? *command.configOverride : defaultResolver();
    if (path.empty()) invalid("The configuration path must not be empty.");
    std::error_code error;
    const auto absolute = std::filesystem::absolute(path, error);
    if (error || !absolute.is_absolute()) invalid("Could not resolve the configuration path.");
    return absolute.lexically_normal();
}
}
