#pragma once
#include <filesystem>
#include <functional>
#include <optional>
#include <string>
#include <vector>

namespace cpg {
enum class CommandKind { Interactive, Launch, InitConfig, ConfigPath, BuildInfo, Licenses, Help, Version, InternalActivatePackage };
struct ParsedCommand {
    CommandKind kind = CommandKind::Interactive;
    std::optional<std::filesystem::path> configOverride;
    std::optional<std::string> proxyHost;
    std::optional<int> proxyPort;
    bool plain = false;
    bool json = false;
    bool repair = false;
    bool activationOnly = false;
    bool force = false;
};
ParsedCommand parseCommandLine(const std::vector<std::wstring> &arguments);
// The resolver is injectable by tests; production always uses Config::defaultPath.
std::filesystem::path resolveConfigPath(const ParsedCommand &command,
    const std::function<std::filesystem::path()> &defaultResolver);
}
