# OOOSplat

[中文](README.md) | [English](README_EN.md)

<p align="center">
  <img src="assets/readme-logo.svg" alt="OOOSplat Logo" width="180">
</p>

<p align="center">
  <a href="https://github.com/ooolabdev/ooosplat/releases/tag/0.3.0"><strong>⬇️ Download OOOSplat 0.3.0 for Windows, macOS, or Ubuntu</strong></a>
</p>

OOOSplat is a local desktop application that turns an ordinary orbit video into a 3D Gaussian Splatting project in one workflow. Choose a video, project directory, and quality preset, and OOOSplat automatically handles frame extraction, camera reconstruction, training, PLY publishing, preview, adjustment, and export.

Windows and the Apple Silicon macOS Alpha provide FFmpeg, FFprobe, COLMAP, and Brush with the application. Linux support remains limited to an Ubuntu 24.04 LTS x86_64 Alpha. Every generation stage runs on the user's own CPU and GPU; input media, project data, models, and logs do not need to be uploaded to a cloud reconstruction or training service. The React interface calls the local Rust backend directly, with no remote service or localhost API required.

Current version: **0.3.0**

See the [OOOSplat Roadmap](ROADMAP.md) for planned work.

> The current release generates and manages `final.ply`, previews Gaussian Splats inside OOOSplat, edits whole-model transforms, plays staged animations, and exports non-destructive results.

## Why OOOSplat

- **One-click Gaussian generation**: Select an input video, project directory, and quality preset. OOOSplat then runs FFmpeg frame extraction, COLMAP camera reconstruction, Brush training, and `final.ply` publishing without manual engine setup or command-line orchestration.
- **Security and data control**: Source media, frames, reconstruction data, models, and logs stay in the local project directory by default and do not need to be uploaded to the cloud. Non-destructive transforms and exports preserve the original `final.ply`.
- **Fully local compute**: Reconstruction and training run on the user's own machine without remote compute services. COLMAP automatically uses a compatible local NVIDIA GPU when available and falls back to CPU otherwise, keeping both processing and data under the user's control.

## Interface Preview

### Create and Manage Tasks

![OOOSplat create-task and task-history workspace](assets/screenshots/task-workspace.png)

### Preview and Adjust Gaussian Splats

![OOOSplat Gaussian Splat preview and transform workspace](assets/screenshots/gaussian-preview.png)

## Key Features

- Create Gaussian Splatting projects from MP4 and MOV videos.
- Automatically run video analysis, uniform frame extraction, feature extraction, sequential matching, camera reconstruction, Brush training, and PLY publishing.
- Bundle CUDA-enabled COLMAP on Windows and arm64 CPU-only COLMAP on macOS; Ubuntu uses its system CPU COLMAP. FFmpeg and Brush follow pinned, verified platform policies.
- Automatically check the bundled CUDA runtime, NVIDIA driver version, and GPU Compute Capability. COLMAP uses GPU acceleration for feature extraction and matching when the requirements are met, and otherwise falls back to CPU.
- Show processing stages, engine output, key counters, elapsed time, and up to 500 UI log entries in real time.
- Write complete raw process output to the project `logs` directory.
- Cancel tasks and terminate the full child-process tree with a Windows Job Object or Unix process group.
- Choose a custom projects root, defaulting to `Documents\SplatStudio\Projects`.
- Track completed, failed, interrupted, and cancelled tasks.
- Preview completed `.ply` projects under “03 Preview” with Orbit, Pan, and Zoom. Switching between Adjust and Animation does not reload the model or reset the camera.
- Use Adjust mode to edit whole-model position, rotation, and uniform scale, with undo and redo.
- Persist transforms in `project.json` and export numbered `edited.ply` copies without overwriting `final.ply`.
- Play a 5-second reveal, an 8-second shockwave, and a continuous camera orbit, then export a watermarked 1080×1920, 30 fps, 23-second H.264 MP4.
- Reveal `final.ply` in the platform file manager or move the complete project to the system trash.
- Resize the left and right panels by dragging the divider, and scale the full interface from 80% to 140%.
- Support Chinese characters, spaces, long file names, and UNC project paths.

