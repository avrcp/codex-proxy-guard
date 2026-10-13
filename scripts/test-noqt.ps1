# Configure the C++20 no-Qt product and execute every registered suite.
param([string] $BuildDirectory = 'target\cpp-noqt')
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$Build = if ([IO.Path]::IsPathRooted($BuildDirectory)) { $BuildDirectory } else { Join-Path $Root $BuildDirectory }
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
$Ninja = Join-Path $Vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe'
$PreviousPath = $Env:PATH
$Env:VSLANG = "1033"
try {
    $Commit = (& git -C $Root rev-parse HEAD | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'git rev-parse failed.' }
    $Dirty = [bool](@(& git -C $Root status --porcelain=v1 --untracked-files=all).Count)
    if ($LASTEXITCODE -ne 0) { throw 'git status failed.' }
    $ConfigureArgs = @(
        '-S', $Root, '-B', $Build, '-G', 'Ninja', "-DCMAKE_MAKE_PROGRAM=$Ninja",
        '-DCMAKE_BUILD_TYPE=Release', '-DBUILD_TESTING=ON',
        "-DCPG_BUILD_COMMIT=$Commit", "-DCPG_BUILD_DIRTY=$($Dirty.ToString().ToLowerInvariant())"
    )
    & $CMake @ConfigureArgs
    if ($LASTEXITCODE -ne 0) { throw 'CMake configuration failed.' }
    & $CMake --build $Build --parallel
    if ($LASTEXITCODE -ne 0) { throw 'C++ compilation failed.' }
    & $CTest --test-dir $Build --output-on-failure --no-tests=error
    if ($LASTEXITCODE -ne 0) { throw 'C++ tests failed.' }
} finally { $Env:PATH = $PreviousPath }
