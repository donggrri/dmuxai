[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Write-Host "Uninstalling DmuxAI Windows Launcher..."

$programsPath = [Environment]::GetFolderPath("Programs")
$shortcutPath = Join-Path -Path $programsPath -ChildPath "DmuxAI.lnk"

if (Test-Path -LiteralPath $shortcutPath) {
    Remove-Item -LiteralPath $shortcutPath -Force
    Write-Host "Removed Start Menu shortcut: $shortcutPath"
}

$installDir = Join-Path -Path $env:LOCALAPPDATA -ChildPath "Programs\DmuxAI"

if (Test-Path -LiteralPath $installDir) {
    Remove-Item -LiteralPath $installDir -Recurse -Force
    Write-Host "Removed installation directory: $installDir"
}

Write-Host "DmuxAI uninstallation completed successfully."
