@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-portable.ps1" %*
set "PORTABLE_EXIT_CODE=%ERRORLEVEL%"
endlocal & exit /b %PORTABLE_EXIT_CODE%
