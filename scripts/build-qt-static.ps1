<#
.SYNOPSIS
Builds the pinned static Qt 6.8.3 SDK with the static CRT for this project.
.DESCRIPTION
Verifies the cached upstream QtBase source archive, configures a Release
static build with -static-runtime (MSVC /MT), builds with a bounded parallel
level and installs it into an isolated prefix under target/qt-static/<key>.
The recipe key binds the cache to the exact source hash, configure arguments,
architecture and toolchain versions, so a stale cache can never be silently
reused. A COMPLETE marker is written only after install verification; failed
builds leave no reusable cache. Qt 6.8's build system is fully CMake-based
(configure.bat only forwards to cmake), so no Perl installation is required.
#>
param(
    [int] $Parallel = 0
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$QtVersion = '6.8.3'
$SourceName = "qtbase-everywhere-src-$QtVersion.tar.xz"
$SourceUrl = "https://download.qt.io/archive/qt/6.8/$QtVersion/submodules/$SourceName"
$SourceHash = '56001b905601bb9023d399f3ba780d7fa940f3e4861e496a7c490331f49e0b80'
$SourceCache = Join-Path $Root "target\qt-source\$SourceName"
$StaticRoot = Join-Path $Root 'target\qt-static'
$ConfigureArguments = @('-release', '-static', '-static-runtime', '-opensource', '-confirm-license', '-nomake', 'examples', '-nomake', 'tests')

function Get-Hash([string] $Path) {
    $Stream = [IO.File]::OpenRead($Path)
    $Hasher = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($Hasher.ComputeHash($Stream))).Replace('-', '').ToLowerInvariant() }
    finally { $Hasher.Dispose(); $Stream.Dispose() }
}

