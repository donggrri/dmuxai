[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$Repository = 'donggrri/dmuxai',
    [string]$InstallDirectory = (Join-Path $env:LOCALAPPDATA 'DmuxAI\bin'),
    [switch]$SkipProviderCheck
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$releaseApi = "https://api.github.com/repos/$Repository/releases/latest"
$assetName = 'dmuxai-windows-x86_64.zip'
$checksumName = "$assetName.sha256"
$requestHeaders = @{ Accept = 'application/vnd.github+json'; 'User-Agent' = 'DmuxAI-Installer' }

function Get-ReleaseAsset {
    param(
        [Parameter(Mandatory)] [object]$Release,
        [Parameter(Mandatory)] [string]$Name
    )

    $matches = @($Release.assets | Where-Object { $_.name -eq $Name })
    if ($matches.Count -ne 1) {
        throw "The latest DmuxAI release does not contain '$Name'."
    }

    return $matches[0]
}

function Add-UserPathEntry {
    param([Parameter(Mandatory)] [string]$Entry)

    $normalizedEntry = [System.IO.Path]::GetFullPath($Entry).TrimEnd('\\')
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $alreadyPresent = @($entries | Where-Object {
        try {
            [System.IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($_)).TrimEnd('\\') -ieq $normalizedEntry
        }
        catch {
            $false
        }
    }).Count -gt 0

    if (-not $alreadyPresent) {
        [Environment]::SetEnvironmentVariable('Path', (@($entries + $Entry) -join ';'), 'User')
        Write-Host "Added '$Entry' to your user PATH."
    }

    $processEntries = @($env:Path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $inCurrentProcess = @($processEntries | Where-Object {
        try {
            [System.IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($_)).TrimEnd('\\') -ieq $normalizedEntry
        }
        catch {
            $false
        }
    }).Count -gt 0
    if (-not $inCurrentProcess) {
        $env:Path = @($processEntries + $Entry) -join ';'
    }
}

function Show-ProviderStatus {
    param([Parameter(Mandatory)] [string]$Name, [Parameter(Mandatory)] [string]$Command)

    $program = Get-Command $Command -CommandType Application -ErrorAction SilentlyContinue
    if ($null -eq $program) {
        Write-Host "[MISSING] $Name ($Command)"
        return
    }

    Write-Host "[FOUND]   $Name ($($program.Source))"
}

try {
    $release = Invoke-RestMethod -Uri $releaseApi -Headers $requestHeaders
    $archiveAsset = Get-ReleaseAsset -Release $release -Name $assetName
    $checksumAsset = Get-ReleaseAsset -Release $release -Name $checksumName
}
catch {
    throw "Could not find the latest DmuxAI GitHub release. $($_.Exception.Message)"
}

$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ("DmuxAI-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null

try {
    $archivePath = Join-Path $temporaryDirectory $assetName
    $checksumPath = Join-Path $temporaryDirectory $checksumName
    Invoke-WebRequest -Uri $archiveAsset.browser_download_url -Headers $requestHeaders -OutFile $archivePath
    Invoke-WebRequest -Uri $checksumAsset.browser_download_url -Headers $requestHeaders -OutFile $checksumPath

    $expectedHash = ((Get-Content -LiteralPath $checksumPath -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
    if ($expectedHash -notmatch '^[0-9a-f]{64}$') {
        throw "The release checksum file '$checksumName' is invalid."
    }
    $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw 'The downloaded DmuxAI archive did not match its published SHA-256 checksum.'
    }

    $extractDirectory = Join-Path $temporaryDirectory 'extracted'
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDirectory -Force
    $sourceExecutable = Join-Path $extractDirectory 'dmuxai.exe'
    if (-not (Test-Path -LiteralPath $sourceExecutable -PathType Leaf)) {
        throw "The DmuxAI archive does not contain 'dmuxai.exe' at its root."
    }

    $destinationExecutable = Join-Path $InstallDirectory 'dmuxai.exe'
    if ($PSCmdlet.ShouldProcess($destinationExecutable, "install DmuxAI $($release.tag_name)")) {
        New-Item -ItemType Directory -Path $InstallDirectory -Force | Out-Null
        Copy-Item -LiteralPath $sourceExecutable -Destination $destinationExecutable -Force
        Add-UserPathEntry -Entry $InstallDirectory
        Write-Host "DmuxAI $($release.tag_name) is installed. Run 'dmuxai' from any project folder."
    }
}
finally {
    Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not $SkipProviderCheck) {
    Write-Host ''
    Write-Host 'Coding-agent CLI availability:'
    Show-ProviderStatus -Name 'Codex' -Command 'codex'
    Show-ProviderStatus -Name 'Cursor Agent' -Command 'cursor-agent'
    Show-ProviderStatus -Name 'AGY' -Command 'agy'
    Show-ProviderStatus -Name 'OpenCode' -Command 'opencode'
    Show-ProviderStatus -Name 'Claude Code' -Command 'claude'
    Show-ProviderStatus -Name 'Pi' -Command 'pi'
}
