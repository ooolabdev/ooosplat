$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $workspace 'engines\manifest.json'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "Missing engine manifest: $manifestPath" }
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json

function Get-Sha256([string]$Path) {
  $stream = [System.IO.File]::OpenRead($Path)
  try {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
      return ([System.BitConverter]::ToString($sha256.ComputeHash($stream))).Replace('-', '')
    } finally {
      $sha256.Dispose()
    }
  } finally {
    $stream.Dispose()
  }
}

$colmapManifest = $manifest.engines | Where-Object { $_.name -eq 'COLMAP' } | Select-Object -First 1
if (-not $colmapManifest) { throw 'Engine manifest is missing the COLMAP entry.' }
$integrityPin = ($manifest.requiredFiles | Where-Object { $_.path -eq 'engines/colmap/SHA256SUMS' }).sha256
if ($colmapManifest.archiveSha256 -notmatch '^[a-fA-F0-9]{64}$' -or $integrityPin -notmatch '^[a-fA-F0-9]{64}$') { throw 'New COLMAP runtime has not been built, reviewed and hash-locked yet.' }
$cudaCompatibility = $colmapManifest.cudaCompatibility
if (-not $cudaCompatibility) { throw 'COLMAP manifest entry is missing cudaCompatibility.' }
if ($cudaCompatibility.toolkitVersion -ne '13.2.0') { throw 'COLMAP CUDA toolkitVersion must be 13.2.0 for the locked release.' }
if ($cudaCompatibility.architecturePolicy -ne '75;80;86;89;90;100;120') { throw 'COLMAP CUDA architecture policy mismatch.' }
if ($cudaCompatibility.minimumWindowsDriver -notmatch '^\d+\.\d+$') { throw 'COLMAP minimumWindowsDriver is invalid.' }
if ($cudaCompatibility.minimumComputeCapability -notmatch '^\d+\.\d+$') { throw 'COLMAP minimumComputeCapability is invalid.' }
foreach ($item in $manifest.requiredFiles) {
  $path = Join-Path $workspace $item.path
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing engine file: $($item.path). Run 'npm run setup:engines' first." }
  $actual = Get-Sha256 $path
  if ($actual -ne $item.sha256) { throw "Hash mismatch for $($item.path): $actual" }
}
$integrityPin = ($manifest.requiredFiles | Where-Object { $_.path -eq 'engines/colmap/SHA256SUMS' }).sha256
& node (Join-Path $PSScriptRoot 'colmap-runtime.mjs') (Join-Path $workspace 'engines/colmap') windows $integrityPin release
if ($LASTEXITCODE -ne 0) { throw 'COLMAP source, runtime hashes, CLI or Caspar verification failed.' }
$brush = Join-Path $workspace 'engines\brush\brush_app.exe'
$savedPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
& node (Join-Path $PSScriptRoot 'brush-runtime.mjs') verify windows
if ($LASTEXITCODE -ne 0) { throw 'OOOBrush CLI, provenance or dependency verification failed.' }
$brushHelp = & $brush --help 2>&1 | Out-String
$brushExit = $LASTEXITCODE
$ErrorActionPreference = $savedPreference
if ($brushExit -ne 0) { throw "Bundled Brush help failed with exit code $brushExit" }
foreach ($flag in '--total-train-iters','--max-resolution','--export-every','--export-path','--export-name') {
  if ($brushHelp -notmatch [regex]::Escape($flag)) { throw "Bundled Brush is missing $flag" }
}
Write-Host "Verified $($manifest.requiredFiles.Count) locked engine files; COLMAP CUDA and Brush CLI are valid."
