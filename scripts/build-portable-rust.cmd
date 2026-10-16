@echo off
rem Canonical Rust single-EXE release build (see scripts/build-portable-rust.ps1).
rem Public releases: powershell -ExecutionPolicy Bypass -File scripts\build-portable-rust.ps1 -Sign -CertificateSha1 ^<thumbprint^>
rem Development builds: add -AllowUnsignedDev (output is marked unsigned).
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-portable-rust.ps1" %*
exit /b %ERRORLEVEL%
