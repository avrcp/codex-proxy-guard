<#
.SYNOPSIS
Builds and verifies the Windows x64 Qt GUI portable split release.
.DESCRIPTION
Requires MSVC x64, Visual Studio CMake/Ninja, Python 3 and py7zr. The complete
pinned Qt SDK is extracted from its verified official archive for each build and
removed afterwards; only the archive cache under target/qt-source persists.

The release is split: a runtime ZIP users run and a source-compliance ZIP
(application source snapshot, upstream Qt source, licenses, recipes and rebuild
instructions) shipped beside it. Both are bound together by release-manifest.json
and re-verified from scratch by scripts/verify-package.py. The full profile
refuses a dirty working tree; use scripts/test-cpp.ps1 for development builds.
#>
param([string] $Version = '0.6.0-rc.1')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Output = Join-Path $Root 'dist\gui'
$Build = Join-Path $Root 'target\gui-release'
$QtVersion = '6.8.3'
$SourceName = "qtbase-everywhere-src-$QtVersion.tar.xz"
$SourceUrl = "https://download.qt.io/archive/qt/6.8/$QtVersion/submodules/$SourceName"
$SourceHash = '56001b905601bb9023d399f3ba780d7fa940f3e4861e496a7c490331f49e0b80'
$QtRoot = Join-Path $Root ('target\qt-sdk-' + [Guid]::NewGuid().ToString('N'))
$SdkManifestPath = Join-Path $Root 'licenses\Qt\sdk-6.8.3-msvc2022-x64.json'
$SdkManifest = Get-Content -LiteralPath $SdkManifestPath -Raw | ConvertFrom-Json
if ($SdkManifest.schema_version -ne 1 -or $SdkManifest.qt_version -ne $QtVersion -or $SdkManifest.source_sha256 -ne $SourceHash) { throw 'Qt binary/source manifest mismatch.' }
$RuntimeZip = Join-Path $Root "dist\CodexProxyGuard-$Version-windows-x86_64-dynamic.zip"
$ComplianceZip = Join-Path $Root "dist\CodexProxyGuard-$Version-source-compliance.zip"
$ReleaseManifest = Join-Path $Root 'dist\release-manifest.json'

function Invoke-Checked([string] $File, [string[]] $Arguments) {
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$File failed with exit code $LASTEXITCODE" }
}

function Get-Hash([string] $Path) {
    # Avoid module auto-loading differences after the MSVC environment import.
    $Stream = [IO.File]::OpenRead($Path)
    $Hasher = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($Hasher.ComputeHash($Stream))).Replace('-', '').ToLowerInvariant() }
    finally { $Hasher.Dispose(); $Stream.Dispose() }
}

