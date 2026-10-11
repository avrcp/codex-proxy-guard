#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <shlobj.h>

#include <filesystem>
#include <optional>
#include <string>
#include <string_view>

namespace cpg {

// Known-folder lookup (never infers from environment strings it cannot
// verify). Throws Error("CONFIG_PATH_UNAVAILABLE", ...) on failure.
std::filesystem::path knownFolder(REFKNOWNFOLDERID id);

// Canonical comparison form for Windows paths: backslashes folded to '/',
// extended-length and UNC device prefixes unwrapped, '.'/'..' segments
// resolved, case-folded (Unicode-aware). Equal files on the same volume
// compare equal regardless of spelling.
std::wstring normalizedWindowsPath(std::wstring_view path);

// True only when `child` is inside `root` (or equals it) after
// normalization; the boundary respects full path segments.
bool pathWithin(const std::filesystem::path &root, const std::filesystem::path &child);

// Resolve through a held handle to the final path (symlinks/junctions
// resolved), stripping \\?\ prefixes. Returns nullopt when the object does
// not exist or cannot be opened. `directory` additionally requires the
// object to be a directory rather than a regular file.
std::optional<std::filesystem::path> canonicalPathOf(const std::filesystem::path &path,
                                                     bool directory);

} // namespace cpg