## Processing Pipeline

```text
Input video
  │
  ├─ FFprobe: read duration, resolution, frame rate, and frame count
  ├─ FFmpeg: extract frames uniformly for the selected quality preset
  ├─ COLMAP: automatically select CPU or CUDA GPU for feature extraction and sequential matching
  ├─ COLMAP: incremental reconstruction and registration validation
  ├─ Brush: train Gaussian Splats with an available GPU backend
  └─ Validate the PLY and atomically publish final.ply
```

The task stops when COLMAP registers fewer than 50% of the input images. A 50%–80% registration rate produces a quality warning but continues, while 80% or higher is treated as normal.

## System Requirements

- Windows 10 or Windows 11, x64.
- WebView2 Runtime support.
- Video export requires WebCodecs AVC support in WebView2. Animation mode remains available when encoding is unavailable, and the UI reports why export is disabled.
- An available GPU graphics backend for Brush training; a discrete GPU is recommended.
- COLMAP CUDA acceleration requires an NVIDIA GPU, Windows driver 528.33 or newer, and Compute Capability 5.0 or higher. OOOSplat automatically uses CPU when these requirements are not met; no manual configuration is required.
- Enough disk space for a source-video copy, extracted frames, COLMAP data, Brush intermediate files, and the final PLY. Long videos and higher quality presets can require substantial space.
- The installer uses a per-machine installation and may require administrator privileges.

The COLMAP build bundled on Windows supports both CPU and CUDA GPU execution. OOOSplat automatically selects the available backend before each task. Brush uses its own available graphics backend; its GPU detection and runtime are independent of COLMAP.

### macOS 15+ Alpha (Apple Silicon only)

> The current deliverable is an unsigned, unnotarized `.app`/`.dmg` Alpha for M1 or newer Apple Silicon Macs. Intel Macs and Universal Binaries are not supported.

- Bundles native arm64 FFmpeg 8.1.2, a real standalone FFprobe, COLMAP 4.0.4 CPU CLI-only, and Brush v0.3.0.
- Users do not install Homebrew, and OOOSplat never falls back to a Homebrew or system `PATH` engine.
- COLMAP always uses CPU in this Alpha. Brush independently selects an available Metal graphics backend, and the UI explains this distinction.
- Gatekeeper may block the unsigned Alpha on first launch. In Finder, right-click the app and choose Open. Signing and notarization are planned for a later production release.

### Ubuntu 24.04 Alpha (x86_64 only)

> This Alpha delivers an x86_64 `.deb` package built on Ubuntu 24.04. It does not claim support for Ubuntu 22.04, other Linux distributions, or production deployment.

- Ubuntu 24.04 LTS, x86_64.
- A graphics backend and driver supported by Brush. Brush officially supports AMD, Intel, and NVIDIA GPUs. Current end-to-end validation used NVIDIA; CPU-only software graphics backends remain unverified but are not artificially blocked by startup checks.
- Source builds require Node.js 22.12+, Rust stable, and Tauri 2's WebKitGTK development dependencies; `.deb` users do not need these development tools.
- Ubuntu 24.04 system `ffmpeg`, `ffprobe`, and CPU-only `colmap` (COLMAP 3.9 from the Ubuntu repository).
- Brush v0.3.0 for Linux x86_64, installed and verified by `npm run setup:engines`.

