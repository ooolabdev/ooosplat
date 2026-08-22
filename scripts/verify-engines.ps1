$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $workspace 'engines\manifest.json'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "Missing engine manifest: $manifestPath" }
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$colmapManifest = $manifest.engines | Where-Object { $_.name -eq 'COLMAP' } | Select-Object -First 1
if (-not $colmapManifest) { throw 'Engine manifest is missing the COLMAP entry.' }
$cudaCompatibility = $colmapManifest.cudaCompatibility
if (-not $cudaCompatibility) { throw 'COLMAP manifest entry is missing cudaCompatibility.' }
if ($cudaCompatibility.toolkitVersion -ne '12.9.1') { throw 'COLMAP CUDA toolkitVersion must be 12.9.1 for the locked release.' }
if ($cudaCompatibility.architecturePolicy -ne 'all-major') { throw 'COLMAP architecturePolicy must be all-major.' }
if ($cudaCompatibility.minimumWindowsDriver -notmatch '^\d+\.\d+$') { throw 'COLMAP minimumWindowsDriver is invalid.' }
if ($cudaCompatibility.minimumComputeCapability -notmatch '^\d+\.\d+$') { throw 'COLMAP minimumComputeCapability is invalid.' }
foreach ($item in $manifest.requiredFiles) {
  $path = Join-Path $workspace $item.path
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing engine file: $($item.path). Run 'npm run setup:engines' first." }
  $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
  if ($actual -ne $item.sha256) { throw "Hash mismatch for $($item.path): $actual" }
}
# CUDA build must ship its NVIDIA runtime DLLs; requiredFiles hash-locks them,
# this existence guard is a coarse second line of defense.
$cudaRuntime = Get-ChildItem -LiteralPath (Join-Path $workspace 'engines\colmap\bin') -File | Where-Object { $_.Name -match '(?i)cudart64_|curand64_|onnxruntime_providers_cuda' }
if (-not $cudaRuntime) { throw 'COLMAP CUDA runtime DLLs missing; expected cudart64_*.dll, curand64_*.dll, onnxruntime_providers_cuda.dll.' }
$colmap = Join-Path $workspace 'engines\colmap\bin\colmap.exe'
$savedPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$help = & $colmap feature_extractor -h 2>&1 | Out-String
$colmapExit = $LASTEXITCODE
if ($colmapExit -ne 0 -or $help -notmatch '(?i)with CUDA') { throw 'Bundled COLMAP did not explicitly report CUDA support.' }
$brush = Join-Path $workspace 'engines\brush\brush_app.exe'
$brushHelp = & $brush --help 2>&1 | Out-String
$brushExit = $LASTEXITCODE
$ErrorActionPreference = $savedPreference
if ($brushExit -ne 0) { throw "Bundled Brush help failed with exit code $brushExit" }
foreach ($flag in '--total-steps','--max-resolution','--export-path','--export-name') {
  if ($brushHelp -notmatch [regex]::Escape($flag)) { throw "Bundled Brush is missing $flag" }
}
Write-Host "Verified $($manifest.requiredFiles.Count) locked engine files; COLMAP CUDA and Brush CLI are valid."
