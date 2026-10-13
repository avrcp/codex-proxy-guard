#pragma once
#include "package_discovery.h"
#include <windows.h>
#include <chrono>
#include <functional>
#include <vector>

namespace cpg::detail {
using PackageFamilyQuery = std::function<LONG(UINT32 *, PWSTR *, UINT32 *, PWSTR, UINT32 *)>;
// Constructor/function injection only: the production command line cannot
// select a test API. Native enumeration and fixtures use this same bounded loop.
std::vector<std::wstring> packageFullNames(const Cancellation &cancel,
    const PackageFamilyQuery &query, std::chrono::steady_clock::time_point deadline);
}