Install Ubuntu dependencies with:

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file ffmpeg colmap \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev
```

Install a working Vulkan driver for the graphics adapter, such as the proprietary NVIDIA driver or Mesa for AMD/Intel. Ubuntu 24.04's non-CUDA COLMAP package automatically uses the CPU, while Brush selects an available graphics backend at runtime. Fully CPU-only software Vulkan has not yet been validated end to end.

After downloading the `OOOSplat-0.3.0-x64-linux` Artifact from GitHub Actions, install it with:

```bash
sudo apt install ./OOOSplat-0.3.0-x64-linux.deb
```

The `.deb` installs FFmpeg, FFprobe, and CPU COLMAP through Ubuntu's package manager; the pinned Brush runtime is included in the package.

## Installation and Use

1. On Windows, run `OOOSplat-0.3.0-x64-windows.exe`. On an Apple Silicon Mac, open `OOOSplat-0.3.0-arm64-macos.dmg` and drag OOOSplat into Applications. On Ubuntu 24.04, run `sudo apt install ./OOOSplat-0.3.0-x64-linux.deb`.
2. Start OOOSplat and confirm that the bundled engine status in the top bar is healthy.
3. Select an input video under “01 Create New Task.”
4. Choose the projects root; OOOSplat remembers the last location.
5. Select the Fast, Balanced, or Detailed quality preset.
6. Review the automatically detected COLMAP acceleration status and its explanation, then select “Start Generation.”
7. Follow live stages, metrics, and logs on the left. When processing finishes, select “Preview” under “02 Task History.”
8. Edit the model in the “Adjust” mode under “03 Preview.” Changes are saved automatically; “Export Gaussian” writes a new edited PLY.
9. Switch to “Animation” for the portrait composition and staged playback. “Export Video” writes a 23-second portrait MP4 into the project directory.

Usage notes:

- Drag the divider between the panels to resize them; double-click it to restore the default ratio.
- Use the percentage button in the lower-right corner to reduce, reset, or increase the interface scale.
- The video, project directory, and quality preset cannot be changed while a task is running.
- “Delete” moves the complete project—including the source copy and intermediate files—to the Recycle Bin. OOOSplat does not fall back to permanent deletion if that operation fails.

## Quality Presets

| Preset | Frames retained | FFmpeg extraction rate | Brush iterations | Maximum training resolution |
| --- | ---: | ---: | ---: | ---: |
| Fast | 30% | Source FPS × 0.30 | 8,000 | 1,200 |
| Balanced | 50% | Source FPS × 0.50 | 15,000 | 1,600 |
| Detailed | 100% | Source FPS × 1.00 | 30,000 | 2,000 |

FFmpeg performs frame reduction; COLMAP does not reduce the number of frames. OOOSplat does not set a maximum extracted-frame count or an additional Splat-count limit. The final number of Splats depends on the source material, reconstruction, and Brush training.

## Project and File Locations

Each generation creates a separate directory under the projects root:

```text
<projects-root>\<yyyyMMdd-HHmmss_video-name>\
  final.ply             Final Gaussian Splatting file
  project.json          Project metadata and result metrics
  edited.ply            First optional non-destructive transform export
  edited-2.ply          Later exports are automatically numbered
  preview.mp4           First optional animation-preview video export
  preview-2.mp4         Later video exports are automatically numbered
  state.json            Pipeline state
  source\
    input.<ext>         Source-video copy
  work\
    frames\             Frames extracted by FFmpeg
    colmap\             COLMAP database and sparse reconstruction
    brush\              Brush dataset and training intermediates
  logs\                 Complete FFmpeg, COLMAP, Brush, and pipeline logs
