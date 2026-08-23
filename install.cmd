@echo off
rem Install the latest published DmuxAI Windows release for the current user.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
if errorlevel 1 exit /b %errorlevel%

set "DMUXAI_BIN=%LOCALAPPDATA%\DmuxAI\bin"
set "PATH=%PATH%;%DMUXAI_BIN%"
echo DmuxAI is ready in this Command Prompt. Run: dmuxai
