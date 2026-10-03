# GUI and engine share the canonical C++ portable build.
& (Join-Path $PSScriptRoot 'build-portable.ps1') @args
if ($LASTEXITCODE) { exit $LASTEXITCODE }