function Write-Utf8([string] $Path, [string] $Text) {
    [IO.File]::WriteAllText($Path, $Text + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
}

function Invoke-Checked([string] $File, [string[]] $Arguments, [string] $WorkingDirectory = '') {
    if ($WorkingDirectory) { Push-Location $WorkingDirectory }
    try {
        & $File @Arguments
        if ($LASTEXITCODE -ne 0) { throw "$File failed with exit code $LASTEXITCODE" }
    } finally { if ($WorkingDirectory) { Pop-Location } }
}

Push-Location $Root
try {
    if (-not (Test-Path -LiteralPath $SourceCache -PathType Leaf)) {
        New-Item -ItemType Directory -Path (Split-Path $SourceCache) -Force | Out-Null
        Write-Host "Downloading corresponding Qt source: $SourceUrl"
        Invoke-WebRequest -UseBasicParsing -Uri $SourceUrl -OutFile $SourceCache -TimeoutSec 900
    }
    if ((Get-Hash $SourceCache) -ne $SourceHash) { throw 'Qt source archive SHA-256 mismatch; remove the cached archive and retry.' }

    $VsWhere = "${Env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $Vs = (& $VsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $Vs) { throw 'MSVC x64 C++ build tools are required.' }
    $VsDevCmd = Join-Path $Vs 'Common7\Tools\VsDevCmd.bat'
    $DevEnvironment = & $Env:ComSpec /d /c "call `"$VsDevCmd`" -no_logo -arch=x64 -host_arch=x64 >nul && set"
    if ($LASTEXITCODE -ne 0) { throw 'Cannot initialize the MSVC x64 environment.' }
    foreach ($Line in $DevEnvironment) {
        if ($Line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process') }
    }
    $CMake = Join-Path $Vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
    $Ninja = Join-Path $Vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe'
    foreach ($Tool in @($CMake, $Ninja)) { if (-not (Test-Path -LiteralPath $Tool)) { throw "Required build tool missing: $Tool" } }
    $Msvc = "${Env:VCToolsVersion}"
    if ([string]::IsNullOrWhiteSpace($Msvc)) {
        $Msvc = (Get-Content -LiteralPath (Join-Path $Vs 'VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt') -Raw).Trim()
    }
    $Env:PATH = "$(Split-Path $Ninja);$Env:PATH"
    if ($Parallel -le 0) {
        $Parallel = [Math]::Min([Environment]::ProcessorCount, 16)
    }

    # The recipe key binds the cache to every build input that matters.
    $RecipeInput = (@($SourceHash, "$Parallel", $Msvc, "$Env:WindowsSDKVersion",
                (& $CMake --version | Select-Object -First 1), (& $Ninja --version),
                'x86_64', ($ConfigureArguments -join ' ')) -join "`n")
    $RecipeHasher = [Security.Cryptography.SHA256]::Create()
    $RecipeKey = ([BitConverter]::ToString($RecipeHasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($RecipeInput)))).Replace('-', '').ToLowerInvariant().Substring(0, 16)
    $RecipeHasher.Dispose()
    $KeyRoot = Join-Path $StaticRoot $RecipeKey
    $Source = Join-Path $KeyRoot 'src'
    $Build = Join-Path $KeyRoot 'build'
    $Install = Join-Path $KeyRoot 'install'
    $Complete = Join-Path $KeyRoot 'COMPLETE'
    $ManifestPath = Join-Path $KeyRoot 'manifest.json'

    if ((Test-Path -LiteralPath $Complete -PathType Leaf) -and (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) {
        $Manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
        $Valid = $Manifest.recipe_key -eq $RecipeKey -and $Manifest.qt_source_sha256 -eq $SourceHash
        foreach ($Required in @('bin\moc.exe', 'bin\rcc.exe', 'lib\cmake\Qt6\Qt6Config.cmake',
                    'lib\cmake\Qt6Core\Qt6CoreConfig.cmake', 'lib\cmake\Qt6Gui\Qt6GuiConfig.cmake',
                    'lib\cmake\Qt6Widgets\Qt6WidgetsConfig.cmake', 'lib\cmake\Qt6Test\Qt6TestConfig.cmake')) {
            if (-not (Test-Path -LiteralPath (Join-Path $Install $Required) -PathType Leaf)) { $Valid = $false }
        }
        foreach ($StaticLib in @('lib\Qt6Core.lib', 'lib\Qt6Gui.lib', 'lib\Qt6Widgets.lib', 'lib\Qt6Test.lib')) {
            if (-not (Test-Path -LiteralPath (Join-Path $Install $StaticLib) -PathType Leaf)) { $Valid = $false }
        }
        if ($Valid) {
            Write-Host "Reusing verified static Qt SDK: $Install"
            Write-Host "recipe key: $RecipeKey"
            return
        }
        throw "Stale static Qt cache at $KeyRoot (COMPLETE marker present but verification failed). Remove the directory and retry."
    }
    if (Test-Path -LiteralPath $KeyRoot) {
        Write-Host "Removing incomplete static Qt build tree: $KeyRoot"
        Remove-Item -LiteralPath $KeyRoot -Recurse -Force
    }
    New-Item -ItemType Directory -Path $KeyRoot -Force | Out-Null

    Write-Host "Extracting Qt source (recipe key $RecipeKey)..."
    $SourceStaging = Join-Path $KeyRoot 'src-extract'
    New-Item -ItemType Directory -Path $SourceStaging -Force | Out-Null
    # Windows bsdtar: GNU tar from a Git installation treats "D:" as a host.
    $Tar = Join-Path $Env:SystemRoot 'System32\tar.exe'
    & $Tar -xJf $SourceCache -C $SourceStaging --strip-components=1
    if ($LASTEXITCODE -ne 0) { throw 'Qt source extraction failed.' }
    Move-Item -Path (Join-Path $SourceStaging '*') -Destination $Source
    Remove-Item -LiteralPath $SourceStaging -Force

    Write-Host "Configuring static Qt ($($ConfigureArguments -join ' '))..."
    New-Item -ItemType Directory -Path $Build -Force | Out-Null
    # Qt's configure accepts "-prefix <path>" (space separated); the single
    # dash form does not support "=" assignment.
    Invoke-Checked (Join-Path $Source 'configure.bat') (@('-prefix', $Install) + $ConfigureArguments) $Build

    Write-Host "Building static Qt with $Parallel parallel jobs (this is the long step)..."
    Invoke-Checked $CMake @('--build', $Build, '--parallel', "$Parallel")
    Invoke-Checked $CMake @('--install', $Build)

    # Post-install verification of the tools, CMake packages and static libs
    # the application build will consume.
    foreach ($Required in @('bin\moc.exe', 'bin\rcc.exe', 'bin\qtpaths.exe',
                'lib\Qt6Core.lib', 'lib\Qt6Gui.lib', 'lib\Qt6Widgets.lib', 'lib\Qt6Test.lib', 'lib\Qt6EntryPoint.lib',
                'lib\cmake\Qt6\Qt6Config.cmake', 'lib\cmake\Qt6Core\Qt6CoreConfig.cmake',
                'lib\cmake\Qt6Gui\Qt6GuiConfig.cmake', 'lib\cmake\Qt6Widgets\Qt6WidgetsConfig.cmake',
                'lib\cmake\Qt6Test\Qt6TestConfig.cmake')) {
        if (-not (Test-Path -LiteralPath (Join-Path $Install $Required) -PathType Leaf)) {
            throw "Static Qt install verification failed: $Required is missing."
        }
    }
    foreach ($Summary in @('configure.summary.txt', 'config.summary')) {
        Copy-Item -LiteralPath (Join-Path $Build $Summary) -Destination $KeyRoot -ErrorAction SilentlyContinue
    }
    $CacheSummary = Get-Content -LiteralPath (Join-Path $Build 'CMakeCache.txt') -ErrorAction Stop
    $Relevant = @($CacheSummary | Where-Object { $_ -match '^(QT_BUILD_|BUILD_TYPE|CMAKE_INSTALL_PREFIX|QT_FEATURE_|CMAKE_MSVC_RUNTIME_LIBRARY)' })
    Write-Utf8 (Join-Path $KeyRoot 'cmake-cache-summary.txt') ($Relevant -join "`n")

    Write-Utf8 $ManifestPath (@{
            recipe_key = $RecipeKey; qt_version = $QtVersion
            qt_source_url = $SourceUrl; qt_source_sha256 = $SourceHash
            configure_arguments = $ConfigureArguments; parallel = $Parallel
            msvc = "MSVC $Msvc"; windows_sdk = "$Env:WindowsSDKVersion"
            cmake = (& $CMake --version | Select-Object -First 1); ninja = (& $Ninja --version)
            build_type = 'Release'; crt = 'static (/MT via -static-runtime)'
            built_at_utc = [DateTime]::UtcNow.ToString('o')
        } | ConvertTo-Json)
    Write-Utf8 $Complete "verified"
    Write-Host "Static Qt SDK installed: $Install"
    Write-Host "recipe key: $RecipeKey"
} finally {
    Pop-Location
}
