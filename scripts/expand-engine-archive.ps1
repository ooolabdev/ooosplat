[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$ArchivePath,
  [Parameter(Mandatory = $true)][string]$DestinationPath
)

$ErrorActionPreference = 'Stop'
$archive = [System.IO.Path]::GetFullPath($ArchivePath)
$destination = [System.IO.Path]::GetFullPath($DestinationPath)

# Engine setup extracts only into a fresh staging directory, never over an
# installed engine. Preserve that contract even when this helper is run alone.
if (Test-Path -LiteralPath $destination) {
  $item = Get-Item -LiteralPath $destination -Force
  if (-not $item.PSIsContainer -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
    throw 'Engine archive destination must be an ordinary empty directory.'
  }
  if (@(Get-ChildItem -LiteralPath $destination -Force | Select-Object -First 1).Count -ne 0) {
    throw 'Engine archive destination must be empty.'
  }
}

# Windows PowerShell 5.1 uses .NET Framework, whose ZipFile API does not have
# the (source, destination, overwrite) overload used by some Archive modules.
# The two-argument API works there and in PowerShell 7; fresh staging needs no
# overwrite flag. Do not load Microsoft.PowerShell.Archive / Expand-Archive.
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::ExtractToDirectory($archive, $destination)