```

Windows-invalid characters are removed from project names. Name collisions receive suffixes such as `-2` and `-3`.

Application settings and the project index are stored in:

```text
%LOCALAPPDATA%\SplatStudio\settings.json
%LOCALAPPDATA%\SplatStudio\project-index.json
```

## Bundled Engines

| Engine | Pinned version/build | Purpose |
| --- | --- | --- |
| FFmpeg / FFprobe | Windows x64 8.1 LGPL shared; macOS arm64 8.1.2 LGPL shared | Video analysis and frame extraction |
| COLMAP | Windows 4.0.4 CUDA; macOS arm64 4.0.4 CPU CLI-only | Feature extraction, matching, and camera reconstruction |
| Brush | v0.3.0 for Windows x64 / macOS arm64 | Gaussian Splatting training and PLY export |

Windows, Ubuntu, and macOS source and integrity policies are recorded in [`engines/manifest.json`](engines/manifest.json), [`engines/manifest.linux.json`](engines/manifest.linux.json), and [`engines/manifest.macos.json`](engines/manifest.macos.json). Large engine files are not committed to Git; developers restore them with `npm run setup:engines`. Release builds verify sources, hashes, architecture, the dynamic-library closure, and Brush CLI compatibility.

Third-party licenses and notices are in [`licenses/`](licenses/):

- FFmpeg: LGPL-2.1-or-later; the selected build disables GPL/nonfree components.
- COLMAP: BSD-3-Clause.
- Brush: Apache-2.0.

## Local Development

### Development Environment

- Node.js 22.12 or newer.
- Rust stable, targeting `x86_64-pc-windows-msvc`.
- Visual Studio 2022 Build Tools with Desktop development with C++.
- The WebView2 development/runtime environment required by Tauri 2.

Install dependencies and start development mode:

```powershell
npm install
npm run setup:engines
npm run tauri -- dev
```

### Ubuntu 24.04 Alpha Development

After installing the Ubuntu system dependencies, Node.js, and Rust:

```bash
npm ci
npm run setup:engines
npm run verify:engines
npm run verify:licenses
npm run tauri -- dev
```

From the repository root, `./scripts/start-app-linux.sh` (or `npm run start:app:linux`) verifies the local engines and license mappings, rebuilds the release executable only when the sources changed, and starts OOOSplat.

Ubuntu 24.04 Alpha engine setup installs only verified Brush under `engines/linux/brush/`; FFmpeg, FFprobe, and CPU COLMAP remain system packages. Tauri embeds Brush in the x86_64 `.deb` and declares FFmpeg and COLMAP as Debian dependencies.

The Ubuntu 24.04 Alpha CI workflow is in `.github/workflows/ubuntu.yml`. Standard GitHub runners cover frontend tests/build, license mappings, Rust tests, Clippy, FFmpeg integration, `.deb` creation, package validation, and upload of the installer plus SHA-256. Brush end-to-end coverage requires a host or self-hosted runner with a working graphics backend. The complete pipeline is currently validated on NVIDIA; AMD, Intel, and software Vulkan test results are welcome.

### macOS 15+ Apple Silicon Alpha Development

On an Apple Silicon Mac with Node.js, Rust, and Xcode Command Line Tools:

```bash
npm ci
npm run setup:engines
npm run verify:engines
npm run verify:licenses
npm run tauri -- dev
```

`setup:engines` downloads the complete arm64 runtime from a pinned Release in this repository. Homebrew is used only when maintainers run `npm run setup:build-deps:macos` and then `npm run build:engines:macos` to rebuild that runtime. `.github/workflows/macos.yml` builds the unsigned app and DMG; `.github/workflows/macos-engines.yml` builds and publishes the locked engine archive.

### Tests and Checks

```powershell
# Frontend tests
npm test

# Frontend production build and TypeScript type-check
npm run build

# Rust tests
cargo test --manifest-path src-tauri\Cargo.toml

# Rust static checks
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings

# Bundled engine version, hash, and CUDA runtime checks
npm run verify:engines

# First-party and third-party license checks
npm run verify:licenses
```

### Build the Windows Installer

```powershell
npm run package:windows
```

The NSIS installer is written to:

```text
dist-artifacts\OOOSplat-0.3.0-x64-windows.exe
```

Run `npm run setup:engines` before the first build. Tauri's `beforeBuildCommand` automatically runs the engine checks and frontend production build, but it does not access the network implicitly during packaging.

## CLI

The repository also provides the `splatstudio` diagnostic CLI:

```powershell
# Check all bundled engines
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- health

# Read video metadata
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- probe "D:\Videos\orbit.mp4"

# Show the frame extraction plan without writing frames
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- plan "D:\Videos\orbit.mp4" --quality balanced

# Extract frames only
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- extract "D:\Videos\orbit.mp4" "D:\Frames" --quality fast

