#pragma once

#include <string>
#include <vector>

namespace cpg {

// Execute the headless CLI/console/bridge/worker roles. The caller has already
// classified the process into this role; runCli owns argument parsing and all
// business entry points over the Unicode command line (excluding argv[0]).
int runCli(const std::vector<std::wstring> &arguments);

} // namespace cpg
