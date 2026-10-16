[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Write-Host "Installing DmuxAI Windows Launcher..."

$sourceDir = $PSScriptRoot
$installDir = Join-Path -Path $env:LOCALAPPDATA -ChildPath "Programs\DmuxAI"

if (-not (Test-Path -LiteralPath $installDir)) {
    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
}

$launcherSource = Join-Path -Path $sourceDir -ChildPath "dmuxai-launcher.exe"
$dmuxaiSource = Join-Path -Path $sourceDir -ChildPath "dmuxai.exe"
$uninstallSource = Join-Path -Path $sourceDir -ChildPath "uninstall.ps1"

if (-not (Test-Path -LiteralPath $launcherSource)) {
    Write-Error "Required executable 'dmuxai-launcher.exe' was not found in '$sourceDir'."
    exit 1
}

Copy-Item -LiteralPath $launcherSource -Destination $installDir -Force
Write-Host "Copied dmuxai-launcher.exe to $installDir"

if (Test-Path -LiteralPath $dmuxaiSource) {
    Copy-Item -LiteralPath $dmuxaiSource -Destination $installDir -Force
    Write-Host "Copied dmuxai.exe to $installDir"
}

if (Test-Path -LiteralPath $uninstallSource) {
    Copy-Item -LiteralPath $uninstallSource -Destination $installDir -Force
}

$programsPath = [Environment]::GetFolderPath("Programs")
if (-not (Test-Path -LiteralPath $programsPath)) {
    New-Item -ItemType Directory -Path $programsPath -Force | Out-Null
}

$shortcutPath = Join-Path -Path $programsPath -ChildPath "DmuxAI.lnk"
$targetExe = Join-Path -Path $installDir -ChildPath "dmuxai-launcher.exe"

$wscript = New-Object -ComObject WScript.Shell
$shortcut = $wscript.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $targetExe
$shortcut.WorkingDirectory = [Environment]::GetFolderPath("UserProfile")
$shortcut.Description = "DmuxAI desktop launcher (runs inside WSL)"
$shortcut.Save()

Write-Host "Created Start Menu shortcut: $shortcutPath"
Write-Host "DmuxAI installation completed successfully."
