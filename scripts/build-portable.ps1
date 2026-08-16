<#
.SYNOPSIS
    Builds the canonical single-file Windows portable artifact.
.PARAMETER OutputDir
    Destination directory. Relative paths are resolved from the repository root.
.PARAMETER NoSmokeTest
    Skip the side-effect-free `--version` and `launch --help` smoke tests.
#>
param(
    [string] $OutputDir = "dist\portable",
    [switch] $NoSmokeTest
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$Package = "codex-proxy-guard"
$PortableName = "codex-proxy-guard-windows-x86_64.exe"
$Target = "x86_64-pc-windows-msvc"
$Profile = "release"
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$Output = if ([IO.Path]::IsPathRooted($OutputDir)) {
    [IO.Path]::GetFullPath($OutputDir)
} else {
    [IO.Path]::GetFullPath((Join-Path $Root $OutputDir))
}

$OutputRoot = [IO.Path]::GetPathRoot($Output)
$RootWithSeparator = $Root.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
$OutputWithSeparator = $Output.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
if (
    $Output -eq $OutputRoot -or
    $Output -eq $Root -or
    $RootWithSeparator.StartsWith($OutputWithSeparator, [StringComparison]::OrdinalIgnoreCase)
) {
    throw "Unsafe OutputDir resolves to a filesystem root, repository root, or repository parent: $Output"
}

$Cargo = Get-Command cargo.exe -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $Cargo) {
    throw "cargo.exe was not found. Install Rust and ensure cargo is on PATH."
}

Push-Location $Root
try {
    $MetadataJson = & $Cargo.Source metadata --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE"
    }
    try {
        $CargoMetadata = ($MetadataJson | Out-String) | ConvertFrom-Json
    } catch {
        throw "cargo metadata returned invalid JSON: $($_.Exception.Message)"
    }
    $PackageMatches = @($CargoMetadata.packages | Where-Object { $_.name -eq $Package })
    if ($PackageMatches.Count -ne 1) {
        throw "Cargo package metadata must contain exactly one '$Package' package; found $($PackageMatches.Count)."
    }
    $PackageMetadata = $PackageMatches[0]
    $BinaryTargets = @(
        $PackageMetadata.targets |
            Where-Object { $_.name -eq $Package -and $_.kind -contains "bin" }
    )
    if ($BinaryTargets.Count -ne 1) {
        throw "Cargo package '$Package' must contain exactly one '$Package' binary target; found $($BinaryTargets.Count)."
    }

    if (Test-Path -LiteralPath $Output) {
        Remove-Item -LiteralPath $Output -Recurse -Force
    }
    New-Item -ItemType Directory -Path $Output -Force | Out-Null

    Write-Host "Building $Package $($PackageMetadata.version) in release mode..."
    & $Cargo.Source build --release --locked --target $Target -p $Package
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }

    $Source = Join-Path $Root "target\$Target\release\$($BinaryTargets[0].name).exe"
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        throw "Build succeeded but the expected binary was not found: $Source"
    }

    $Destination = Join-Path $Output $PortableName
    Copy-Item -LiteralPath $Source -Destination $Destination -Force

    if ($NoSmokeTest) {
        $VersionSmokeOutput = "skipped (-NoSmokeTest)"
        $LaunchHelpSmokeOutput = "skipped (-NoSmokeTest)"
    } else {
        Write-Host "Running side-effect-free smoke test: $PortableName --version"
        $SmokeLines = @(& $Destination --version 2>&1 | ForEach-Object { $_.ToString() })
        $SmokeExitCode = $LASTEXITCODE
        $VersionSmokeOutput = ($SmokeLines -join [Environment]::NewLine).Trim()
        if ($SmokeExitCode -ne 0) {
            throw "Portable binary smoke test failed with exit code $SmokeExitCode"
        }
        if ([String]::IsNullOrWhiteSpace($VersionSmokeOutput)) {
            throw "Portable binary smoke test produced no output"
        }
        if ($VersionSmokeOutput -notmatch [Regex]::Escape([string]$PackageMetadata.version)) {
            throw "Portable binary smoke output did not contain Cargo package version $($PackageMetadata.version): $VersionSmokeOutput"
        }

        Write-Host "Running side-effect-free smoke test: $PortableName launch --help"
        $HelpLines = @(& $Destination launch --help 2>&1 | ForEach-Object { $_.ToString() })
        $HelpExitCode = $LASTEXITCODE
        $LaunchHelpSmokeOutput = ($HelpLines -join [Environment]::NewLine).Trim()
        if ($HelpExitCode -ne 0) {
            throw "Portable launch help smoke test failed with exit code $HelpExitCode"
        }
        if (
            [String]::IsNullOrWhiteSpace($LaunchHelpSmokeOutput) -or
            $LaunchHelpSmokeOutput -notmatch "--json"
        ) {
            throw "Portable launch help smoke output did not contain the expected options"
        }
    }

    $Sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $HashBytes = $Sha256.ComputeHash([IO.File]::ReadAllBytes($Destination))
    } finally {
        $Sha256.Dispose()
    }
    $Hash = [BitConverter]::ToString($HashBytes).Replace('-', '').ToLowerInvariant()
    $AuthenticodeStatus = "Unavailable"
    try {
        $AuthenticodeStatus = [string](Get-AuthenticodeSignature -LiteralPath $Destination).Status
    } catch {
        Write-Warning "Authenticode status is unavailable in this PowerShell environment."
    }
    $ShaPath = Join-Path $Output "$PortableName.sha256"
    "$Hash  $PortableName" | Set-Content -LiteralPath $ShaPath -Encoding ASCII

    $BuildInfo = [ordered]@{
        package = $Package
        target = $Target
        profile = $Profile
        version = [string]$PackageMetadata.version
        binary = $PortableName
        sha256 = $Hash
        smoke_test = $VersionSmokeOutput
        smoke_tests = [ordered]@{
            version = $VersionSmokeOutput
            launch_help = $LaunchHelpSmokeOutput
        }
        authenticode_status = $AuthenticodeStatus
        built_at_utc = [DateTime]::UtcNow.ToString("o")
    }

    $Git = Get-Command git.exe -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($Git -and (Test-Path -LiteralPath (Join-Path $Root ".git"))) {
        $GitCommit = (& $Git.Source -C $Root rev-parse --verify HEAD 2>$null | Out-String).Trim()
        if ($LASTEXITCODE -eq 0 -and -not [String]::IsNullOrWhiteSpace($GitCommit)) {
            $GitStatus = @(& $Git.Source -C $Root status --porcelain 2>$null)
            if ($LASTEXITCODE -eq 0) {
                $BuildInfo["git_commit"] = $GitCommit
                $BuildInfo["git_dirty"] = $GitStatus.Count -gt 0
            }
        }
    }

    $BuildInfoPath = Join-Path $Output "build-info.json"
    $BuildInfoJson = $BuildInfo | ConvertTo-Json
    [IO.File]::WriteAllText(
        $BuildInfoPath,
        $BuildInfoJson + [Environment]::NewLine,
        (New-Object Text.UTF8Encoding($false))
    )
    $null = Get-Content -LiteralPath $BuildInfoPath -Raw | ConvertFrom-Json

    Write-Host "Portable artifact: $Destination"
    Write-Host "Package version: $($PackageMetadata.version)"
    Write-Host "SHA-256: $Hash"
    Write-Host "Version smoke test: $VersionSmokeOutput"
    Write-Host "launch help smoke test: $(if ($NoSmokeTest) { 'skipped' } else { 'passed' })"
    Write-Host "Authenticode status: $AuthenticodeStatus"
} finally {
    Pop-Location
}
