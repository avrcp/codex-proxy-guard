#pragma once

#include <filesystem>

namespace cpg::tui {

// Line-oriented interactive console: the fallback when VT output or an
// interactive console is unavailable, and the `--plain` spelling.
int runPlainConsole(const std::filesystem::path &configPath);

} // namespace cpg::tui
