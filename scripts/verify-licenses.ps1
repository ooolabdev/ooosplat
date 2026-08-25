[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$workspace = Split-Path -Parent $PSScriptRoot

function Resolve-WorkspacePath {
    param([Parameter(Mandatory = $true)][string]$RelativePath)

    Join-Path $workspace ($RelativePath -replace "/", [IO.Path]::DirectorySeparatorChar)
}

function Read-Utf8Text {
    param([Parameter(Mandatory = $true)][string]$RelativePath)

    Get-Content -LiteralPath (Resolve-WorkspacePath $RelativePath) -Raw -Encoding UTF8
}

function Assert-True {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Contains {
    param(
        [Parameter(Mandatory = $true)][string]$Text,
        [Parameter(Mandatory = $true)][string]$Expected,
        [Parameter(Mandatory = $true)][string]$Context
    )

    Assert-True ($Text.Contains($Expected)) "$Context is missing '$Expected'."
}

function Get-NormalizedSha256 {
    param([Parameter(Mandatory = $true)][string]$Text)

    $normalized = $Text.Replace("`r`n", "`n").Replace("`r", "`n").TrimEnd("`n")
    $bytes = [Text.Encoding]::UTF8.GetBytes($normalized)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace("-", "")
    } finally {
        $sha.Dispose()
    }
}

$requiredFiles = @(
    "LICENSE",
    "NOTICE",
    "TRADEMARK_POLICY.md",
    "GENERATED_OUTPUTS.md",
    "licenses/THIRD_PARTY_NOTICES.txt",
    "licenses/FFmpeg-LGPL-2.1.txt",
    "licenses/COLMAP-LICENSE.txt",
    "licenses/NVIDIA-CUDA-Runtime.txt",
    "licenses/Brush-LICENSE.txt",
    "engines/manifest.json",
    "engines/manifest.linux.json",
    "engines/manifest.macos.json"
)

foreach ($relativePath in $requiredFiles) {
    Assert-True (Test-Path -LiteralPath (Resolve-WorkspacePath $relativePath) -PathType Leaf) "Required license file is missing: $relativePath"
}

$apacheText = Read-Utf8Text "LICENSE"
$expectedApacheSha256 = "58D1E17FFE5109A7AE296CAAFCADFDBE6A7D176F0BC4AB01E12A689B0499D8BD"
Assert-True ((Get-NormalizedSha256 $apacheText) -eq $expectedApacheSha256) "LICENSE is not the unmodified Apache License 2.0 text."

$package = (Read-Utf8Text "package.json") | ConvertFrom-Json
Assert-True ($package.license -eq "Apache-2.0") "package.json must declare Apache-2.0."
Assert-True ($package.author -eq "ooolabdev") "package.json must identify ooolabdev as author."
Assert-True ($package.repository.url -eq "git+https://github.com/ooolabdev/ooosplat.git") "package.json repository URL is incorrect."

$cargo = Read-Utf8Text "src-tauri/Cargo.toml"
Assert-True ([regex]::IsMatch($cargo, '(?m)^license\s*=\s*"Apache-2\.0"\s*$')) "Cargo.toml must declare Apache-2.0."
Assert-True ([regex]::IsMatch($cargo, '(?m)^authors\s*=\s*\["ooolabdev"\]\s*$')) "Cargo.toml must identify ooolabdev as author."
Assert-True ([regex]::IsMatch($cargo, '(?m)^repository\s*=\s*"https://github\.com/ooolabdev/ooosplat"\s*$')) "Cargo.toml repository URL is incorrect."

$tauri = (Read-Utf8Text "src-tauri/tauri.conf.json") | ConvertFrom-Json
Assert-True ($tauri.bundle.license -eq "Apache-2.0") "Tauri bundle license must be Apache-2.0."
Assert-True ($tauri.bundle.licenseFile -eq "../LICENSE") "Tauri bundle licenseFile must point to ../LICENSE."
$resourceNames = @($tauri.bundle.resources.PSObject.Properties.Name)
foreach ($resource in "../LICENSE", "../NOTICE", "../TRADEMARK_POLICY.md", "../GENERATED_OUTPUTS.md", "../licenses/THIRD_PARTY_NOTICES.txt", "../licenses/FFmpeg-LGPL-2.1.txt", "../licenses/COLMAP-LICENSE.txt", "../licenses/NVIDIA-CUDA-Runtime.txt", "../licenses/Brush-LICENSE.txt") {
    Assert-True ($resourceNames -contains $resource) "Tauri resources are missing $resource."
}

$tauriWindows = (Read-Utf8Text "src-tauri/tauri.windows.conf.json") | ConvertFrom-Json
Assert-True (@($tauriWindows.bundle.resources.PSObject.Properties.Name) -contains "../engines/manifest.json") "Windows Tauri resources are missing the Windows engine manifest."
$tauriMacos = (Read-Utf8Text "src-tauri/tauri.macos.conf.json") | ConvertFrom-Json
Assert-True ($tauriMacos.bundle.macOS.minimumSystemVersion -eq "15.0") "macOS bundle must target macOS 15.0."
$macosResources = @($tauriMacos.bundle.resources.PSObject.Properties.Name)
foreach ($resource in "../engines/manifest.macos.json", "../engines/macos/arm64/") {
    Assert-True ($macosResources -contains $resource) "macOS Tauri resources are missing $resource."
}

$manifest = (Read-Utf8Text "engines/manifest.json") | ConvertFrom-Json
Assert-True ($manifest.schemaVersion -ge 2) "Engine manifest schemaVersion must include license mappings."
Assert-True ($manifest.engines.Count -eq 3) "License verification expects exactly the three direct native engines."

$expectedEngines = @{
    "FFmpeg / FFprobe" = @{
        License = "LGPL-2.1-or-later"
        File = "licenses/FFmpeg-LGPL-2.1.txt"
    }
    "COLMAP" = @{
        License = "BSD-3-Clause"
        File = "licenses/COLMAP-LICENSE.txt"
    }
    "Brush" = @{
        License = "Apache-2.0"
        File = "licenses/Brush-LICENSE.txt"
    }
}

$thirdParty = Read-Utf8Text "licenses/THIRD_PARTY_NOTICES.txt"
Assert-True (-not $thirdParty.Contains("OOOSplat 0.2.0")) "Third-party notices still contain the obsolete 0.2.0 heading."
Assert-True (-not (Test-Path -LiteralPath (Resolve-WorkspacePath "licenses/FFmpeg-LICENSE.txt"))) "The obsolete LGPLv3 FFmpeg-LICENSE.txt file must not exist."

foreach ($engine in $manifest.engines) {
    Assert-True ($expectedEngines.ContainsKey($engine.name)) "Unexpected direct engine in manifest: $($engine.name)"
    $expected = $expectedEngines[$engine.name]
    Assert-True ($engine.license.StartsWith($expected.License)) "$($engine.name) license identifier does not match $($expected.License)."
    Assert-True (@($engine.licenseFiles).Count -eq 1) "$($engine.name) must map to one direct license file."
    Assert-True ($engine.licenseFiles[0] -eq $expected.File) "$($engine.name) license file mapping is incorrect."
    Assert-True (Test-Path -LiteralPath (Resolve-WorkspacePath $expected.File) -PathType Leaf) "$($engine.name) mapped license file is missing."
    Assert-Contains $thirdParty $engine.name "THIRD_PARTY_NOTICES.txt"
    Assert-Contains $thirdParty $expected.License "THIRD_PARTY_NOTICES.txt"
    Assert-Contains $thirdParty $expected.File "THIRD_PARTY_NOTICES.txt"
}

$linuxManifest = (Read-Utf8Text "engines/manifest.linux.json") | ConvertFrom-Json
Assert-True ($linuxManifest.schemaVersion -ge 2) "Linux engine manifest schemaVersion must include license mappings."
Assert-True ($linuxManifest.brush.version -eq "0.3.0") "Linux Brush version is incorrect."
Assert-True ($linuxManifest.brush.sourceUrl -eq "https://github.com/ArthurBrussee/brush/releases/download/v0.3.0/brush-app-x86_64-unknown-linux-gnu.tar.xz") "Linux Brush release archive is incorrect."
Assert-True ($linuxManifest.brush.license -eq "Apache-2.0") "Linux Brush license identifier is incorrect."
Assert-True (@($linuxManifest.brush.licenseFiles).Count -eq 1) "Linux Brush must map to one direct license file."
Assert-True ($linuxManifest.brush.licenseFiles[0] -eq "licenses/Brush-LICENSE.txt") "Linux Brush license file mapping is incorrect."
foreach ($marker in "Ubuntu 24.04 Alpha, Linux x86_64 release archive", "brush-app-x86_64-unknown-linux-gnu.tar.xz", "engines/manifest.linux.json") {
    Assert-Contains $thirdParty $marker "THIRD_PARTY_NOTICES.txt"
}

$macosManifest = (Read-Utf8Text "engines/manifest.macos.json") | ConvertFrom-Json
Assert-True ($macosManifest.schemaVersion -ge 1) "macOS engine manifest schemaVersion is missing."
Assert-True ($macosManifest.platform -eq "macos") "macOS engine manifest platform is incorrect."
Assert-True ($macosManifest.architecture -eq "arm64") "macOS engine manifest must be Apple arm64 only."
Assert-True ($macosManifest.minimumSystemVersion -eq "15.0") "macOS engine manifest must target macOS 15.0."
Assert-True ($macosManifest.buildEnvironment.homebrewCoreCommit -match '^[A-F0-9]{40}$') "macOS Homebrew/core build commit is not pinned."
Assert-True ($macosManifest.buildEnvironment.runner -eq "macos-15") "macOS engine runner must be pinned to macos-15."
Assert-True ($macosManifest.buildEnvironment.usePinnedHomebrewBottles -eq $true) "macOS build dependencies must use pinned Homebrew bottles."
Assert-True ($macosManifest.engines.Count -eq 3) "macOS manifest must contain the three direct engines."
foreach ($engine in $macosManifest.engines) {
    Assert-True ($expectedEngines.ContainsKey($engine.name)) "Unexpected macOS engine: $($engine.name)"
    $expected = $expectedEngines[$engine.name]
    Assert-True ($engine.license -eq $expected.License) "$($engine.name) macOS license identifier is incorrect."
    Assert-True (@($engine.licenseFiles).Count -eq 1 -and $engine.licenseFiles[0] -eq $expected.File) "$($engine.name) macOS license mapping is incorrect."
    Assert-True ($engine.sourceSha256 -match '^[A-F0-9]{64}$') "$($engine.name) macOS source SHA-256 is invalid."
}
foreach ($marker in "macOS 15+ Apple Silicon arm64", "ffmpeg-8.1.2.tar.xz", "COLMAP 4.0.4 macOS arm64 CPU CLI-only", "brush-app-aarch64-apple-darwin.tar.xz", "engines/manifest.macos.json") {
    Assert-Contains $thirdParty $marker "THIRD_PARTY_NOTICES.txt"
}

$ffmpegLicense = Read-Utf8Text "licenses/FFmpeg-LGPL-2.1.txt"
Assert-Contains $ffmpegLicense "GNU LESSER GENERAL PUBLIC LICENSE" "FFmpeg license"
Assert-Contains $ffmpegLicense "Version 2.1, February 1999" "FFmpeg license"
Assert-True (-not $ffmpegLicense.Contains("Version 3, 29 June 2007")) "FFmpeg license still contains the LGPLv3 text."

$colmapLicense = Read-Utf8Text "licenses/COLMAP-LICENSE.txt"
Assert-Contains $colmapLicense "ETH Zurich and UNC Chapel Hill" "COLMAP license"
Assert-Contains $colmapLicense "Redistributions of source code" "COLMAP license"
Assert-Contains $colmapLicense "Redistributions in binary form" "COLMAP license"
Assert-Contains $colmapLicense "Neither the name" "COLMAP license"

$notice = Read-Utf8Text "NOTICE"
Assert-Contains $notice "Copyright 2026 ooolabdev" "NOTICE"
Assert-Contains $notice "licenses/THIRD_PARTY_NOTICES.txt" "NOTICE"
Assert-Contains $notice "TRADEMARK_POLICY.md" "NOTICE"

$trademark = Read-Utf8Text "TRADEMARK_POLICY.md"
Assert-Contains $trademark "does not grant a license to use the OOOSplat Marks" "Trademark policy"
Assert-Contains $trademark "https://github.com/ooolabdev/ooosplat/issues" "Trademark policy"

$outputs = Read-Utf8Text "GENERATED_OUTPUTS.md"
foreach ($term in "final.ply", "Apache License 2.0", "General Public License (GPL)", "Lesser General Public License (LGPL)", "does not assign copyright ownership") {
    Assert-Contains $outputs $term "Generated outputs policy"
}

Write-Host "Verified OOOSplat license metadata and Windows/Linux/macOS notices for 3 direct engines."
