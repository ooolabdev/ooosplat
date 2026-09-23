#requires -Version 5.1
#requires -RunAsAdministrator

[CmdletBinding()]
param(
    [ValidateSet('Close', 'Restore')]
    [string]$Action = 'Close'
)

$ErrorActionPreference = 'Stop'

# This list is replaced by the application with the exact names it detected.
$ServiceNames = @('{{SERVICE_NAMES}}')
$AllowedPrefixes = @(
    'ACE-', 'WeGame', 'uunetfilter', 'EasyAntiCheat', 'EAC-',
    'vgk', 'BEDaisy', 'nProtect', 'XunYou'
)

# When copied directly from the repository, discover only the same allowlisted
# driver services. The application replaces the placeholder with exact names.
if ($ServiceNames.Count -eq 1 -and $ServiceNames[0] -eq '{{SERVICE_NAMES}}') {
    $ServiceNames = @(Get-CimInstance Win32_SystemDriver |
        Where-Object { $_.Name -match '^(ACE-|WeGame|uunetfilter|EasyAntiCheat|EAC-|vgk|BEDaisy|nProtect|XunYou)' } |
        Select-Object -ExpandProperty Name)
}

$SystemDirectory = [Environment]::SystemDirectory
$Sc = Join-Path $SystemDirectory 'sc.exe'
$StateDirectory = Join-Path $env:ProgramData 'OOOSplat\GpuConflictScript'
$StateFile = Join-Path $StateDirectory 'restore.json'

function Assert-SafeServiceName {
    param([Parameter(Mandatory = $true)][string]$Name)

    if ($Name -notmatch '^[A-Za-z0-9_.-]{1,64}$') {
        throw "Unsafe service name: $Name"
    }
    $matched = $false
    foreach ($prefix in $AllowedPrefixes) {
        if ($Name.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
            $matched = $true
            break
        }
    }
    if (-not $matched) { throw "Service is outside the OOOSplat conflict allowlist: $Name" }
}

function Invoke-Sc {
    param([Parameter(Mandatory = $true)][string[]]$Arguments)

    $output = & $Sc @Arguments 2>&1 | Out-String
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "sc.exe $($Arguments -join ' ') failed with exit code $exitCode`n$output"
    }
    $output
}

function Get-StartType {
    param([Parameter(Mandatory = $true)][string]$Name)

    $output = Invoke-Sc @('qc', $Name)
    $match = [regex]::Match($output, 'START_TYPE\s*:\s*(\d+)')
    if (-not $match.Success) { throw "Could not read START_TYPE for $Name" }
    switch ($match.Groups[1].Value) {
        '0' { 'boot'; break }
        '1' { 'system'; break }
        '2' { 'auto'; break }
        '3' { 'demand'; break }
        '4' { 'disabled'; break }
        default { throw "Unknown START_TYPE for $Name" }
    }
}

function Get-StateCode {
    param([Parameter(Mandatory = $true)][string]$Name)

    $output = Invoke-Sc @('query', $Name)
    $match = [regex]::Match($output, 'STATE\s*:\s*(\d+)')
    if (-not $match.Success) { throw "Could not read STATE for $Name" }
    $match.Groups[1].Value
}

function Confirm-Action {
    param([Parameter(Mandatory = $true)][string]$Expected)

    Write-Warning 'This script changes third-party kernel-driver service configuration.'
    Write-Warning 'It may stop games, anti-cheat, accelerators, network filters, or other software.'
    Write-Warning 'Use the vendor''s own repair/uninstall flow when available.'
    $answer = Read-Host "Type $Expected exactly to continue"
    if ($answer -cne $Expected) {
        Write-Host 'Cancelled. No service was changed.'
        exit 1
    }
}

foreach ($name in $ServiceNames) { Assert-SafeServiceName $name }
if (-not (Test-Path -LiteralPath $Sc -PathType Leaf)) { throw "System sc.exe not found: $Sc" }

if ($Action -eq 'Close') {
    Confirm-Action 'CLOSE'
    $entries = @()
    foreach ($name in $ServiceNames) {
        try {
            $startType = Get-StartType $name
            $state = Get-StateCode $name
            $entries += [pscustomobject]@{
                Name = $name
                StartType = $startType
                PreviousState = $state
            }
        } catch {
            Write-Warning $_
        }
    }
    if ($entries.Count -eq 0) { throw 'None of the detected services could be queried.' }

    New-Item -ItemType Directory -Path $StateDirectory -Force | Out-Null
    $entries | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $StateFile -Encoding UTF8

    foreach ($entry in $entries) {
        $state = [int](Get-StateCode $entry.Name)
        if ($state -ne 1) {
            Invoke-Sc @('stop', $entry.Name) | Out-Null
            for ($attempt = 0; $attempt -lt 20; $attempt++) {
                Start-Sleep -Milliseconds 250
                if ([int](Get-StateCode $entry.Name) -eq 1) { break }
            }
        }
        if ([int](Get-StateCode $entry.Name) -ne 1) {
            throw "Could not stop $($entry.Name); its start type was not changed."
        }
        Invoke-Sc @('config', $entry.Name, 'start=', 'disabled') | Out-Null
        Write-Host "Closed $($entry.Name) (original start type: $($entry.StartType))."
    }
    Write-Host "Backup written to $StateFile. Run this script with -Action Restore to undo the start-type changes."
    exit 0
}

Confirm-Action 'RESTORE'
if (-not (Test-Path -LiteralPath $StateFile -PathType Leaf)) {
    throw "No restore backup found at $StateFile"
}
$entries = @(Get-Content -LiteralPath $StateFile -Raw | ConvertFrom-Json)
foreach ($entry in $entries) {
    Assert-SafeServiceName $entry.Name
    if ($entry.StartType -notin @('boot', 'system', 'auto', 'demand', 'disabled')) {
        throw "Unsafe restore start type for $($entry.Name)"
    }
    Invoke-Sc @('config', $entry.Name, 'start=', $entry.StartType) | Out-Null
    Write-Host "Restored $($entry.Name) to $($entry.StartType)."
}
Remove-Item -LiteralPath $StateFile -Force
Write-Host 'Restore completed. A reboot may be required before a kernel driver is loaded again.'
