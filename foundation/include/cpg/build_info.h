#pragma once

#include <string>

namespace cpg {

// Embedded build provenance (compile definitions from CMake; the single
// source of truth for the `build-info` role and release manifests).
const char *productVersion();
const char *buildCommit();
bool buildDirty();

} // namespace cpg
