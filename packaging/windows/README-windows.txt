DmuxAI Windows Desktop Launcher
===============================

DmuxAI runs AI coding agents (Cursor, AGY, OpenCode, Codex) inside WSL 2
while providing a seamless Windows desktop experience.

Installation:
1. Extract the release zip file (dmuxai-windows-x86_64.zip).
2. Right-click 'install.ps1' and select "Run with PowerShell"
   (or open PowerShell in this directory and run:
    powershell -ExecutionPolicy Bypass -File .\install.ps1)

This will:
- Copy dmuxai-launcher.exe and dmuxai.exe to:
  %LOCALAPPDATA%\Programs\DmuxAI
- Create a Start Menu shortcut named "DmuxAI" pointing to dmuxai-launcher.exe.
- No Administrator rights, Rust, Cargo, or Git are required on Windows.

Requirements:
- Windows 10/11 with WSL 2 and at least one Linux distribution (e.g., Ubuntu).
- DmuxAI installed inside the WSL distribution (git clone https://github.com/donggrri/dmuxai && cd dmuxai && cargo install --path .).
- Windows Terminal (recommended; falls back to default console if absent).

Uninstallation:
- Run 'uninstall.ps1' from PowerShell:
  powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
- This removes the Start Menu shortcut and %LOCALAPPDATA%\Programs\DmuxAI.

Installer Note:
- MSI installer is currently not available for this release; the zip package
  with install.ps1 is the official Windows distribution format.