# Run the complete pipeline
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- generate "D:\Videos\orbit.mp4" --projects-root "D:\Splat Projects" --quality balanced
```

For development or diagnostics, use the global `--engine-dir <path>` argument or the `OOOSPLAT_ENGINE_DIR` environment variable to override the default engine directory.

On Linux, `OOOSPLAT_FFMPEG`, `OOOSPLAT_FFPROBE`, `OOOSPLAT_COLMAP`, and `OOOSPLAT_BRUSH` can override individual executables. Otherwise, OOOSplat searches managed repository locations and the system `PATH`.

## FAQ

### Why is COLMAP using the CPU instead of the GPU?

OOOSplat enables COLMAP GPU acceleration only when the bundled CUDA runtime is healthy and it can confirm an NVIDIA driver version of at least 528.33 and Compute Capability 5.0 or higher. If detection fails or a requirement is not met, COLMAP automatically falls back to CPU and the application shows the specific reason. Brush is independent of COLMAP and selects an available graphics backend at runtime.

### Why does a task stop after camera reconstruction?

The usual cause is an image registration rate below 50%. Use an orbit video with stable exposure, clear frames, continuous movement, and sufficient viewpoint overlap. Avoid fast rotation, strong reflections, large plain-color areas, and moving subjects.

### Why does a project use so much disk space?

Each project keeps a source-video copy, extracted frames, COLMAP data, and Brush intermediates for diagnostics and traceability. After confirming the result, use “Delete” in Task History to move the complete project to the Recycle Bin.

### Can I view final.ply directly in OOOSplat?

Yes. Select “Preview” on a completed project in “02 Task History” to open its `.ply` in the dedicated preview workspace. “Adjust” edits the whole model's position, rotation, and uniform scale. “Animation” adds a 5-second reveal, an 8-second shockwave, a continuous orbit, and watermarked portrait MP4 export. Per-Gaussian selection, deletion, cropping, cleanup, `.sog`, and `.spz` are not supported yet.

## Technology

- Desktop framework: Tauri 2
- Backend: Rust and Tokio
- Frontend: React 19, TypeScript, Vite, and Zustand
- Gaussian preview and animation: PlayCanvas Engine and PlayCanvas React
- Video encoding and muxing: WebCodecs and Mediabunny
- Native pipeline: FFmpeg / FFprobe, COLMAP, and Brush
- Process-tree management: Windows Job Object; Linux Unix process group

## 🤝 Contributing

Contributions are welcome!

Whether it's bug fixes, Linux/macOS support, UI improvements,
documentation, or new Gaussian Splatting features, we'd love your help.

See [CONTRIBUTING.md](CONTRIBUTING.md) to get started.

## License

### Code

OOOSplat first-party code and accompanying documentation are released under the [Apache License 2.0](LICENSE). See [NOTICE](NOTICE) for copyright information.

### Third-party Components

FFmpeg / FFprobe, COLMAP, Brush, PlayCanvas, and Mediabunny remain subject to their own licenses and do not become Apache-2.0 software merely because they are distributed with OOOSplat. See [Third-party Notices](licenses/THIRD_PARTY_NOTICES.txt) and the [Engine Manifest](engines/manifest.json) for direct components, versions, sources, and license files. This list is not represented as a complete audit of transitive dependencies such as Qt, Boost, or Ceres.

### Brand

Apache-2.0 does not grant permission to use the “OOOSplat” name, logo, icons, or other visual identifiers as trademarks. See the [OOOSplat Trademark Policy](TRADEMARK_POLICY.md) for truthful references, tutorials, screenshots, unmodified distribution, and modified-version naming rules.

### Generated Models

`final.ply` and other generated outputs do not automatically become subject to Apache, GPL, LGPL, or another bundled software license merely because OOOSplat was used. This statement does not determine copyright ownership or grant rights in input or third-party material; see [Generated Outputs](GENERATED_OUTPUTS.md) for the complete boundary.
