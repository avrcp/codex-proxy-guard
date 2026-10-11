#pragma once

#include "core.h"

namespace cpg {

// Bounded, read-only native package discovery. Builds the schema-1 discovery
// envelope (the same record shape the PowerShell baseline produced) purely
// through Win32/AppModel APIs: FindPackagesByPackageFamily,
// PackageIdFromFullName, GetPackagePathByFullName and the Appx manifest
// reader. No powershell.exe/pwsh.exe child, no fallback to it.
Json discoverPackagesNative(const Cancellation &cancel);

} // namespace cpg
