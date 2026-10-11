#pragma once

#include "cpg/json.h"

#include <filesystem>
#include <string>
#include <vector>

namespace cpg::tui {

// Entry point for the interactive role: renders the VT TUI over the shared
// engine session. Returns the process exit code. `plainOverride` forces the
// line-based fallback (also used automatically when VT output or an
// interactive console is unavailable).
int runTui(const std::filesystem::path &configPath, bool plainOverride);

} // namespace cpg::tui
