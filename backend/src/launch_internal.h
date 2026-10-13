#pragma once

#include "launch.h"

#include <string>

namespace cpg::detail {

// Fixture seams use the same orchestration as production, without launching real
// Desktop processes, touching a user's Home, or interrupting a shared daemon.
std::string daemonStopStatus(const std::string &output);
Json activationReceipt(const Json &worker);
CodexCli resolveCliFrom(const std::filesystem::path &explicitHome,
                        const std::filesystem::path &userHome,
                        const std::string &path, const std::filesystem::path &overridePath,
                        const std::filesystem::path &cwd);
struct LaunchServices {
    std::function<void()> requireElevation;
    std::function<Desktop()> discover;
    std::function<ProcessState(const Desktop &)> process;
    std::function<void()> prepare;
    std::function<CodexCli()> resolve;
    std::function<std::string(const CodexCli &)> stop;
    std::function<Json(const Desktop &, const std::string &)> activate;
    std::function<Json(const Desktop &, const UnicodeEnvironment &)> native;
    std::function<void()> markSubmitted;
};
// The caller owns both launch/configuration leases for the whole call.
Json launchWith(const Config &config, LaunchOptions options,
                const Cancellation &cancel, const LaunchServices &services, LaunchFacts *facts = nullptr);

} // namespace cpg::detail
