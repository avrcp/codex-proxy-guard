<#
.SYNOPSIS
Builds and verifies the no-Qt single-EXE Windows x64 portable release.

.DESCRIPTION
Canonical release pipeline: clean-tree check, CMake configure+build (static
CRT), full CTest, packaged-EXE provenance/licenses smoke, import scan,
optional Authenticode signing (signtool) with verification, SHA-256 digests,
an EXE-only transport ZIP, a source snapshot ZIP, size report and
release-manifest.json — everything re-verified from scratch by
scripts/verify-package.py --schema static-single before promotion to
dist/releases/<version>-<commit>.

Use scripts/test-noqt.ps1 for development builds (this script requires a
clean tree). Signing requires a code-signing certificate thumbprint visible
in the user or machine store; without -Sign the release must explicitly pass
-AllowUnsignedDev and is marked UNSIGNED DEVELOPMENT BUILD everywhere.
#>
param(
    [string] $Version = '0.7.0-rc.3',
    [switch] $Sign,
    [string] $CertificateSha1,
    [string] $TimestampUrl = 'http://timestamp.digicert.com',
    [switch] $AllowUnsignedDev,
    [string] $BuildDirectory = 'target\cpp-release'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$TargetRoot = [IO.Path]::GetFullPath((Join-Path $Root 'target'))
$BuildBase = [IO.Path]::GetFullPath($(if ([IO.Path]::IsPathRooted($BuildDirectory)) { $BuildDirectory } else { Join-Path $Root $BuildDirectory }))
function Assert-TargetPath([string] $Path) {
    $Full = [IO.Path]::GetFullPath($Path)
    if (-not $Full.StartsWith($TargetRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Release intermediates must be strictly inside target: $Full"
    }
    $Ancestor = $Full
    while ($Ancestor -and $Ancestor -ne $Root) {
        if ((Test-Path -LiteralPath $Ancestor) -and ((Get-Item -LiteralPath $Ancestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Release intermediates cannot traverse reparse points: $Ancestor"
        }
        $Ancestor = Split-Path $Ancestor -Parent
    }
}
function Assert-ReleasePath([string] $Path) {
    $Full = [IO.Path]::GetFullPath($Path)
    $ReleaseRoot = [IO.Path]::GetFullPath((Join-Path $Root 'dist\releases'))
    if (-not $Full.StartsWith($ReleaseRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Release destination must stay inside dist/releases: $Full"
    }
    $Ancestor = $Full
    while ($Ancestor -and $Ancestor -ne $Root) {
        if ((Test-Path -LiteralPath $Ancestor) -and ((Get-Item -LiteralPath $Ancestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Release destination cannot traverse reparse points: $Ancestor"
        }
        $Ancestor = Split-Path $Ancestor -Parent
    }
}
Assert-TargetPath $BuildBase
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[A-Za-z0-9.]+)?$') { throw 'Invalid release version.' }
$RunDirectory = Join-Path $BuildBase ([Guid]::NewGuid().ToString('N'))
Assert-TargetPath $RunDirectory
$Build = Join-Path $RunDirectory 'build'
$SourceRoot = Join-Path $RunDirectory 'source'
$TransportZipName = "CodexProxyGuard-$Version-windows-x86_64.zip"
$SourceZipName = "CodexProxyGuard-$Version-source.zip"

if ($Sign -and -not $CertificateSha1) { throw 'Signing requires -CertificateSha1 <thumbprint>.' }
if (-not $Sign -and -not $AllowUnsignedDev) {
    throw 'Public releases must be signed (-Sign -CertificateSha1 ...); pass -AllowUnsignedDev for an explicitly unsigned development build.'
}

function Invoke-Checked([string] $File, [string[]] $Arguments) {
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$File failed with exit code $LASTEXITCODE" }
}

function Get-Hash([string] $Path) {
    $Stream = [IO.File]::OpenRead($Path)
    $Hasher = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($Hasher.ComputeHash($Stream))).Replace('-', '').ToLowerInvariant() }
    finally { $Hasher.Dispose(); $Stream.Dispose() }
}

function Write-Utf8([string] $Path, [string] $Text) {
    [IO.File]::WriteAllText($Path, $Text + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
}

# Serialize source capture, tests, signing and promotion across release runs.
New-Item -ItemType Directory -Path $TargetRoot -Force | Out-Null
$ReleaseLock = [IO.File]::Open((Join-Path $TargetRoot 'release.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
try {
# --- 1. Clean tree + immutable release destination ----------------------------
$Dirty = @(& git -C $Root status --porcelain=v1 --untracked-files=all).Count
if ($LASTEXITCODE -ne 0) { throw 'git status failed.' }
if ($Dirty -ne 0) { throw 'The release build requires a clean working tree.' }
$Commit = (& git -C $Root rev-parse HEAD | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $Commit -notmatch '^[0-9a-f]{40}$') { throw 'git rev-parse failed.' }
# Release directories are immutable: a verified, possibly already distributed
# build of this version/commit must never be silently overwritten. Checked
# before anything expensive runs.
$ReleaseDirectory = Join-Path $Root "dist\releases\$Version-$($Commit.Substring(0, 8))"
Assert-ReleasePath $ReleaseDirectory
if (Test-Path -LiteralPath $ReleaseDirectory) {
    throw "Release destination already exists; versions/commits are immutable: $ReleaseDirectory"
}

# --- 2/3. Toolchain, configure, build ----------------------------------------
$VsWhere = "${Env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$Vs = (& $VsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or -not $Vs) { throw 'MSVC x64 tools are required.' }
$VsDevCmd = Join-Path $Vs 'Common7\Tools\VsDevCmd.bat'
$EnvironmentLines = & $Env:ComSpec /d /c "call `"$VsDevCmd`" -no_logo -arch=x64 -host_arch=x64 >nul && set"
if ($LASTEXITCODE -ne 0) { throw 'MSVC environment setup failed.' }
foreach ($Line in $EnvironmentLines) {
    if ($Line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process') }
}
$CMake = Join-Path $Vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
$CTest = Join-Path (Split-Path $CMake) 'ctest.exe'
$DumpBin = Join-Path $Vs 'VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe'
$DumpBin = (Get-Item $DumpBin | Select-Object -First 1).FullName
$SignTool = $null
if ($Sign) {
    $SignTool = Join-Path $Vs 'Common7\IDE\CommonExtensions\Microsoft\TeamFoundation\Team Explorer\NativeBinaries\signtool.exe'
    if (-not (Test-Path -LiteralPath $SignTool)) {
        $Command = Get-Command signtool.exe -ErrorAction SilentlyContinue
        if ($Command) { $SignTool = $Command.Source } else { $SignTool = $null }
    }
    if (-not $SignTool) { throw 'signtool.exe is required for signing but was not found.' }
}

# Build only the fixed Git object's source. Never copy mutable workspace files.
New-Item -ItemType Directory -Path $RunDirectory | Out-Null
$CapturedSourceZip = Join-Path $RunDirectory $SourceZipName
Invoke-Checked 'git' @('-C', $Root, 'archive', '--format=zip', "--output=$CapturedSourceZip", $Commit)
Expand-Archive -LiteralPath $CapturedSourceZip -DestinationPath $SourceRoot
$Env:VSLANG = '1033'
Invoke-Checked $CMake @('-S', $SourceRoot, '-B', $Build, '-G', 'Ninja',
    "-DCMAKE_MAKE_PROGRAM=$((Get-Command ninja.exe).Source)",
    '-DCMAKE_BUILD_TYPE=Release', '-DBUILD_TESTING=ON',
    "-DCPG_BUILD_COMMIT=$Commit", "-DCPG_PRODUCT_VERSION=$Version", '-DCPG_BUILD_DIRTY=false')
Invoke-Checked $CMake @('--build', $Build, '--parallel')
$Exe = Join-Path $Build 'app\CodexProxyGuard.exe'
if (-not (Test-Path $Exe)) { throw 'Build did not produce app\CodexProxyGuard.exe.' }

# --- 4. Test suites -----------------------------------------------------------
Invoke-Checked $CTest @('--test-dir', $Build, '--output-on-failure', '--no-tests=error')

# --- 5. Smoke: provenance, licenses, plain console ----------------------------
$InfoText = & $Exe 'build-info'
if ($LASTEXITCODE -ne 0) { throw 'build-info smoke failed.' }
$Info = $InfoText | ConvertFrom-Json
if ($Info.version -ne $Version -or $Info.commit -ne $Commit -or $Info.language -ne 'C++20' -or $Info.dirty -ne $false) {
    throw 'Packaged build-info provenance mismatch.'
}
$Licenses = (& $Exe 'licenses') -join "`n"
if ($LASTEXITCODE -ne 0) { throw 'licenses smoke failed.' }
foreach ($Marker in '===== LICENSE =====', '===== json/LICENSE.MIT =====', '===== toml++/LICENSE =====') {
    if ($Licenses -notlike "*$Marker*") { throw "Embedded license stream missing $Marker." }
}
$SmokeConfig = Join-Path $RunDirectory 'smoke-config.toml'
$PlainProbe = 'q' | & $Exe 'console' '--plain' '--config' $SmokeConfig
if ($LASTEXITCODE -ne 0) { throw 'Plain-console smoke failed.' }

# --- 6. Import scan -----------------------------------------------------------
$Dependents = & $DumpBin '/DEPENDENTS' $Exe
if ($LASTEXITCODE -ne 0) { throw 'dumpbin failed.' }
$ImportText = ($Dependents -join "`n")
$Forbidden = Select-String -InputObject $ImportText -Pattern 'Qt6|MSVCP140|VCRUNTIME140|CONCRT140' -AllMatches
if ($Forbidden) { throw "Import scan found forbidden dependencies: $ImportText" }
$ImportList = @($ImportText -split "`n" | Where-Object { $_ -match '\.dll$' } | ForEach-Object { $_.Trim().ToUpperInvariant() })

# --- 7/8. Authenticode --------------------------------------------------------
$Signed = $false
$AuthenticodeStatus = 'Unsigned'
$Publisher = ''
if ($Sign) {
    Invoke-Checked $SignTool @('sign', '/sha1', $CertificateSha1, '/fd', 'SHA256',
        '/tr', $TimestampUrl, '/td', 'SHA256', $Exe)
    & $SignTool @('verify', '/pa', '/all', $Exe)
    if ($LASTEXITCODE -ne 0) { throw 'signtool verify failed after signing.' }
    $Signed = $true
    $AuthenticodeStatus = 'Valid'
    $Signature = & $SignTool @('verify', '/pa', '/all', '/v', $Exe) | Out-String
    if ($Signature -match 'Issued to:\s+(.+)$') { $Publisher = $Matches[1].Trim() }
}

# Nothing may touch the EXE after this point (signing is the last mutation).

# --- 9-13. Package ------------------------------------------------------------
# Unique per-run staging on the same volume as the release directory: two
# parallel builds can never delete each other's work, and promotion is one
# atomic Move-Item instead of file-by-file copies.
$Staging = Join-Path $Root ("target\release-staging\$Version-$($Commit.Substring(0, 8))-$PID-" +`
    [Guid]::NewGuid().ToString('N').Substring(0, 8))
Assert-TargetPath $Staging
Assert-ReleasePath $ReleaseDirectory
New-Item -ItemType Directory -Path $Staging, (Split-Path $ReleaseDirectory -Parent) -Force | Out-Null
try {

$StagedExe = Join-Path $Staging 'CodexProxyGuard.exe'
Copy-Item $Exe $StagedExe
$ExeHash = Get-Hash $StagedExe
$ExeSize = (Get-Item $StagedExe).Length

$TransportZip = Join-Path $Staging $TransportZipName
Compress-Archive -Path $StagedExe -DestinationPath $TransportZip -CompressionLevel Optimal
$TransportHash = Get-Hash $TransportZip
Write-Utf8 "$TransportZip.sha256" "$TransportHash $TransportZipName"

$SourceZip = Join-Path $Staging $SourceZipName
Copy-Item -LiteralPath $CapturedSourceZip -Destination $SourceZip
$SourceHash = Get-Hash $SourceZip
Write-Utf8 "$SourceZip.sha256" "$SourceHash $SourceZipName"

Write-Utf8 (Join-Path $Staging 'SHA256SUMS.txt') (@(
    "$ExeHash CodexProxyGuard.exe",
    "$TransportHash $TransportZipName",
    "$SourceHash $SourceZipName"
) -join "`n")

$SizeReportDirectory = Join-Path $RunDirectory 'size-report'
New-Item -ItemType Directory -Path $SizeReportDirectory -Force | Out-Null
Invoke-Checked (Get-Command python).Source @((Join-Path $SourceRoot 'scripts\report-package-size.py'),
    '--root', $Staging, '--archive', $TransportZip, '--output', $SizeReportDirectory)
$SizeReport = Join-Path $Staging 'size-report.json'
Copy-Item (Join-Path $SizeReportDirectory 'size-report.json') $SizeReport

$BuildDate = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
# PE mitigation facts (CFG/CET/ASLR/DEP) derived by the verifier's own PE
# parser from the packaged EXE - never hand-maintained here.
$MitigationsJson = (& (Get-Command python).Source @((Join-Path $SourceRoot 'scripts\verify-package.py'),
    '--mitigations-of', $Exe) | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { throw 'PE mitigation inspection failed.' }
$Mitigations = $MitigationsJson | ConvertFrom-Json
$Manifest = [ordered]@{
    schema = 'static-single'
    schema_version = 1
    version = $Version
    commit = $Commit
    dirty = $false
    created_utc = $BuildDate
    qt = $false
    powershell_appx_discovery = $false
    subsystem = 'console'
    crt = 'static'
    mitigations = $Mitigations
    signed = $Signed
    authenticode_status = $AuthenticodeStatus
    publisher = $Publisher
    unsigned_dev = (-not $Signed)
    build_notes = if ($Signed) { 'Authenticode signed release build' } else { 'UNSIGNED DEVELOPMENT BUILD - not for public distribution' }
    executable = [ordered]@{
        path = 'CodexProxyGuard.exe'
        sha256 = $ExeHash
        size = $ExeSize
        signed = $Signed
        qt = $false
        imports = $ImportList
    }
    transport_zip = [ordered]@{ path = $TransportZipName; sha256 = $TransportHash }
    source_zip = [ordered]@{ path = $SourceZipName; sha256 = $SourceHash }
    artifacts = @('CodexProxyGuard.exe', $TransportZipName, "$TransportZipName.sha256",
                  $SourceZipName, "$SourceZipName.sha256", 'release-manifest.json',
                  'SHA256SUMS.txt', 'size-report.json')
}
Write-Utf8 (Join-Path $Staging 'release-manifest.json') ($Manifest | ConvertTo-Json -Depth 5)

# --- 14. Independent re-verification ------------------------------------------
Invoke-Checked (Get-Command python).Source @((Join-Path $SourceRoot 'scripts\verify-package.py'),
    '--release', $Staging, '--schema', 'static-single')

# --- 15. Promotion: one atomic move on the same volume -------------------------
$FinalHead = (& git -C $Root rev-parse HEAD | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $FinalHead -ne $Commit) { throw 'Source HEAD changed during release.' }
$FinalDirty = @(& git -C $Root status --porcelain=v1 --untracked-files=all).Count
if ($LASTEXITCODE -ne 0 -or $FinalDirty -ne 0) { throw 'Working tree changed during release.' }
if (Test-Path -LiteralPath $ReleaseDirectory) { throw 'Immutable release destination appeared during build.' }
Assert-TargetPath $Staging
Assert-ReleasePath $ReleaseDirectory
Move-Item -LiteralPath $Staging -Destination $ReleaseDirectory
Invoke-Checked (Get-Command python).Source @((Join-Path $SourceRoot 'scripts\verify-package.py'),
    '--release', $ReleaseDirectory, '--schema', 'static-single')
} finally {
    # The promoted directory is never touched here; only leftover staging of
    # a failed run is cleaned up.
    if (Test-Path -LiteralPath $Staging) {
        Assert-TargetPath $Staging
        Remove-Item -LiteralPath $Staging -Recurse -Force
    }
}

Write-Host ''
Write-Host "Release promoted: $ReleaseDirectory"
Write-Host "  CodexProxyGuard.exe $([math]::Round($ExeSize / 1MB, 2)) MiB  sha256=$ExeHash"
Write-Host "  signed=$Signed authenticode=$AuthenticodeStatus"
if (-not $Signed) { Write-Host '  UNSIGNED DEVELOPMENT BUILD' }

} finally { $ReleaseLock.Dispose() }
