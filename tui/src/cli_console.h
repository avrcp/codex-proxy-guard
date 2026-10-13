#pragma once
#include <filesystem>
#include <iosfwd>
namespace cpg { class EngineSession; }
namespace cpg::tui {
int runPlainConsole(const std::filesystem::path &configPath);
// The same production line loop with injected streams and engine services.
int runPlainConsoleSession(EngineSession &session, std::istream &input, std::ostream &output);
} // namespace cpg::tui
