@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-gui.ps1" %*
set "GUI_EXIT_CODE=%ERRORLEVEL%"
endlocal & exit /b %GUI_EXIT_CODE%
