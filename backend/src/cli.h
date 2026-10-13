#pragma once

#include <filesystem>
#include <string>
#include <vector>
#include <functional>
#include "cpg/cancellation.h"
#include "cpg/json.h"

namespace cpg {

// Parse the Unicode arguments once and dispatch the selected command.
int runCli(const std::vector<std::wstring> &arguments);
Json runConsoleOperation(const std::function<Json(const Cancellation &)> &operation);

// The interactive frontend (TUI with plain fallback) lives above the engine;
// the app entry point injects it so the engine never links the TUI layer.
void setInteractiveConsoleEntry(int (*entry)(const std::filesystem::path &configPath, bool plain));

} // namespace cpg
