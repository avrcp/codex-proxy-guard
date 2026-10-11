#pragma once

#include "core.h"
#include "cpg/json.h"
#include "cpg/process.h"

#include <chrono>
#include <filesystem>
#include <string>
#include <vector>

namespace cpg {

struct Desktop {
    std::string product = "chat_gpt";
    std::string displayName = "ChatGPT Desktop";
    std::string packageName, packageVersion, architecture, discoverySource;
    bool registered = false;
    std::filesystem::path executable, installLocation;
    std::string packageFullName, packageFamilyName;
    std::string applicationId, aumid, manifestExecutable, runtimeKind;
    Json publicJson() const;
};
struct ProcessState {
    std::string state = "unknown";
    std::uint32_t pid = 0;
};
struct HelperResult {
    std::string out, err;
    int exitCode = -1;
    bool submitted = false;
};
// Only owns/kills its direct short-lived helper, never a process tree.
HelperResult runHelper(const std::filesystem::path &program,
                       const std::vector<std::wstring> &arguments,
                       std::string_view input, const Cancellation &cancel,
                       std::chrono::milliseconds timeout, std::size_t outputLimit,
                       const UnicodeEnvironment &environment = UnicodeEnvironment::current(),
                       bool activationSubmission = false);
std::string elevation(); // not_elevated/elevated/unknown; never treat query failure as safe
void requireNonElevated();
Desktop discoverDesktop(const Config &config, const Cancellation &cancel);
// Exposed for bounded fixture tests; validates schema and registered path containment.
Desktop parseDiscovery(const std::string &json,
                       const std::filesystem::path &overrideExecutable = {});
ProcessState desktopProcessState(const Desktop &desktop);
Json activateDesktop(const Desktop &desktop, const std::string &arguments,
                     const Cancellation &cancel);
int activationWorker();

} // namespace cpg
