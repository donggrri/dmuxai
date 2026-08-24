[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })]
    [string]$SpecPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$content = Get-Content -LiteralPath $SpecPath -Raw
$problems = [System.Collections.Generic.List[string]]::new()

function Get-Section {
    param(
        [Parameter(Mandatory)] [string]$Document,
        [Parameter(Mandatory)] [string]$HeadingPattern
    )

    $match = [regex]::Match(
        $Document,
        "(?ms)^## $HeadingPattern\s*\r?\n(.*?)(?=^## |\z)"
    )
    if (-not $match.Success) {
        return $null
    }
    return $match.Groups[1].Value
}

$requirements = Get-Section -Document $content -HeadingPattern '(?:2\. Functional requirements|4\. Requirements)'
$scenarios = Get-Section -Document $content -HeadingPattern '(?:4\. Acceptance scenarios and current evidence|5\. Acceptance scenarios and test design)'

if ($null -eq $requirements) {
    $problems.Add('Missing a Requirements section with a supported heading.')
}
if ($null -eq $scenarios) {
    $problems.Add('Missing an Acceptance scenarios section with a supported heading.')
}

if ($problems.Count -eq 0) {
    $requirementRows = [regex]::Matches(
        $requirements,
        '(?m)^\|\s*((?:FR|BR|UX|INT|NFR)-\d+)\s*\|\s*([^|]+)\|'
    )
    if ($requirementRows.Count -eq 0) {
        $problems.Add('No requirement rows with stable IDs were found.')
    }

    $definedIds = @($requirementRows | ForEach-Object { $_.Groups[1].Value })
    $duplicateIds = @($definedIds | Group-Object | Where-Object Count -gt 1 | ForEach-Object Name)
    foreach ($id in $duplicateIds) {
        $problems.Add("Requirement ID $id is defined more than once.")
    }

    $testRows = [regex]::Matches($scenarios, '(?m)^\|\s*(TC-\d+)\s*\|')
    if ($testRows.Count -eq 0) {
        $problems.Add('No acceptance scenario rows with TC-* IDs were found.')
    }
    $testIds = @($testRows | ForEach-Object { $_.Groups[1].Value })
    $duplicateTestIds = @($testIds | Group-Object | Where-Object Count -gt 1 | ForEach-Object Name)
    foreach ($id in $duplicateTestIds) {
        $problems.Add("Test ID $id is defined more than once.")
    }

    foreach ($row in $requirementRows) {
        $id = $row.Groups[1].Value
        $priority = $row.Groups[2].Value.Trim()
        if ($priority -eq 'Must' -and $scenarios -notmatch "\b$([regex]::Escape($id))\b") {
            $problems.Add("Must requirement $id has no acceptance-scenario mapping.")
        }
    }
}

if ($problems.Count -gt 0) {
    $problems | ForEach-Object { Write-Error "SPEC CHECK: $_" }
    exit 1
}

Write-Host "SPEC CHECK: PASS ($SpecPath)"
