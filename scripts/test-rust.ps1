# Development gate: format check, clippy -D warnings, all workspace tests.
$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent $PSScriptRoot
Set-Location $Repo

Write-Host "== cargo fmt --check ==" -ForegroundColor Cyan
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { Write-Host "FORMAT FAILED" -ForegroundColor Red; exit 1 }

Write-Host "== cargo clippy -D warnings ==" -ForegroundColor Cyan
cargo clippy --workspace --all-targets --locked -- -D warnings
if ($LASTEXITCODE -ne 0) { Write-Host "CLIPPY FAILED" -ForegroundColor Red; exit 1 }

Write-Host "== cargo test ==" -ForegroundColor Cyan
cargo test --workspace --locked
if ($LASTEXITCODE -ne 0) { Write-Host "TESTS FAILED" -ForegroundColor Red; exit 1 }

Write-Host "== git diff --check ==" -ForegroundColor Cyan
git diff --check
if ($LASTEXITCODE -ne 0) { Write-Host "WHITESPACE FAILED" -ForegroundColor Red; exit 1 }

Write-Host "ALL RUST GATES PASSED" -ForegroundColor Green
