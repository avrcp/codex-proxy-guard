#pragma once

#include "platform.h"

#include <chrono>
#include <string>

namespace cpg {

struct LaunchOptions { bool repair = false; bool activationOnly = false; };
struct ProxyPlan { std::string endpoint; std::string arguments; };
struct CodexCli { std::filesystem::path executable; std::filesystem::path home; };
struct LaunchFacts {
    std::string activationState = "not_submitted";
    std::string daemonPreparation = "skipped";
    std::string backendProxyConfig = "unknown";
};
ProxyPlan proxyPlan(const Config &config);
UnicodeEnvironment proxyEnvironment(const Config &config);
CodexCli resolveCodexCli(const Config &config);
std::string stopCodexDaemon(const CodexCli &cli, const Cancellation &cancel,
                            std::chrono::milliseconds budget = std::chrono::milliseconds(720000));
// The single authoritative pipeline for CLI, plain console and TUI frontends.
Json launchPipeline(const Config &config, const std::filesystem::path &configPath,
                    LaunchOptions options, const Cancellation &cancel, LaunchFacts *facts = nullptr);

} // namespace cpg
