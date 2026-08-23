[CmdletBinding(SupportsShouldProcess)]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSCommandPath
if ([string]::IsNullOrWhiteSpace($repositoryRoot)) {
    throw 'Could not determine the DmuxAI repository directory.'
}

$cargoHome = if ([string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
    Join-Path $env:USERPROFILE '.cargo'
}
else {
    $env:CARGO_HOME
}
$cargoHome = [System.IO.Path]::GetFullPath($cargoHome)
$cargoBin = Join-Path $cargoHome 'bin'

$cargoCommand = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
if ($null -ne $cargoCommand) {
    $cargoPath = $cargoCommand.Source
}
else {
    $cargoPath = Join-Path $cargoBin 'cargo.exe'
    if (-not (Test-Path -LiteralPath $cargoPath -PathType Leaf)) {
        throw "Cargo was not found. Install Rust with rustup, then run this script again. Expected Cargo at '$cargoPath'."
    }
}

if ($PSCmdlet.ShouldProcess($cargoBin, "install DmuxAI from '$repositoryRoot'")) {
    & $cargoPath install --path $repositoryRoot --locked --force
    if ($LASTEXITCODE -ne 0) {
        throw "DmuxAI installation failed with exit code $LASTEXITCODE."
    }

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $userEntries = @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $hasCargoBin = @($userEntries | Where-Object {
        [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\\') -ieq $cargoBin.TrimEnd('\\')
    })
    if ($hasCargoBin.Count -eq 0) {
        $newUserPath = @($userEntries + $cargoBin) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
        Write-Host "Added '$cargoBin' to your user PATH."
    }

    $currentEntries = @($env:Path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $hasCurrentCargoBin = @($currentEntries | Where-Object {
        [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\\') -ieq $cargoBin.TrimEnd('\\')
    })
    if ($hasCurrentCargoBin.Count -eq 0) {
        $env:Path = @($currentEntries + $cargoBin) -join ';'
    }

    Write-Host 'DmuxAI is installed. In any project folder, run: dmuxai'
}
