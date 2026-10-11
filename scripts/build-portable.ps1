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
    [string] $Version = '0.7.0-rc.1',
    [switch] $Sign,
    [string] $CertificateSha1,
    [string] $TimestampUrl = 'http://timestamp.digicert.com',
    [switch] $AllowUnsignedDev,
    [string] $BuildDirectory = 'target\cpp-release'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Build = if ([IO.Path]::IsPathRooted($BuildDirectory)) { $BuildDirectory } else { Join-Path $Root $BuildDirectory }
$Staging = Join-Path $Root 'dist\staging'
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

# --- 1. Clean tree -----------------------------------------------------------
$Dirty = @(& git -C $Root status --porcelain=v1 --untracked-files=all).Count
if ($Dirty -ne 0) { throw 'The release build requires a clean working tree.' }
$Commit = (& git -C $Root rev-parse HEAD | Out-String).Trim()

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
$SignTool = Join-Path $Vs 'Common7\IDE\CommonExtensions\Microsoft\TeamFoundation\Team Explorer\NativeBinaries\signtool.exe'
if (-not (Test-Path $SignTool)) {
    $SignTool = (Get-Command signtool.exe -ErrorAction SilentlyContinue).Source
}
if ($Sign -and -not $SignTool) { throw 'signtool.exe is required for signing but was not found.' }

if (Test-Path $Build) { Remove-Item -Recurse -Force $Build }
Invoke-Checked $CMake @('-S', $Root, '-B', $Build, '-G', 'Ninja',
    "-DCMAKE_MAKE_PROGRAM=$((Get-Command ninja.exe).Source)",
    '-DCMAKE_BUILD_TYPE=Release', '-DBUILD_TESTING=ON',
    "-DCPG_BUILD_COMMIT=$Commit", '-DCPG_BUILD_DIRTY=false')
Invoke-Checked $CMake @('--build', $Build, '--parallel')
$Exe = Join-Path $Build 'app\CodexProxyGuard.exe'
if (-not (Test-Path $Exe)) { throw 'Build did not produce app\CodexProxyGuard.exe.' }

# --- 4. Test suites -----------------------------------------------------------
Invoke-Checked $CTest @('--test-dir', $Build, '--output-on-failure', '--no-tests=error')

# --- 5. Smoke: provenance, licenses, plain console ----------------------------
$Info = (& $Exe 'build-info' | ConvertFrom-Json)
if ($Info.version -ne $Version -or $Info.commit -ne $Commit -or $Info.language -ne 'C++20') {
    throw 'Packaged build-info provenance mismatch.'
}
$Licenses = (& $Exe 'licenses') -join "`n"
foreach ($Marker in '===== LICENSE =====', '===== json/LICENSE.MIT =====', '===== toml++/LICENSE =====') {
    if ($Licenses -notlike "*$Marker*") { throw "Embedded license stream missing $Marker." }
}
'SmokeConfigProbe' | Out-Null
$PlainProbe = 'q' | & $Exe 'console' '--plain'
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
if (Test-Path $Staging) { Remove-Item -Recurse -Force $Staging }
$ReleaseDirectory = Join-Path $Root "dist\releases\$Version-$($Commit.Substring(0, 8))"
if (Test-Path $ReleaseDirectory) { Remove-Item -Recurse -Force $ReleaseDirectory }
New-Item -ItemType Directory -Path $Staging, (Split-Path $ReleaseDirectory -Parent) -Force | Out-Null

$StagedExe = Join-Path $Staging 'CodexProxyGuard.exe'
Copy-Item $Exe $StagedExe
$ExeHash = Get-Hash $StagedExe
$ExeSize = (Get-Item $StagedExe).Length

$TransportZip = Join-Path $Staging $TransportZipName
Compress-Archive -Path $StagedExe -DestinationPath $TransportZip -CompressionLevel Optimal
$TransportHash = Get-Hash $TransportZip
Write-Utf8 "$TransportZip.sha256" "$TransportHash $TransportZipName"

$SourceZip = Join-Path $Staging $SourceZipName
$SourceList = & git -C $Root ls-files
$SourceTemp = Join-Path $env:TEMP "cpg-source-$Commit"
if (Test-Path $SourceTemp) { Remove-Item -Recurse -Force $SourceTemp }
New-Item -ItemType Directory -Path $SourceTemp | Out-Null
foreach ($Relative in $SourceList) {
    $Target = Join-Path $SourceTemp $Relative
    New-Item -ItemType Directory -Path (Split-Path $Target -Parent) -Force | Out-Null
    Copy-Item (Join-Path $Root $Relative) $Target
}
Compress-Archive -Path (Join-Path $SourceTemp '*') -DestinationPath $SourceZip -CompressionLevel Optimal
Remove-Item -Recurse -Force $SourceTemp
$SourceHash = Get-Hash $SourceZip
Write-Utf8 "$SourceZip.sha256" "$SourceHash $SourceZipName"

Write-Utf8 (Join-Path $Staging 'SHA256SUMS.txt') (@(
    "$ExeHash CodexProxyGuard.exe",
    "$TransportHash $TransportZipName",
    "$SourceHash $SourceZipName"
) -join "`n")

$SizeReportDirectory = Join-Path $Root 'dist\size-report'
New-Item -ItemType Directory -Path $SizeReportDirectory -Force | Out-Null
Invoke-Checked (Get-Command python).Source @((Join-Path $Root 'scripts\report-package-size.py'),
    '--root', $Staging, '--archive', $TransportZip, '--output', $SizeReportDirectory)
$SizeReport = Join-Path $Staging 'size-report.json'
Copy-Item (Join-Path $SizeReportDirectory 'size-report.json') $SizeReport

$BuildDate = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
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
    artifacts = @('CodexProxyGuard.exe', $TransportZipName, $SourceZipName,
                  'release-manifest.json', 'SHA256SUMS.txt', 'size-report.json')
}
Write-Utf8 (Join-Path $Staging 'release-manifest.json') ($Manifest | ConvertTo-Json -Depth 5)

# --- 14. Independent re-verification ------------------------------------------
$StagingRelease = Join-Path $Staging 'release'
New-Item -ItemType Directory -Path $StagingRelease -Force | Out-Null
# verify-package expects the release directory itself
Invoke-Checked (Get-Command python).Source @((Join-Path $Root 'scripts\verify-package.py'),
    '--release', $Staging, '--schema', 'static-single')

# --- 15. Promotion -------------------------------------------------------------
New-Item -ItemType Directory -Path $ReleaseDirectory -Force | Out-Null
Copy-Item (Join-Path $Staging '*') $ReleaseDirectory
Invoke-Checked (Get-Command python).Source @((Join-Path $Root 'scripts\verify-package.py'),
    '--release', $ReleaseDirectory, '--schema', 'static-single')
Remove-Item -Recurse -Force $Staging

Write-Host ''
Write-Host "Release promoted: $ReleaseDirectory"
Write-Host "  CodexProxyGuard.exe $([math]::Round($ExeSize / 1MB, 2)) MiB  sha256=$ExeHash"
Write-Host "  signed=$Signed authenticode=$AuthenticodeStatus"
if (-not $Signed) { Write-Host '  UNSIGNED DEVELOPMENT BUILD' }
