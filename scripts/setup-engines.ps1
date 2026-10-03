[CmdletBinding()]
param(
  [switch]$Force,
  [string]$CacheDirectory
)

$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $workspace 'engines\manifest.json'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
  throw "Missing engine manifest: $manifestPath"
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
# Validate/install COLMAP first so an unbuilt release cannot partially update
# the other engines or silently accept the previously downloaded official zip.
& node (Join-Path $PSScriptRoot 'setup-colmap-runtime.mjs') windows
if ($LASTEXITCODE -ne 0) { throw 'Locked COLMAP runtime installation failed.' }
if (-not $CacheDirectory) {
  $CacheDirectory = Join-Path $workspace '.cache\engines'
}
$CacheDirectory = [System.IO.Path]::GetFullPath($CacheDirectory)
New-Item -ItemType Directory -Path $CacheDirectory -Force | Out-Null

function Assert-WorkspaceDestination([string]$RelativePath) {
  $full = [System.IO.Path]::GetFullPath((Join-Path $workspace $RelativePath))
  $root = [System.IO.Path]::GetFullPath($workspace).TrimEnd('\') + '\'
  if (-not $full.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Engine destination escapes the workspace: $RelativePath"
  }
  return $full
}

function Test-RequiredFiles($Engine, [string]$Destination) {
  $prefix = ($Destination.Replace('\', '/').TrimEnd('/') + '/')
  $required = @($manifest.requiredFiles | Where-Object { $_.path.Replace('\', '/').StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase) })
  if ($required.Count -eq 0) { return $false }
  foreach ($item in $required) {
    $path = Join-Path $workspace $item.path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return $false }
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $item.sha256) { return $false }
  }
  return $true
}

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

foreach ($engine in $manifest.engines) {
  if ($engine.name -eq 'COLMAP' -or $engine.name -eq 'Brush') { continue }
  $install = $engine.install
  if (-not $install) { throw "Engine '$($engine.name)' has no install configuration." }

  if (-not $Force -and (Test-RequiredFiles $engine $install.destination)) {
    Write-Host "Ready: $($engine.name)"
    continue
  }

  $archivePath = Join-Path $CacheDirectory $install.archiveName
  $download = $true
  if (Test-Path -LiteralPath $archivePath -PathType Leaf) {
    $download = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash -ne $engine.archiveSha256
    if ($download) {
      Write-Warning "Cached archive hash changed; downloading a clean copy for $($engine.name)."
      Remove-Item -LiteralPath $archivePath -Force
    }
  }

  if ($download) {
    Write-Host "Downloading $($engine.name)..."
    Invoke-WebRequest -Uri $engine.sourceUrl -OutFile $archivePath -UseBasicParsing
  } else {
    Write-Host "Using cached archive: $($install.archiveName)"
  }

  $archiveHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
  if ($archiveHash -ne $engine.archiveSha256) {
    throw "Archive hash mismatch for $($engine.name). Expected $($engine.archiveSha256), got $archiveHash. The upstream asset may have changed; update manifest.json only after reviewing the new release."
  }

  $temporary = Join-Path $workspace ('.tmp\engine-setup-' + [guid]::NewGuid().ToString('N'))
  try {
    New-Item -ItemType Directory -Path $temporary -Force | Out-Null
    Expand-Archive -LiteralPath $archivePath -DestinationPath $temporary -Force
    $anchors = @(Get-ChildItem -LiteralPath $temporary -Recurse -File | Where-Object { $_.Name -eq $install.anchorFile })
    if ($anchors.Count -ne 1) {
      throw "Expected one '$($install.anchorFile)' in $($install.archiveName), found $($anchors.Count)."
    }

    $sourceRoot = $anchors[0].Directory
    for ($level = 0; $level -lt [int]$install.rootFromAnchorParent; $level++) {
      $sourceRoot = $sourceRoot.Parent
      if (-not $sourceRoot) { throw "Invalid extraction root for $($engine.name)." }
    }

    $destination = Assert-WorkspaceDestination $install.destination
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Get-ChildItem -LiteralPath $destination -Force |
      Where-Object { $_.Name -ne 'README.md' } |
      Remove-Item -Recurse -Force
    Get-ChildItem -LiteralPath $sourceRoot.FullName -Force |
      ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $destination -Recurse -Force }
    Write-Host "Installed: $($engine.name) -> $($install.destination)"
  } finally {
    if (Test-Path -LiteralPath $temporary) {
      Remove-Item -LiteralPath $temporary -Recurse -Force
    }
  }
}

$brushArguments = @('prepare', 'windows', '--cache', $CacheDirectory)
if ($Force) { $brushArguments += '--force' }
& node (Join-Path $PSScriptRoot 'brush-runtime.mjs') @brushArguments
if ($LASTEXITCODE -ne 0) { throw 'Pinned OOOBrush installation failed.' }

& (Join-Path $PSScriptRoot 'verify-engines.ps1')
