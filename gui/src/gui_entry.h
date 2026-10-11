#pragma once

#include <string>

namespace guard {

// Run the GUI role. The caller classified the process into this role; this
// adapter constructs exactly one QApplication from the Unicode command line
// (Qt reads the same wide line internally, so applicationFilePath() and
// arguments() keep their previous behavior). configPath is empty for the
// default Guard configuration, otherwise an absolute path forwarded to the
// bridge child so GUI and engine observe the same isolated configuration.
int runGui(const std::wstring &configPath, bool smokeTest);

} // namespace guard
