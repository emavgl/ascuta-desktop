@echo off
rem Double-click friendly wrapper: runs install.ps1 without touching the
rem machine-wide PowerShell execution policy.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
if errorlevel 1 pause
