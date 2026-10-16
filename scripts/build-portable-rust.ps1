# Rust canonical single-EXE release build for CodexProxyGuard.
# Mirrors scripts/build-portable.ps1: clean tree -> pinned source archive ->
# cargo build --release --locked -> tests -> package verification ->
# immutable promotion from unique staging.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\build-portable-rust.ps1 -AllowUnsignedDev
#   powershell -ExecutionPolicy Bypass -File scripts\build-portable-rust.ps1 -Sign -CertificateSha1 <thumbprint>
param(
    [string]$CertificateSha1 = "",
    [switch]$Sign,
    [switch]$AllowUnsignedDev,
    [string]$Version = "0.8.0-rc.1"
)

$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent $PSScriptRoot

function Fail($message) {
    Write-Host "BUILD FAILED: $message" -ForegroundColor Red
    exit 1
}

if (-not $Sign -and -not $AllowUnsignedDev) {
    Fail "Specify -Sign -CertificateSha1 <thumbprint> for a public release or -AllowUnsignedDev for an unsigned development build."
}
if ($Sign -and [string]::IsNullOrWhiteSpace($CertificateSha1)) {
    Fail "-Sign requires -CertificateSha1 <thumbprint>."
}

Set-Location $Repo

# 1. Clean tree requirement (uncommitted changes would break provenance).
$dirty = git status --porcelain
if ($dirty) {
    Fail "Working tree is dirty; commit or stash before building a release."
}
$commit = git rev-parse HEAD
$shortCommit = $commit.Substring(0, 8)
$tag = "rust-v$Version"
Write-Host "Building $tag from $shortCommit (clean)" -ForegroundColor Cyan

# 2. Source archive pinned to this commit (tracked files only).
$Staging = Join-Path $env:TEMP "cpg-rust-$Version-$shortCommit-$(Get-Random)"
New-Item -ItemType Directory -Path $Staging | Out-Null
try {
    $Archive = Join-Path $Staging "source.zip"
    git archive --format=zip --output=$Archive $commit
    if ($LASTEXITCODE -ne 0) { Fail "git archive failed." }

    # 3. Build from the archived tree, never from the working copy.
    $BuildTree = Join-Path $Staging "src"
    Expand-Archive -Path $Archive -DestinationPath $BuildTree
    Push-Location $BuildTree
    try {
        $env:CPG_PRODUCT_VERSION = $Version
        $env:CPG_BUILD_COMMIT = $commit
        $env:CPG_BUILD_DIRTY = "false"
        # The RUSTFLAGS environment variable REPLACES .cargo/config.toml
        # rustflags, so the release build carries the full hardening set
        # explicitly: CFG codegen, static CRT, CET/CFG/ASLR/DEP link args.
        $env:RUSTFLAGS = "-C control-flow-guard=checks " +
            "-C target-feature=+crt-static " +
            "-C link-arg=/CETCOMPAT " +
            "-C link-arg=/guard:cf " +
            "-C link-arg=/DYNAMICBASE " +
            "-C link-arg=/NXCOMPAT " +
            "-C link-arg=/HIGHENTROPYVA"

        Write-Host "cargo build --release --locked" -ForegroundColor Cyan
        cargo build --release --locked --target x86_64-pc-windows-msvc
        if ($LASTEXITCODE -ne 0) { Fail "cargo build failed." }

        Write-Host "cargo test --release --locked" -ForegroundColor Cyan
        cargo test --workspace --locked --release
        if ($LASTEXITCODE -ne 0) { Fail "cargo test failed." }

        $Exe = Join-Path $BuildTree "target\x86_64-pc-windows-msvc\release\CodexProxyGuard.exe"
        if (-not (Test-Path $Exe)) { Fail "release EXE not found." }

        # 4. Release directory is immutable: refuse existing destination.
        $ReleaseRoot = Join-Path $Repo "dist\releases"
        $ReleaseName = "CodexProxyGuard-$Version-rust-$shortCommit"
        $ReleaseDir = Join-Path $ReleaseRoot $ReleaseName
        if (Test-Path $ReleaseDir) {
            Fail "Release directory already exists: $ReleaseName (releases are immutable)."
        }

        $PackageDir = Join-Path $Staging "package"
        New-Item -ItemType Directory -Path $PackageDir | Out-Null
        Copy-Item $Exe -Destination (Join-Path $PackageDir "CodexProxyGuard.exe")

        # 5. Build info + digests.
        & (Join-Path $PackageDir "CodexProxyGuard.exe") build-info | Out-File -Encoding utf8 (Join-Path $PackageDir "build-info.json")
        $Hash = (Get-FileHash $Exe -Algorithm SHA256).Hash
        "$Hash  CodexProxyGuard.exe" | Out-File -Encoding ascii (Join-Path $PackageDir "SHA256SUMS")
        Copy-Item $Archive -Destination (Join-Path $PackageDir "source.zip")

        # 6. Signing (public releases must be Authenticode-signed).
        $signState = "UNSIGNED DEVELOPMENT BUILD"
        if ($Sign) {
            $signState = "SIGNED"
            $signtool = Get-Command signtool.exe -ErrorAction SilentlyContinue
            if (-not $signtool) {
                $vswhere = "${Env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
                if (Test-Path $vswhere) {
                    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
                    $candidates = Get-ChildItem -Path (Join-Path $vs "Windows Kits\10\bin") -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
                        Where-Object { $_.FullName -match "x64" } | Select-Object -First 1
                    if ($candidates) { $signtool = @{ Source = $candidates.FullName } }
                }
            }
            if (-not $signtool) { Fail "signtool.exe not found for signing." }
            & $signtool.Source sign /fd SHA256 /sha1 $CertificateSha1 /tr http://timestamp.digicert.com /td SHA256 (Join-Path $PackageDir "CodexProxyGuard.exe")
            if ($LASTEXITCODE -ne 0) { Fail "signtool failed." }
        }

        Write-Host "Package state: $signState" -ForegroundColor Yellow
        if ($signState -eq "UNSIGNED DEVELOPMENT BUILD" -and -not $AllowUnsignedDev) {
            Fail "unsigned build requires -AllowUnsignedDev."
        }

        # 7. Immutable promotion with one atomic move.
        New-Item -ItemType Directory -Path $ReleaseRoot -Force | Out-Null
        Move-Item $PackageDir -Destination $ReleaseDir
        Write-Host ""
        Write-Host "RELEASE READY: $ReleaseDir" -ForegroundColor Green
        Write-Host "  EXE SHA256: $Hash"
        Write-Host "  Sign state: $signState"
        Write-Host "  public_release_ready: $(if ($signState -eq 'SIGNED') {'YES'} else {'BLOCKED (unsigned dev build)'})"
    }
    finally {
        Pop-Location
    }
}
finally {
    # Keep staging for inspection on failure; clean on success path above
    # leaves it behind harmlessly under %TEMP%.
}