function Write-Utf8([string] $Path, [string] $Text) {
    [IO.File]::WriteAllText($Path, $Text + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
}

function Get-SourceFingerprint {
    $Tracked = @(& git -c core.quotepath=false ls-files --cached --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot enumerate source files.' }
    $Entries = foreach ($Name in ($Tracked | Sort-Object -Unique)) {
        $Path = Join-Path $Root $Name
        if (Test-Path -LiteralPath $Path -PathType Leaf) { "$Name $(Get-Hash $Path)" } else { "$Name deleted" }
    }
    return ($Entries -join "`n")
}

function Get-ToolchainRecord {
    # VsDevCmd exports VCToolsVersion/WindowsSDKVersion into this process; fall
    # back to the pinned default file when a toolchain omits the variable.
    $Msvc = "${Env:VCToolsVersion}"
    if ([string]::IsNullOrWhiteSpace($Msvc)) {
        $Msvc = (Get-Content -LiteralPath (Join-Path $Vs 'VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt') -Raw).Trim()
    }
    return @{
        msvc = "MSVC $Msvc"; windows_sdk = "$Env:WindowsSDKVersion"
        cmake = (& $CMake --version | Select-Object -First 1); ninja = (& $Ninja --version)
        python = (& $Python.Source --version); qt_version = $QtVersion
    }
}

function Assert-OfficialQtSdk {
    foreach ($Entry in $SdkManifest.files_sha256.PSObject.Properties) {
        $SdkFile = Join-Path $QtRoot $Entry.Name
        if (-not (Test-Path -LiteralPath $SdkFile -PathType Leaf) -or (Get-Hash $SdkFile) -ne $Entry.Value) {
            throw "Qt SDK differs from the verified official archive: $($Entry.Name). A modified SDK requires its corresponding source and a reviewed release manifest."
        }
    }
}

# Read GUI output through a bounded pipe even though it is a Windows subsystem EXE.
function Invoke-Smoke([string] $File, [string] $Arguments, [string] $InputData = '') {
    $Start = New-Object Diagnostics.ProcessStartInfo
    $Start.FileName = $File
    $Start.Arguments = $Arguments
    $Start.WorkingDirectory = Split-Path -Parent $File
    $Start.UseShellExecute = $false
    $Start.CreateNoWindow = $true
    $Start.RedirectStandardOutput = $true
    $Start.RedirectStandardError = $true
    $Start.RedirectStandardInput = $true
    $Start.StandardOutputEncoding = New-Object Text.UTF8Encoding($false, $true)
    $Start.StandardErrorEncoding = New-Object Text.UTF8Encoding($false, $true)
    $Process = New-Object Diagnostics.Process
    $Process.StartInfo = $Start
    $Started = $false
    try {
        if (-not $Process.Start()) { throw 'Cannot start package smoke test' }
        $Started = $true
        # Chunked reads cap memory even if a broken child continuously floods a pipe.
        $Readers = @($Process.StandardOutput, $Process.StandardError)
        $Buffers = @((New-Object char[] 1024), (New-Object char[] 1024))
        $Texts = @((New-Object Text.StringBuilder), (New-Object Text.StringBuilder))
        $Reads = @($Readers[0].ReadAsync($Buffers[0], 0, 1024), $Readers[1].ReadAsync($Buffers[1], 0, 1024))
        $Done = @($false, $false)
        if ($InputData.Length -gt 4096) { throw 'Smoke request exceeds pipe input budget.' }
        if ($InputData) {
            $Process.StandardInput.Write($InputData)
            $Process.StandardInput.Flush()
        }
        # Keep stdin open: explicit shutdown must finish without waiting for EOF.
        $Watch = [Diagnostics.Stopwatch]::StartNew()
        while (-not ($Process.HasExited -and $Done[0] -and $Done[1])) {
            if ($Watch.ElapsedMilliseconds -gt 30000) { throw "Package smoke test timed out: $Arguments" }
            for ($Index = 0; $Index -lt 2; $Index++) {
                if (-not $Done[$Index] -and $Reads[$Index].IsCompleted) {
                    $Count = $Reads[$Index].GetAwaiter().GetResult()
                    if ($Count -eq 0) { $Done[$Index] = $true; continue }
                    if ($Texts[$Index].Length + $Count -gt 65536) { throw 'Oversized smoke output' }
                    $null = $Texts[$Index].Append($Buffers[$Index], 0, $Count)
                    $Reads[$Index] = $Readers[$Index].ReadAsync($Buffers[$Index], 0, 1024)
                }
            }
            Start-Sleep -Milliseconds 10
        }
        if ($Process.ExitCode -ne 0) { throw "Package smoke test failed: $Arguments ($($Process.ExitCode)): $($Texts[1])" }
        return $Texts[0].ToString().Trim()
    } finally {
        if ($Started -and -not $Process.HasExited) {
            $Process.Kill()
            if (-not $Process.WaitForExit(5000)) { Write-Warning 'Smoke child did not exit after termination.' }
        }
        $Process.Dispose()
    }
}

function Invoke-BridgeSmoke([string] $File, [string] $Version, [string] $Commit) {
    $SmokeDirectory = Join-Path $Root ('target\gui-smoke-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $SmokeDirectory -Force | Out-Null
    try {
        $SmokeConfig = Join-Path $SmokeDirectory 'guard.toml'
        Write-Utf8 $SmokeConfig "[proxy]`nhost = `"127.0.0.1`"`nport = 10808"
        $ConfigHash = Get-Hash $SmokeConfig
        $Frames = '{"schema":1,"id":1,"method":"hello","params":{}}' + "`n" + '{"schema":1,"id":2,"method":"shutdown","params":{}}' + "`n"
        $BridgeOutput = Invoke-Smoke $File "--config `"$SmokeConfig`" bridge" $Frames
        $Lines = @($BridgeOutput -split '\r?\n')
        if ($Lines.Count -ne 2) { throw 'Bridge smoke expected exactly hello and shutdown responses.' }
        $Hello = $Lines[0] | ConvertFrom-Json
        $Shutdown = $Lines[1] | ConvertFrom-Json
        if ($Hello.schema -ne 1 -or $Hello.id -ne 1 -or $Hello.ok -ne $true -or $Hello.result.protocol_version -ne 1 -or $Hello.result.engine_version -ne $Version -or $Hello.result.engine_commit -ne $Commit) { throw 'Packaged bridge hello/provenance mismatch.' }
        if ($Shutdown.schema -ne 1 -or $Shutdown.id -ne 2 -or $Shutdown.ok -ne $true -or $Shutdown.result.shutting_down -ne $true) { throw 'Packaged bridge shutdown failed.' }
        if ((Get-Hash $SmokeConfig) -ne $ConfigHash -or @(Get-ChildItem -LiteralPath $SmokeDirectory -File).Count -ne 1) { throw 'Read-only bridge smoke unexpectedly modified its isolated configuration.' }
        return 'hello/shutdown passed; stdin held open; isolated config unchanged'
    } finally {
        Remove-Item -LiteralPath $SmokeDirectory -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Compress-Directory([string] $Directory, [string] $ZipPath) {
    if (Test-Path -LiteralPath $ZipPath) { Remove-Item -LiteralPath $ZipPath -Force }
    Compress-Archive -Path "$Directory\*" -DestinationPath $ZipPath -CompressionLevel Optimal
}

# Build the source-compliance side asset in an isolated staging directory.
function New-SourceCompliance([string] $Commit) {
    $Stage = Join-Path $Root ('target\source-compliance-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path "$Stage\app", "$Stage\upstream", "$Stage\patches", "$Stage\recipes", "$Stage\licenses" -Force | Out-Null
    try {
        # Snapshot the exact clean commit; this includes vendored toml++, CMake,
        # resources, generators, build and verification scripts.
        $AppArchive = Join-Path $Stage 'app.zip'
        Invoke-Checked git @('-C', $Root, 'archive', '--format=zip', '-o', $AppArchive, $Commit)
        Expand-Archive -LiteralPath $AppArchive -DestinationPath "$Stage\app"
        Remove-Item -LiteralPath $AppArchive -Force

        $SourceCache = Join-Path $Root "target\qt-source\$SourceName"
        New-Item -ItemType Directory -Path (Split-Path $SourceCache) -Force | Out-Null
        if (-not (Test-Path -LiteralPath $SourceCache)) {
            Write-Host "Downloading corresponding Qt source: $SourceUrl"
            Invoke-WebRequest -UseBasicParsing -Uri $SourceUrl -OutFile $SourceCache -TimeoutSec 900
        }
        if ((Get-Hash $SourceCache) -ne $SourceHash) { throw 'Qt source archive SHA-256 mismatch; remove the cached archive and retry.' }
        Copy-Item -LiteralPath $SourceCache -Destination "$Stage\upstream"

        Write-Utf8 "$Stage\patches\README.txt" "No Qt patches are applied. The pinned upstream qtbase $QtVersion release tarball (SHA-256 $SourceHash) is built and deployed unmodified from the official binary SDK archive."
        Write-Utf8 "$Stage\recipes\toolchain.json" ((Get-ToolchainRecord) | ConvertTo-Json)
        Write-Utf8 "$Stage\recipes\packaging.json" (@{
                entry_point = 'scripts/build-portable.ps1'
                qt_sdk_archive_url = $SdkManifest.archive_url
                qt_sdk_archive_sha256 = $SdkManifest.archive_sha256
                qt_sdk_manifest = 'licenses/Qt/sdk-6.8.3-msvc2022-x64.json'
                qt_source_url = $SourceUrl; qt_source_sha256 = $SourceHash
            } | ConvertTo-Json)
        Copy-Item -LiteralPath (Join-Path $Root 'LICENSE'), (Join-Path $Root 'THIRD_PARTY_NOTICES.md') -Destination "$Stage\licenses"
        New-Item -ItemType Directory -Path "$Stage\licenses\Qt" -Force | Out-Null
        Get-ChildItem -LiteralPath (Join-Path $Root 'licenses\Qt') | Copy-Item -Destination "$Stage\licenses\Qt" -Recurse
        New-Item -ItemType Directory -Path "$Stage\licenses\tomlplusplus" -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $Root 'backend\third_party\toml++\LICENSE') -Destination "$Stage\licenses\tomlplusplus"

        Write-Utf8 "$Stage\REBUILD.md" @"
Rebuilding Codex ProxyGuard $Version from this compliance package
=================================================================

Prerequisites (not redistributed here; obtain from their official sources):

- Visual Studio 2022 or newer with the MSVC x64 C++ toolchain and the bundled
  CMake + Ninja (versions recorded in recipes/toolchain.json).
- Windows 10/11 SDK (version recorded in recipes/toolchain.json).
- Python 3 with py7zr (``python -m pip install --user py7zr``).
- The official Qt $QtVersion MSVC 2022 x64 binary SDK archive referenced by
  recipes/packaging.json (its URL and SHA-256 are pinned there).

Steps:

1. Extract ``app/`` to an empty directory and open it.
2. Verify/extract the official Qt SDK and build the portable package:
   ``powershell -File scripts\build-portable.ps1 -Version $Version``
   The script downloads the pinned Qt SDK archive itself (or reuses the cache
   under target\qt-source), verifies every file hash against
   licenses/Qt/sdk-6.8.3-msvc2022-x64.json, builds, tests, packages and
   re-verifies the split release.
3. Run the C++ test suites alone with ``powershell -File scripts\test-cpp.ps1``
   (pass your Qt SDK root via -QtRoot).

Replacing or rebuilding Qt from source:

- The corresponding Qt source archive is provided in upstream/ with its
  SHA-256; Qt's own build instructions apply. This dynamic build deploys the
  Qt shared libraries recorded in the release manifest, so a rebuilt Qt of the
  same version can replace them per THIRD_PARTY_NOTICES.md. Verify any such
  replacement with scripts/verify-package.py adjusted to the new provenance.
- app/ is the complete application source of commit $Commit; no generated or
  hidden inputs are required beyond the prerequisites above.
"@
        Write-Utf8 "$Stage\SOURCE_REVISION.txt" "CodexProxyGuard application source commit: $Commit`nQt source: qtbase $QtVersion official release tarball`nQt source SHA-256: $SourceHash"
        Write-Utf8 "$Stage\SOURCE_ACCESS.txt" @"
Corresponding source availability for this release:

- CodexProxyGuard-$Version-source-compliance.zip (this archive) contains the
  complete application source snapshot, the upstream Qt source archive,
  licenses, recipes and rebuild instructions.
- It is distributed at the same location as the runtime archive
  CodexProxyGuard-$Version-windows-x86_64-dynamic.zip (same release channel,
  no additional registration or fee is required to obtain it).
- The Qt source archive is also available upstream at:
  $SourceUrl
- Verification: SHA-256 values for every file are recorded in manifest.json
  inside this archive and in release-manifest.json shipped beside the
  runtime ZIP.

publication_pending: this text records how corresponding source is provided.
Whether a specific distribution channel has actually published both archives
is tracked by the release checklist, not by this file.
"@
        $ComplianceFiles = [ordered]@{}
        foreach ($File in (Get-ChildItem -LiteralPath $Stage -Recurse -File | Sort-Object FullName)) {
            $Relative = $File.FullName.Substring($Stage.Length + 1).Replace('\', '/')
            $ComplianceFiles[$Relative] = Get-Hash $File.FullName
        }
        Write-Utf8 "$Stage\manifest.json" (@{
                schema_version = 1; product_version = $Version; source_commit = $Commit
                qt_source = @{ name = $SourceName; sha256 = $SourceHash; url = $SourceUrl }
                files_sha256 = $ComplianceFiles
            } | ConvertTo-Json -Depth 4)
        # manifest.json hashes every other member; regenerate its own exclusion
        # by listing files first (manifest.json itself is verified via the
        # release manifest after compression).
        Compress-Directory $Stage $ComplianceZip
        return @{
            zip     = $ComplianceZip
            sha256  = (Get-Hash $ComplianceZip)
            bytes   = (Get-Item -LiteralPath $ComplianceZip).Length
            members = $ComplianceFiles.Count + 1
        }
    } finally {
        Remove-Item -LiteralPath $Stage -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Push-Location $Root
$OriginalPath = $Env:PATH
$OriginalQtPluginPath = $Env:QT_PLUGIN_PATH
try {
    if ((Get-Hash (Join-Path $Root 'backend\third_party\toml++\toml.hpp')) -ne '6b5172ad4dd6519aec67b919181fa7a38a2234131e5b2afa232dfe444819783e') {
        throw 'Vendored toml++ differs from the reviewed upstream release.'
    }
    # Keep the script and both CMake default caches on one product version.
    foreach ($CmakeLists in @('CMakeLists.txt', 'gui\CMakeLists.txt')) {
        $DefaultVersion = [regex]::Match((Get-Content -LiteralPath (Join-Path $Root $CmakeLists) -Raw), 'CPG_PRODUCT_VERSION "([^"]+)"').Groups[1].Value
        if ($DefaultVersion -ne $Version) { throw "Product version drift: $CmakeLists default '$DefaultVersion' does not match '$Version'." }
    }
    # These hashes come from the checksum-verified official SDK archive, not
    # from this machine's installation or its self-reported qmake version.
    & (Join-Path $PSScriptRoot 'verify-qt-sdk.ps1') -SdkOutputPath $QtRoot
    Assert-OfficialQtSdk
    $VsWhere = "${Env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $Vs = (& $VsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $Vs) { throw 'MSVC x64 C++ build tools are required.' }
    # Import only this process's developer environment; do not alter machine/user settings.
    $VsDevCmd = Join-Path $Vs 'Common7\Tools\VsDevCmd.bat'
    $DevEnvironment = & $Env:ComSpec /d /c "call `"$VsDevCmd`" -no_logo -arch=x64 -host_arch=x64 >nul && set"
    if ($LASTEXITCODE -ne 0) { throw 'Cannot initialize the MSVC x64 environment.' }
    foreach ($Line in $DevEnvironment) {
        if ($Line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process') }
    }
    $CMake = Join-Path $Vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
    $CTest = Join-Path (Split-Path $CMake) 'ctest.exe'
    $Ninja = Join-Path $Vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe'
    foreach ($Tool in @($CMake, $CTest, $Ninja)) { if (-not (Test-Path -LiteralPath $Tool)) { throw "Required build tool missing: $Tool" } }
    $Python = Get-Command python.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $Python) { throw 'Python 3 (with py7zr) is required for SDK verification and the final package check.' }
    $Env:PATH = "$QtRoot\bin;$(Split-Path $Ninja);$Env:PATH"
    $Env:QT_PLUGIN_PATH = Join-Path $QtRoot 'plugins'

    # Build both C++ executables from the same source snapshot and toolchain.
    $SourceBefore = Get-SourceFingerprint
    $Commit = (& git rev-parse HEAD | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $Commit) { throw 'Cannot read source commit.' }
    $StatusBefore = (@(& git status --porcelain=v1 --untracked-files=all) -join "`n")
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read source status.' }
    $Dirty = [bool]($StatusBefore.Length -gt 0)
    if ($Dirty) { throw 'The split release requires a clean working tree; commit or stash first. Development builds use scripts/test-cpp.ps1.' }

    Invoke-Checked $CMake @('--fresh', '-S', $Root, '-B', $Build, '-G', 'Ninja', "-DCMAKE_MAKE_PROGRAM=$Ninja", '-DCMAKE_BUILD_TYPE=Release', "-DCMAKE_PREFIX_PATH=$QtRoot", "-DQt6_DIR=$QtRoot/lib/cmake/Qt6", '-DBUILD_TESTING=ON', "-DCPG_PRODUCT_VERSION=$Version", "-DCPG_BUILD_COMMIT=$Commit", "-DCPG_BUILD_DIRTY=false")
    Invoke-Checked $CMake @('--build', $Build, '--config', 'Release', '--parallel')
    Invoke-Checked $CTest @('--test-dir', $Build, '-C', 'Release', '--output-on-failure', '--no-tests=error')

    # This fixed output is the only recursive removal target; verify before deleting.
    $ExpectedOutput = [IO.Path]::GetFullPath((Join-Path $Root 'dist\gui'))
    if ($Output -ne $ExpectedOutput -or -not $Output.StartsWith($Root + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe package directory.' }
    if (Test-Path -LiteralPath $Output) { Remove-Item -LiteralPath $Output -Recurse -Force }
    New-Item -ItemType Directory -Path "$Output\engine" -Force | Out-Null
    Copy-Item -LiteralPath "$Build\gui\CodexProxyGuard.exe" -Destination $Output
    Copy-Item -LiteralPath "$Build\backend\codex-proxy-guard.exe" -Destination "$Output\engine\codex-proxy-guard.exe"
    $EngineHash = Get-Hash "$Output\engine\codex-proxy-guard.exe"
    Invoke-Checked "$QtRoot\bin\windeployqt.exe" @('--release', '--compiler-runtime', '--no-patchqt', '--qtpaths', "$QtRoot\bin\qtpaths.exe", '--skip-plugin-types', 'generic,networkinformation,tls', '--no-translations', '--no-opengl-sw', '--no-system-d3d-compiler', '--no-system-dxc-compiler', '--dir', $Output, "$Output\CodexProxyGuard.exe")
    # Keep every Qt DLL byte-identical to the verified SDK. qt.conf supplies
    # relative deployment paths instead of windeployqt patching Qt6Core.dll.
    Write-Utf8 "$Output\qt.conf" "[Paths]`nPrefix=.`nPlugins=."
    # The worker lives beside the engine, so it needs its own Qt Core runtime.
    Copy-Item -LiteralPath "$QtRoot\bin\Qt6Core.dll" -Destination "$Output\engine"
    Write-Utf8 "$Output\engine\qt.conf" "[Paths]`nPrefix=.`nPlugins=."
    Assert-OfficialQtSdk
    $DeployedQt = [ordered]@{}
    foreach ($Dll in (Get-ChildItem -LiteralPath $Output -Recurse -File -Filter '*.dll')) {
        $Relative = $Dll.FullName.Substring($Output.Length + 1).Replace('\', '/')
        $SourceRelative = if ($Relative -eq 'engine/Qt6Core.dll') { 'bin/Qt6Core.dll' } elseif ($Relative -notmatch '/') { "bin/$Relative" } else { "plugins/$Relative" }
        $Entry = $SdkManifest.files_sha256.PSObject.Properties[$SourceRelative]
        if ($null -eq $Entry) {
            # Compiler runtime is checked/copied separately below. Any Qt DLL
            # or plugin outside the pinned QtBase archive is a packaging error.
            if ($Relative -match '^Qt' -or $Relative -match '/') { throw "Unrecognized Qt deployment dependency: $Relative" }
            continue
        }
        $DeployedHash = Get-Hash $Dll.FullName
        if ($DeployedHash -ne $Entry.Value) { throw "Deployed Qt file differs from official SDK: $Relative" }
        $DeployedQt[$Relative] = @{ sdk_path = $SourceRelative; sdk_sha256 = $Entry.Value; deployed_sha256 = $DeployedHash }
    }
    # windeployqt can emit only the VC installer; portable users also need app-local CRT DLLs.
    $RedistVersion = (Get-Content -LiteralPath "$Vs\VC\Auxiliary\Build\Microsoft.VCRedistVersion.default.txt" -Raw).Trim()
    $CrtDirs = @(Get-ChildItem -LiteralPath "$Vs\VC\Redist\MSVC\$RedistVersion\x64" -Directory | Where-Object Name -match '^Microsoft\.VC\d+\.CRT$')
    if ($CrtDirs.Count -ne 1) { throw 'Cannot unambiguously locate the app-local x64 VC runtime.' }
    Get-ChildItem -LiteralPath $CrtDirs[0].FullName -Filter '*.dll' | Copy-Item -Destination $Output
    # The C++ engine/activation worker starts from engine/, so the parent's
    # application-local runtime is not on its DLL search path on a clean PC.
    Get-ChildItem -LiteralPath $CrtDirs[0].FullName -Filter '*.dll' | Copy-Item -Destination "$Output\engine"
    Copy-Item -LiteralPath 'LICENSE', 'THIRD_PARTY_NOTICES.md' -Destination $Output
    Copy-Item -LiteralPath 'licenses' -Destination $Output -Recurse
    New-Item -ItemType Directory -Path "$Output\licenses\tomlplusplus" -Force | Out-Null
    Copy-Item -LiteralPath 'backend\third_party\toml++\LICENSE' -Destination "$Output\licenses\tomlplusplus"

    foreach ($Required in @('Qt6Core.dll', 'Qt6Gui.dll', 'Qt6Widgets.dll', 'platforms\qwindows.dll', 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'engine\Qt6Core.dll', 'engine\msvcp140.dll', 'engine\vcruntime140.dll')) {
        if (-not (Test-Path -LiteralPath "$Output\$Required" -PathType Leaf)) { throw "Incomplete deployment: $Required" }
    }
    # Verify DLL loading without SDK paths or plugin overrides.
    $Env:PATH = "$Env:SystemRoot\System32;$Env:SystemRoot"
    Remove-Item Env:QT_PLUGIN_PATH -ErrorAction SilentlyContinue
    $GuiSmoke = Invoke-Smoke "$Output\CodexProxyGuard.exe" '--smoke-test'
    $GuiEmbedded = (Invoke-Smoke "$Output\CodexProxyGuard.exe" '--build-info') | ConvertFrom-Json
    if ($GuiEmbedded.git_commit -ne $Commit -or [bool]$GuiEmbedded.git_dirty -ne $false -or $GuiEmbedded.product_version -ne $Version -or $GuiEmbedded.qt_version -ne $QtVersion -or $GuiEmbedded.protocol_version -ne 1) { throw 'GUI embedded provenance mismatch.' }
    $EngineEmbedded = (Invoke-Smoke "$Output\engine\codex-proxy-guard.exe" 'build-info') | ConvertFrom-Json
    if ($EngineEmbedded.commit -ne $Commit -or [bool]$EngineEmbedded.dirty -ne $false -or $EngineEmbedded.version -ne $Version -or $EngineEmbedded.language -ne 'C++20' -or $EngineEmbedded.qt_version -ne $QtVersion -or $EngineEmbedded.protocol_version -ne 1) { throw 'Packaged engine provenance mismatch.' }
    if ((Get-Hash "$Output\engine\codex-proxy-guard.exe") -ne $EngineHash) { throw 'Packaged engine hash changed during deployment.' }
    $BridgeSmoke = Invoke-BridgeSmoke "$Output\engine\codex-proxy-guard.exe" $Version $Commit
    $Env:PATH = $OriginalPath
    $StatusAfter = (@(& git status --porcelain=v1 --untracked-files=all) -join "`n")
    $CommitAfter = (& git rev-parse HEAD | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $StatusAfter -ne $StatusBefore -or $CommitAfter -ne $Commit -or (Get-SourceFingerprint) -ne $SourceBefore) { throw 'Sources changed during GUI packaging; retry after edits finish.' }

    $Files = [ordered]@{}
    foreach ($File in (Get-ChildItem -LiteralPath $Output -Recurse -File | Sort-Object FullName)) {
        $Relative = $File.FullName.Substring($Output.Length + 1).Replace('\', '/')
        $Files[$Relative] = Get-Hash $File.FullName
    }
    $Info = [ordered]@{
        product_version = $Version; git_commit = $Commit; git_dirty = $false
        built_at_utc = [DateTime]::UtcNow.ToString('o')
        profile = 'dynamic-split'
        gui = @{ compiler = 'MSVC x64'; qt_version = $QtVersion; linkage = 'dynamic'; smoke_test = $GuiSmoke }
        engine = @{ version = $Version; language = 'C++20'; qt_version = $QtVersion; linkage = 'dynamic QtCore'; protocol_version = 1; path = 'engine/codex-proxy-guard.exe'; bridge_smoke_test = $BridgeSmoke }
        qt_source = @{ url = $SourceUrl; sha256 = $SourceHash; provided_by = "CodexProxyGuard-$Version-source-compliance.zip (not shipped inside the runtime ZIP)" }
        qt_sdk = @{ archive_url = $SdkManifest.archive_url; archive_sha256 = $SdkManifest.archive_sha256; manifest = 'licenses/Qt/sdk-6.8.3-msvc2022-x64.json'; deployed_files = $DeployedQt }
        files_sha256 = $Files
    }
    Write-Utf8 "$Output\build-info.json" ($Info | ConvertTo-Json -Depth 6)
    $HashLines = @($Files.GetEnumerator() | ForEach-Object { "$($_.Value)  $($_.Key)" })
    $HashLines += "$(Get-Hash "$Output\build-info.json")  build-info.json"
    Write-Utf8 "$Output\SHA256SUMS.txt" ($HashLines -join "`n")
    Compress-Directory $Output $RuntimeZip
    Write-Utf8 "$RuntimeZip.sha256" "$(Get-Hash $RuntimeZip)  $([IO.Path]::GetFileName($RuntimeZip))"

    $Compliance = New-SourceCompliance $Commit
    Write-Utf8 "$ComplianceZip.sha256" "$($Compliance.sha256)  $([IO.Path]::GetFileName($ComplianceZip))"

    Write-Utf8 $ReleaseManifest (@{
            schema_version = 2; profile = 'dynamic-split'; product_version = $Version
            source_commit = $Commit; source_dirty = $false
            built_at_utc = [DateTime]::UtcNow.ToString('o')
            runtime_archive = @{ name = [IO.Path]::GetFileName($RuntimeZip); sha256 = (Get-Hash $RuntimeZip); bytes = (Get-Item -LiteralPath $RuntimeZip).Length; layout = 'dist/gui directory' }
            source_compliance_archive = @{ name = [IO.Path]::GetFileName($ComplianceZip); sha256 = $Compliance.sha256; bytes = $Compliance.bytes; members = $Compliance.members }
            qt_source = @{ name = $SourceName; url = $SourceUrl; sha256 = $SourceHash }
            qt_sdk = @{ archive_url = $SdkManifest.archive_url; archive_sha256 = $SdkManifest.archive_sha256; manifest = 'licenses/Qt/sdk-6.8.3-msvc2022-x64.json' }
            toolchain = (Get-ToolchainRecord)
        } | ConvertTo-Json -Depth 5)
    Write-Utf8 (Join-Path $Root 'dist\SHA256SUMS.txt') (@(
            "$(Get-Hash $RuntimeZip)  $([IO.Path]::GetFileName($RuntimeZip))"
            "$($Compliance.sha256)  $([IO.Path]::GetFileName($ComplianceZip))"
            "$(Get-Hash $ReleaseManifest)  release-manifest.json"
        ) -join "`n")
    # Final independent gate: re-open both archives and re-verify CRC, sidecars,
    # every manifest member, deployed Qt hashes, provenance and the split
    # runtime/source relationship from scratch.
    Invoke-Checked $Python.Source @((Join-Path $PSScriptRoot 'verify-package.py'), $RuntimeZip, '--expected-commit', $Commit)
    Write-Host "Runtime bundle: $Output"
    Write-Host "Runtime ZIP: $RuntimeZip"
    Write-Host "Source compliance ZIP: $ComplianceZip"
    Write-Host "Release manifest: $ReleaseManifest"
    Write-Host "Source commit: $Commit (clean)"
} finally {
    $Env:PATH = $OriginalPath
    if ($null -ne $OriginalQtPluginPath) { $Env:QT_PLUGIN_PATH = $OriginalQtPluginPath } else { Remove-Item Env:QT_PLUGIN_PATH -ErrorAction SilentlyContinue }
    # The per-run SDK extraction is disposable; only the target/qt-source
    # archive cache persists between builds.
    $TargetDir = Join-Path $Root 'target'
    if ($QtRoot.StartsWith($TargetDir + '\', [StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $QtRoot).StartsWith('qt-sdk-')) {
        Remove-Item -LiteralPath $QtRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
    Pop-Location
}
