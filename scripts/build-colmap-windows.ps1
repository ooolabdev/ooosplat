[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
$lock = Get-Content -Raw (Join-Path $workspace 'engines/colmap-build.json') | ConvertFrom-Json
function Invoke-Checked([string]$Command, [string[]]$Arguments) {
  & $Command @Arguments
  if ($LASTEXITCODE -ne 0) { throw "$Command failed with exit code $LASTEXITCODE" }
}
if (-not $env:CUDA_PATH) { throw 'CUDA_PATH must identify the locked CUDA toolkit.' }
$env:PATH = "$(Join-Path $env:CUDA_PATH 'bin');$env:PATH"
foreach ($command in 'git', 'curl.exe', 'tar', 'cmake', 'ninja', 'nvcc', 'cl', 'node') {
  if (-not (Get-Command $command -ErrorAction SilentlyContinue)) { throw "Missing $command. Use a Visual Studio x64 developer shell with CUDA $($lock.cudaVersion)." }
}
if ((cmake --version | Select-Object -First 1) -ne "cmake version $($lock.cmakeVersion)") { throw 'CMake version differs from build lock.' }
if ((ninja --version) -ne $lock.ninjaVersion) { throw 'Ninja version differs from build lock.' }
Invoke-Checked 'node' @((Join-Path $PSScriptRoot 'verify-cuda-toolkit.mjs'), 'windows', $env:CUDA_PATH)
$cudaCompiler = Join-Path $env:CUDA_PATH 'bin/nvcc.exe'
$cache = Join-Path $workspace '.cache/colmap-windows-build'
New-Item -ItemType Directory -Force -Path $cache | Out-Null
$taskDirectory = Join-Path $cache ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $taskDirectory | Out-Null
$archive = Join-Path $cache "colmap-$($lock.commit).tar.gz"
if (-not (Test-Path $archive) -or (Get-FileHash $archive -Algorithm SHA256).Hash -ne $lock.sourceSha256) {
  Invoke-Checked 'curl.exe' @('--fail', '--location', '--retry', '3', $lock.sourceUrl, '--output', $archive)
}
if ((Get-FileHash $archive -Algorithm SHA256).Hash -ne $lock.sourceSha256) { throw 'COLMAP source SHA-256 mismatch.' }
Invoke-Checked 'tar' @('-xf', $archive, '-C', $taskDirectory)
$source = Join-Path $taskDirectory "colmap-$($lock.commit)"
$vcpkg = Join-Path $taskDirectory 'vcpkg'
Invoke-Checked 'git' @('clone', '--no-checkout', 'https://github.com/microsoft/vcpkg.git', $vcpkg)
Invoke-Checked 'git' @('-C', $vcpkg, 'checkout', '--detach', $lock.vcpkgCommit)
Invoke-Checked (Join-Path $vcpkg 'bootstrap-vcpkg.bat') @('-disableMetrics')
$build = Join-Path $taskDirectory 'build'
$stage = Join-Path $taskDirectory 'ooosplat-colmap-windows-x64'
# Match the locked upstream CUDA host-compiler setup, without changing sources.
$env:CUDAFLAGS = '-allow-unsupported-compiler'
$options = @('-S', $source, '-B', $build, '-G', 'Ninja', '-DCMAKE_BUILD_TYPE=Release',
  "-DCMAKE_INSTALL_PREFIX=$stage", "-DGIT_COMMIT_ID=$($lock.commit)", '-DGIT_COMMIT_DATE=Unknown',
  "-DCMAKE_TOOLCHAIN_FILE=$vcpkg/scripts/buildsystems/vcpkg.cmake", '-DVCPKG_TARGET_TRIPLET=x64-windows-release',
  '-DVCPKG_USE_LEGACY_APPLOCAL=ON', '-DBUILD_SHARED_LIBS=OFF', '-DCUDA_ENABLED=ON', '-DCASPAR_ENABLED=ON', '-DCASPAR_USE_DOUBLE=OFF',
  "-DCMAKE_CUDA_ARCHITECTURES:STRING=$($lock.cudaArchitectures -join ';')",
  "-DCMAKE_CUDA_COMPILER:FILEPATH=$cudaCompiler", "-DCUDAToolkit_ROOT:PATH=$env:CUDA_PATH")
foreach ($feature in $lock.disabledFeatures) { $options += "-D${feature}_ENABLED=OFF" }
Invoke-Checked 'cmake' $options
Invoke-Checked 'node' @((Join-Path $PSScriptRoot 'verify-cuda-toolkit.mjs'), 'windows', $env:CUDA_PATH, $build)
Invoke-Checked 'cmake' @('--build', $build, '--parallel', '4')
Invoke-Checked 'cmake' @('--install', $build)
$installed = Join-Path $build 'vcpkg_installed/x64-windows-release'
# Preserve every release DLL/plugin supplied by the selected core ports. The
# unused GUI/ONNX ports were excluded during configure, not deleted afterward.
Get-ChildItem (Join-Path $installed 'bin') -Filter '*.dll' -File -Recurse | ForEach-Object {
  $relative = $_.FullName.Substring((Join-Path $installed 'bin').Length + 1)
  $target = Join-Path (Join-Path $stage 'bin') $relative
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
  Copy-Item -LiteralPath $_.FullName -Destination $target -Force
}
foreach ($pattern in 'cudart64_*.dll', 'curand64_*.dll') {
  $dlls = @(Get-ChildItem -LiteralPath (Join-Path $env:CUDA_PATH 'bin') -Recurse -File -Filter $pattern)
  if (-not $dlls.Count) { throw "Missing CUDA runtime $pattern" }
  foreach ($dll in $dlls) { Copy-Item -LiteralPath $dll.FullName -Destination (Join-Path $stage 'bin') -Force }
}
if (-not $env:VCToolsRedistDir) { throw 'VCToolsRedistDir is required to collect the MSVC runtime.' }
$crt = @(Get-ChildItem -LiteralPath (Join-Path $env:VCToolsRedistDir 'x64') -Directory -Filter 'Microsoft.VC*.CRT')
if ($crt.Count -ne 1) { throw 'Cannot unambiguously locate the MSVC CRT runtime.' }
Get-ChildItem -LiteralPath $crt[0].FullName -File -Filter '*.dll' | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $stage 'bin') -Force }
Invoke-Checked 'curl.exe' @('--fail', '--location', '--retry', '3', 'https://visualstudio.microsoft.com/license-terms/vs2022-cruntime/', '--output', (Join-Path $stage 'MSVC-LICENSE.html'))
Invoke-Checked 'curl.exe' @('--fail', '--location', '--retry', '3', 'https://docs.nvidia.com/cuda/archive/13.2.0/eula/index.html', '--output', (Join-Path $stage 'CUDA-EULA.html'))
Invoke-Checked 'node' @((Join-Path $PSScriptRoot 'package-colmap-runtime.mjs'), 'windows', $stage, $source, $build, $installed)
Write-Host "Build retained for review: $taskDirectory"
