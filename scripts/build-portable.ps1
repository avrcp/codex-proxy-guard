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

    # Capture the exact build provenance before compiling and hand it to the
    # build script, so the embedded --build-info commit/dirty describe this
    # source tree rather than whatever Cargo may have cached.
    $Git = Get-Command git.exe -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    $BuildCommit = $null
    $BuildDirty = $null
    $GitStatusBefore = $null
    if ($Git -and (Test-Path -LiteralPath (Join-Path $Root ".git"))) {
        $BuildCommit = (& $Git.Source -C $Root rev-parse --verify HEAD 2>$null | Out-String).Trim()
        if ($LASTEXITCODE -ne 0 -or [String]::IsNullOrWhiteSpace($BuildCommit)) {
            throw "Cannot determine the git commit of $Root; a portable artifact must record its source."
        }
        $GitStatusBefore = @(& $Git.Source -C $Root status --porcelain 2>$null)
        if ($LASTEXITCODE -ne 0) {
            throw "Cannot determine whether $Root is dirty; a portable artifact must record a proven dirty flag."
        }
        $BuildDirty = [bool]($GitStatusBefore.Count -gt 0)
        if ($BuildDirty) {
            Write-Warning "The working tree is dirty; the artifact will be marked dirty=true and is not a clean-HEAD release."
        }
    } else {
        throw "No .git repository found at $Root; a portable artifact must be built from version control."
    }

    Write-Host "Building $Package $($PackageMetadata.version) in release mode..."
    $PreviousCommit = $Env:CPG_BUILD_COMMIT
    $PreviousDirty = $Env:CPG_BUILD_DIRTY
    $Env:CPG_BUILD_COMMIT = $BuildCommit
    $Env:CPG_BUILD_DIRTY = if ($BuildDirty) { "true" } else { "false" }
    try {
        & $Cargo.Source build --release --locked --target $Target -p $Package
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
    } finally {
        if ($null -ne $PreviousCommit) {
            $Env:CPG_BUILD_COMMIT = $PreviousCommit
        } else {
            Remove-Item Env:CPG_BUILD_COMMIT -ErrorAction SilentlyContinue
        }
        if ($null -ne $PreviousDirty) {
            $Env:CPG_BUILD_DIRTY = $PreviousDirty
        } else {
            Remove-Item Env:CPG_BUILD_DIRTY -ErrorAction SilentlyContinue
        }
    }

    # The build itself must not have modified tracked sources.
    $GitStatusAfter = @(& $Git.Source -C $Root status --porcelain 2>$null)
    if ($LASTEXITCODE -ne 0 -or (@($GitStatusAfter).Count -gt @($GitStatusBefore).Count)) {
        throw "The working tree changed during the build; the artifact provenance is no longer trustworthy."
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
        $BuildInfoSmokeOutput = "skipped (-NoSmokeTest)"
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

        Write-Host "Running side-effect-free smoke test: $PortableName build-info"
        $BuildInfoLines = @(& $Destination build-info 2>&1 | ForEach-Object { $_.ToString() })
        $BuildInfoExitCode = $LASTEXITCODE
        $BuildInfoSmokeOutput = ($BuildInfoLines -join [Environment]::NewLine).Trim()
        if ($BuildInfoExitCode -ne 0) {
            throw "Portable build-info smoke test failed with exit code $BuildInfoExitCode"
        }
        try {
            $EmbeddedInfo = $BuildInfoSmokeOutput | ConvertFrom-Json
        } catch {
            throw "Portable build-info smoke output is not JSON: $BuildInfoSmokeOutput"
        }
        if ([string]$EmbeddedInfo.commit -ne $BuildCommit) {
            throw ("Embedded build-info commit '{0}' does not match the source commit '{1}'. " -f
                [string]$EmbeddedInfo.commit, $BuildCommit) +
                "The binary was probably served from a stale Cargo cache; clean and rebuild."
        }
        if ([bool]$EmbeddedInfo.dirty -ne $BuildDirty) {
            throw ("Embedded build-info dirty '{0}' does not match the recorded '{1}'." -f
                [string]$EmbeddedInfo.dirty, [string]$BuildDirty)
        }
        if ([string]$EmbeddedInfo.version -ne [string]$PackageMetadata.version) {
            throw ("Embedded build-info version '{0}' does not match Cargo package version '{1}'." -f
                [string]$EmbeddedInfo.version, [string]$PackageMetadata.version)
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

        # The hidden activation worker must reject malformed input before COM
        # is ever initialized — proven here without activating anything. The
        # rejection is written to stderr, which must not abort this script.
        Write-Host "Running side-effect-free smoke test: worker rejects malformed requests"
        $PreviousEap = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $WorkerOutput = "not json" | & $Destination internal-activate-package 2>&1
        } finally {
            $ErrorActionPreference = $PreviousEap
        }
        $WorkerExitCode = $LASTEXITCODE
        if ($WorkerExitCode -eq 0) {
            throw "The activation worker accepted malformed stdin; it must refuse before any activation"
        }
        $WorkerText = (@($WorkerOutput) | ForEach-Object { $_.ToString() }) -join " "
        if ($WorkerText -notmatch "APPX_ACTIVATION_PROTOCOL_INVALID") {
            throw "Unexpected activation worker rejection output: $WorkerText"
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
            build_info = $BuildInfoSmokeOutput
            worker_rejects_malformed_input = "passed"
        }
        authenticode_status = $AuthenticodeStatus
        built_at_utc = [DateTime]::UtcNow.ToString("o")
        embedded_commit = $BuildCommit
        embedded_dirty = $BuildDirty
    }

    # The provenance captured before the build is authoritative: it is what
    # the binary itself reports via --build-info (verified in the smoke test).
    $BuildInfo["git_commit"] = $BuildCommit
    $BuildInfo["git_dirty"] = $BuildDirty

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
    Write-Host "Source commit: $BuildCommit (dirty: $BuildDirty)"
    Write-Host "Version smoke test: $VersionSmokeOutput"
    Write-Host "launch help smoke test: $(if ($NoSmokeTest) { 'skipped' } else { 'passed' })"
    Write-Host "build-info smoke test: $(if ($NoSmokeTest) { 'skipped' } else { 'passed' })"
    Write-Host "Authenticode status: $AuthenticodeStatus"
} finally {
    Pop-Location
}
