@echo off
rem Starts SulcusAI from the code (see dev.ps1). Double-click it, or run it
rem from a terminal; add -TestData for a separate test folder.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0dev.ps1" %*
pause
