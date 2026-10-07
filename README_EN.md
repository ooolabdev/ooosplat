# OOOSplat

[中文](README.md) | [English](README_EN.md)

<p align="center">
  <img src="assets/readme-logo.svg" alt="OOOSplat Logo" width="180">
</p>

<p align="center">
  <a href="https://trendshift.io/repositories/177239?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-177239" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/177239/daily?language=Rust" alt="OOOSplat — Trendshift Rust Repository of the Day" width="250" height="55" /></a>
  <a href="https://trendshift.io/repositories/177239?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-177239" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/177239/weekly?language=Rust" alt="OOOSplat — Trendshift Rust Repository of the Week" width="250" height="55" /></a>
</p>

<p align="center">
  <a href="https://github.com/ooolabdev/ooosplat/releases/tag/0.5.0"><strong>⬇️ Download the latest release, OOOSplat 0.5.0, for Windows, macOS, or Ubuntu</strong></a>
</p>

OOOSplat is a local desktop application that turns an ordinary orbit video or image sequence into a 3D Gaussian Splatting project in one workflow. Choose source media, a project directory, and a quality preset, and OOOSplat automatically handles image preparation, camera reconstruction, training, PLY publishing, preview, adjustment, and export.

Windows and the Apple Silicon macOS Alpha provide FFmpeg, FFprobe, COLMAP, and Brush with the application. Linux support remains limited to an Ubuntu 24.04 LTS x86_64 Alpha. Every generation stage runs on the user's own CPU and GPU; input media, project data, models, and logs do not need to be uploaded to a cloud reconstruction or training service. The React interface calls the local Rust backend directly. Normal GUI workflows do not depend on a remote service or localhost API; a loopback HTTP endpoint starts only when the optional MCP service is enabled.

Main development version: **0.6.0**. Latest published release: **0.5.0**.

The main branch includes optional local MCP v1. It is disabled by default, listens only on `127.0.0.1`, and does not require a token. Enabling it automatically authorizes the projects root's `Inputs` directory, with additional input directories available by explicit authorization. A local AI Agent can use seven tools to create, start, list, inspect, read logs from, and cancel tasks that remain visible in the same application. See [MCP v1 setup, tools, and lifecycle](docs/mcp-v1.md).

See the [OOOSplat Roadmap](ROADMAP_EN.md) for planned work.

> The 0.6.0 main development version adds local MCP v1, the shared task service, cross-platform COLMAP 4.2.1 runtimes, unpremultiplied transparent input with binary masks, and further Auto Optimize, training densification, and preview stability improvements.

## Why OOOSplat

- **One-click Gaussian generation**: Select an input video or image-sequence folder, project directory, and quality preset. OOOSplat then runs image preparation, COLMAP camera reconstruction, Brush training, and `final.ply` publishing without manual engine setup or command-line orchestration.
- **Cross-platform compatibility**: Supports Windows, macOS, and Linux, bringing local Gaussian Splat generation to all three major desktop platforms. See the compatibility section below for specific OS and processor requirements.
- **Security and privacy protection**: Source media, extracted frames, camera reconstruction data, Gaussian models, and logs stay in the user-selected local project directory by default. The core generation workflow runs on the user's machine, so original videos, images, and models do not need to be uploaded to third-party reconstruction or training platforms. This reduces exposure risks during network transfer, cloud retention, and unauthorized access.
- **Fully local compute**: Reconstruction and training run on the user's own machine without remote compute services. COLMAP automatically uses a compatible local NVIDIA GPU when available and falls back to CPU otherwise, keeping both processing and data under the user's control.

## Demo Video

https://github.com/user-attachments/assets/5b9e8cef-4c71-4bfa-ba23-641fcdd37659

## Interface Preview

### Create and Manage Tasks

![OOOSplat create-task and task-history workspace](assets/screenshots/task-workspace.png)

### Preview and Adjust Gaussian Splats

![OOOSplat Gaussian Splat preview and transform workspace](assets/screenshots/gaussian-preview.png)

## Key Features

- Create Gaussian Splatting projects from MP4/MOV videos or folders containing JPG, JPEG, and PNG images.
- “Auto Optimize (Experimental)” is enabled by default. Video starts at a 6 / 8 / 12 FPS target and uses preset-specific resolution policies for frame preparation, COLMAP, and Brush. If initial reconstruction coverage is low, OOOSplat can add bridge frames within the remaining budget. Disabling it restores the legacy fixed-ratio extraction and resolution strategy.
- Image sequences keep every image and use a shared camera, exhaustive matching, and the existing incremental Mapper. Videos use sequential matching; bridge-frame recovery does not apply to image sequences.
- Detect Alpha channels in transparent MOV files, extract unpremultiplied RGBA PNG frames, and create strict 0/255 COLMAP masks at `alpha >= 128` in one pass. Masked pixels are excluded from feature extraction while the original color and Alpha remain available to Brush.
- Detect transparent PNG images automatically, preserve the original file, bit depth, RGB, and Alpha, and generate binary COLMAP masks at the equivalent 0.5 Alpha threshold.
- Add high-resolution reshoots to completed projects. Capture video or images with the same device, lens, and resolution; OOOSplat reuses the original database, shared camera, and sparse model, processes only the new features, matches, and registrations, then retrains Brush from every registered image. Transparent MOV/PNG reshoots preserve RGBA and receive automatic masks. Each reshoot creates a separate derived project and never overwrites its source.
- Bundle COLMAP 4.2.1 from the same commit on all platforms: self-built CUDA/Caspar/Ceres on Windows/Linux, Ceres CPU on Apple Silicon. Brush uses pinned OOOBrush ooo-v1.0.0 headless CLI; FFmpeg policy is unchanged.
- Automatically check the bundled CUDA runtime, NVIDIA driver version, and GPU Compute Capability. COLMAP uses GPU acceleration for feature extraction and matching when the requirements are met, and otherwise falls back to CPU.
- Detailed quality selects the Brush training resolution and Splat limit from detected VRAM. If an explicit out-of-memory failure occurs, OOOSplat safely retries once with a lower training profile. On Windows systems with one NVIDIA discrete GPU, Brush also prefers that adapter and provides clearer guidance for device-loss failures.
- Show processing stages, engine output, key counters, elapsed time, and up to 500 UI log entries in real time.
- Write complete raw process output to the project `logs` directory.
- Cancel tasks and terminate the full child-process tree with a Windows Job Object or Unix process group.
- Choose a custom projects root, defaulting to `Documents\SplatStudio\Projects`.
- Track completed, failed, interrupted, and cancelled tasks.
- Share tasks, the execution lock, progress, logs, and results between the GUI and optional local MCP v1. Local Agents can call `get_app_status`, `create_generation_task`, `start_task`, `list_tasks`, `get_task_status`, `read_task_logs`, and `cancel_task`.
- Resume interrupted pipelines at stage boundaries. OOOSplat validates frame, mask, COLMAP database, sparse reconstruction, and PLY checkpoints, reuses trusted stages, and safely reruns from the earliest invalid stage.
- Estimate generation time from source size, quality preset, and recent successful local projects, while continuously updating Brush training progress.
- Preview completed `.ply` projects under “03 Preview” with Orbit, Pan, and Zoom. Switching between Adjust and Animation does not reload the model or reset the camera.
- Use Adjust mode to edit whole-model position, rotation, and uniform scale, with undo and redo.
- The preview ground grid uses persistent rendering resources to reduce repeated work during long sessions, and the model Transform scale range is expanded to `0.001–10000`.
- Use rectangle, sphere, and box Gaussian selection tools. Rectangle selection projects centers through the scene for non-destructive deletion, while sphere and box crops keep points inside the live selection volume.
- Crop and deletion state is saved automatically. “Save” writes the current result to the single `edit.ply`; later saves safely replace it, while the original `final.ply` is never overwritten.
- Play a 5-second reveal, an 8-second shockwave, and a continuous camera orbit. Choose portrait (1080×1920) or landscape (1920×1080) composition and export a watermarked 30 fps, 23-second H.264 MP4.
- The Export menu also creates a full-quality, single-file offline HTML viewer with the runtime and model embedded. Double-click to rotate, pan, zoom, and manually play the animation without internet access. Current transforms, crops, deletions, and all retained Gaussian / SH data are preserved; `final.ply` and `edit.ply` are not modified. File size is roughly 4/3 of the corresponding PLY, plus the viewer. Very large models remain subject to browser and device capacity.
- Reveal `final.ply` in the platform file manager or move the complete project to the system trash.
- Resize the left and right panels by dragging the divider, and scale the full interface from 80% to 140%.
- Support Chinese characters, spaces, long file names, and UNC project paths.
- Switch instantly between the Simplified Chinese and English interfaces. The first launch follows the system language, and an explicit choice is remembered.

### Gaussian Editing Shortcuts

- In Rectangle Select, drag with the left mouse button to replace the selection, use `Shift + drag` to add, and `Ctrl + drag` to remove. Use the middle mouse button to orbit, the right mouse button to pan, and the wheel to zoom.
- Yellow highlights show the temporary selection. Press `Delete` or `Backspace` to remove selected Gaussians non-destructively, or `Esc` to clear the temporary selection.
- Sphere and box selection enter an orthographic view automatically. Switch between side, front, and top views, then adjust the volume in the viewport or through numeric fields.
- Use `Ctrl + Z` to undo and `Ctrl + Shift + Z` or `Ctrl + Y` to redo. Transform, crop, and deletion changes share one history; camera movement and temporary highlights are not recorded.

## Processing Pipeline

```text
Input video or image sequence
  │
  ├─ Video: inspect with FFprobe and extract at the preset target FPS and working resolution; emit RGBA frames and masks for transparent media
  ├─ Images: sort by filename, keep all images, and generate masks for transparent PNGs
  ├─ COLMAP: auto-select CPU/CUDA for features; sequential matching for video, exhaustive for images
  ├─ COLMAP: incremental reconstruction and registration validation; Auto Optimize can attempt bridge-frame recovery
  ├─ Brush: train Gaussian Splats with an available GPU backend; Detailed adapts its profile to VRAM
  └─ Validate the PLY and atomically publish final.ply
```

As long as COLMAP produces at least one registered image and valid 3D points, the task continues to Brush. Registration below 80% produces a quality warning, but no longer stops automatically below 50%.

## System Requirements

- Windows 11, x64.
- WebView2 Runtime support.
- Video export requires WebCodecs AVC support in WebView2. Animation mode remains available when encoding is unavailable, and the UI reports why export is disabled.
- An available GPU graphics backend for Brush training; a discrete GPU is recommended.
- COLMAP CUDA acceleration requires an NVIDIA GPU, Windows driver 580.00 or newer, and Compute Capability 7.5 or higher. OOOSplat automatically uses CPU when these requirements are not met; no manual configuration is required.
- Enough disk space for source-media copies, input images, COLMAP data, Brush intermediate files, and the final PLY. Long videos, large image sequences, and higher quality presets can require substantial space.
- The installer uses a per-machine installation and may require administrator privileges.

The COLMAP build bundled on Windows supports both CPU and CUDA GPU execution. OOOSplat automatically selects the available backend before each task. Brush uses its own available graphics backend; its GPU detection and runtime are independent of COLMAP.

### macOS 15+ Alpha (Apple Silicon only)

> The current deliverable is an unsigned, unnotarized `.app`/`.dmg` Alpha for M1 or newer Apple Silicon Macs. Intel Macs and Universal Binaries are not supported.

- Bundles native arm64 FFmpeg 8.1.2, a real standalone FFprobe, COLMAP 4.2.1 Ceres CPU CLI-only, and OOOBrush ooo-v1.0.0 CLI.
- Users do not install Homebrew, and OOOSplat never falls back to a Homebrew or system `PATH` engine.
- COLMAP always uses CPU in this Alpha. Brush independently selects an available Metal graphics backend, and the UI explains this distinction.
- Gatekeeper may block the unsigned Alpha on first launch. In Finder, right-click the app and choose Open. Signing and notarization are planned for a later production release.

### Ubuntu 24.04 Alpha (x86_64 only)

> This Alpha delivers an x86_64 `.deb` package built on Ubuntu 24.04. It does not claim support for Ubuntu 22.04, other Linux distributions, or production deployment.

- Ubuntu 24.04 LTS, x86_64.
- A graphics backend and driver supported by Brush. Brush officially supports AMD, Intel, and NVIDIA GPUs. Current end-to-end validation used NVIDIA; CPU-only software graphics backends remain unverified but are not artificially blocked by startup checks.
- Source builds require Node.js 22.12+, Rust stable, and Tauri 2's WebKitGTK development dependencies; `.deb` users do not need these development tools.
- Ubuntu system `ffmpeg`/`ffprobe`; self-built, hash-locked COLMAP 4.2.1 is bundled, never selected from apt or PATH.
- OOOBrush ooo-v1.0.0 Linux x86_64 CLI, installed and verified by `npm run setup:engines`.

Install Ubuntu dependencies with:

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file ffmpeg \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev
```

Install a working Vulkan driver for the graphics adapter, such as the proprietary NVIDIA driver or Mesa for AMD/Intel. Bundled COLMAP uses CUDA/Caspar on compatible NVIDIA devices and CPU/Ceres otherwise; Brush selects its graphics backend independently. Fully CPU-only software Vulkan has not yet been validated end to end.

After downloading the `OOOSplat-0.6.0-x64-linux` Artifact from the main-branch GitHub Actions run, install it with:

```bash
sudo apt install ./OOOSplat-0.6.0-x64-linux.deb
```

The `.deb` installs only FFmpeg/FFprobe through Ubuntu's package manager; pinned COLMAP and Brush runtimes are included.

## Installation and Use

Sample media: [Download from Quark Drive](https://pan.quark.cn/s/1dde892a1324) to test or try the generation workflow.

1. On Windows, run `OOOSplat-0.5.0-x64-windows.exe`. On an Apple Silicon Mac, open `OOOSplat-0.5.0-arm64-macos.dmg` and drag OOOSplat into Applications. On Ubuntu 24.04, run `sudo apt install ./OOOSplat-0.5.0-x64-linux.deb`.
2. Start OOOSplat and confirm that the bundled engine status in the top bar is healthy. Use the `EN / 中文` action in the upper-right corner to switch the interface language instantly.
3. Under “01 Create New Task,” choose Video or Images from the input-type menu, then click the input field to select a video file or image-sequence folder.
4. Choose the projects root; OOOSplat remembers the last location.
5. Select the Fast, Balanced, or Detailed quality preset.
6. Review the automatically detected COLMAP acceleration status and its explanation, then select “Start Generation.”
7. Follow live stages, metrics, and logs on the left. When processing finishes, select “Preview” under “02 Task History.”
8. Edit the model in the “Adjust” mode under “03 Preview.” Changes are saved automatically; “Save” creates or updates `edit.ply`.
9. Switch to “Animation,” choose Portrait or Landscape in the center of the command bar, and use “Export → Export video” for a 23-second MP4. Both Adjust and Animation offer “Export → Export offline HTML” to share the current edits. Exports can be cancelled, and the output folder can be opened on completion.
10. To add detail, choose “High-res reshoot” on a completed project under “02 Task History.” Use the same device, lens, orientation, resolution, and zoom as the source project, and keep enough overlap with the original capture.

Usage notes:

- Drag the divider between the panels to resize them; double-click it to restore the default ratio.
- Use the percentage button in the lower-right corner to reduce, reset, or increase the interface scale.
- The input media, project directory, and quality preset cannot be changed while a task is running.
- “Delete” moves the complete project—including the source copy and intermediate files—to the Recycle Bin. OOOSplat does not fall back to permanent deletion if that operation fails.

## Quality Presets

“Auto Optimize (Experimental)” is enabled by default. Video is scaled once during extraction to the selected working resolution while preserving aspect ratio and never upscaling lower-resolution media. Actual sampling never exceeds the source FPS or total frame count. Image sequences keep every valid image at its original input resolution, while COLMAP and Brush still apply the selected preset's processing limits.

| Preset | Initial / bridge-frame ceiling | Video working long edge | COLMAP long edge / maximum features | Brush training |
| --- | ---: | ---: | ---: | --- |
| Fast | 6 / 9 FPS | Up to 1,600 | 1,200 / 4,096 | 8,000 iterations, maximum resolution 1,600 |
| Balanced | 8 / 12 FPS | Up to 1,920 | 1,600 / 8,192 | 15,000 iterations, maximum resolution 1,920 |
| Detailed | 12 / 15 FPS | Up to 3,200, 3,840, or native resolution according to VRAM | 3,200 / 16,384 | 30,000 iterations, with maximum resolution and Splat limit selected from VRAM |

Detailed starts from one of three VRAM profiles. Below 8 GB, or when VRAM cannot be read, the video and Brush long edges are capped at 3,200. From 8 GB inclusive to below 12 GB, they are capped at 3,840. At 12 GB or more, video stays at its native long edge and Brush uses that native long edge. All three Detailed profiles cap the COLMAP image long edge at 3,200. Bridge-frame recovery is attempted only when the initial video reconstruction has insufficient coverage and unused candidate frames remain. A failed attempt safely returns to the initial reconstruction, and the mechanism does not apply to image sequences. Detailed performs at most one automatic retry with a lower training profile after an explicit out-of-memory failure.

With Auto Optimize disabled, Fast, Balanced, and Detailed return to the legacy 30%, 50%, and 100% source-frame retention strategy. Video uses the legacy maximum long edge of 1,920, Brush resolutions return to 1,200, 1,600, and 2,000, and no additional Splat-count limit is applied.

## Project and File Locations

Each generation creates a separate directory under the projects root:

```text
<projects-root>\<yyyyMMdd-HHmmss_source-name>\
  final.ply             Final Gaussian Splatting file
  project.json          Project metadata and result metrics
  edit.ply              Current edited result; safely replaced on later saves (optional)
  preview.mp4           First optional animation-preview video export
  preview-2.mp4         Later video exports are automatically numbered
  preview-landscape.mp4 Landscape video (later exports are automatically numbered)
  preview.html          Offline viewer (later exports use preview-2.html, etc.)
  state.json            Pipeline state
  source\
    input.<ext>         Source-video copy (video projects)
    images\             Normalized source copies (image-sequence projects)
  work\
    frames\             Extracted or prepared input images
    masks\              COLMAP masks for transparent input (when needed)
    colmap\             COLMAP database and sparse reconstruction
    brush\              Brush dataset and training intermediates
  logs\                 Complete FFmpeg, COLMAP, Brush, and pipeline logs
```

Windows-invalid characters are removed from project names. Name collisions receive suffixes such as `-2` and `-3`.

Application settings and the project index are stored in:

```text
%LOCALAPPDATA%\SplatStudio\settings.json
%LOCALAPPDATA%\SplatStudio\project-index.json
%LOCALAPPDATA%\SplatStudio\telemetry.json
```

## Anonymous Usage Statistics

OOOSplat collects anonymous usage statistics by default to track stability and per-stage timings. Turn it off at any time under **Settings -> Privacy** in the top right; no anonymous analytics requests are sent once it is off.

What is sent:

| Field | Description |
| --- | --- |
| Install ID | A random UUID generated on first launch. No hardware serial, MAC address, or device fingerprint is read |
| App version, OS, CPU architecture | For example `0.6.0` / `windows` / `x86_64` |
| Event name | `daily_active`, `generation_started`, `generation_completed`, `generation_failed`, `pipeline_stage_completed`, `planner_evaluation`. `daily_active` is sent at most once a day, and once more on the day the app version changes |
| Quality preset and input type | Enumerated values such as `balanced` / `video`; an image-sequence input reports `images` |
| Stage and total durations | Milliseconds |
| Frame count and video duration | Bucketed values, not raw counts |
| Auto Optimize effectiveness metrics | Anonymous media dimensions and counts, plan, registration result, bridge recovery, Brush configuration, Splat count, and stage timings used to compare planner effectiveness |
| Failing stage and error code | Enumerated values such as `colmap_mapper_failed`; no raw error text |

Anonymous analytics never sends: source media, including videos, images, and PLY files; file names, paths, and project names; logs and command output; user names or any personal information.

### Voluntary error reports

After generation fails, choose **Send error report**, review the complete redacted report, then choose **Agree and send**. Reports include the failure reason, error details, the latest failure-time log excerpt (up to 200 lines / 64 KiB), app version, system version, and all detectable graphics cards. They exclude media, models, device serials, and anonymous installation identifiers. Paths, filenames, and usernames are removed. Unavailable hardware fields are left empty; the actual GPU is never guessed.

Consent is separate and one-time: you can send a report even with anonymous analytics disabled, without enabling it. Cancelling the preview uploads nothing; failures can only be retried manually, and successful delivery shows a report ID. Local reports and previews remain valid for the current app session without a review time limit, and are cleared when the app closes. Reports received by the server are retained for 30 days. If the receiving service has not been deployed or the network is unavailable, the app reports delivery failure rather than claiming success. See the [receiving service integration guide](server/diagnostics/README.md).

## Bundled Engines

| Engine | Pinned version/build | Purpose |
| --- | --- | --- |
| FFmpeg / FFprobe | Windows x64 8.1 LGPL shared; macOS arm64 8.1.2 LGPL shared | Video analysis and frame extraction |
| COLMAP | 4.2.1 same commit; Windows/Linux CUDA + Caspar/Ceres; macOS Ceres CPU | Feature extraction, matching, and camera reconstruction |
| Brush | OOOBrush ooo-v1.0.0; Windows/Linux x64, macOS arm64 | Headless Gaussian Splatting training and PLY export |

Windows, Ubuntu, and macOS source and integrity policies are recorded in [`engines/manifest.json`](engines/manifest.json), [`engines/manifest.linux.json`](engines/manifest.linux.json), and [`engines/manifest.macos.json`](engines/manifest.macos.json). Large engine files are not committed to Git; developers restore them with `npm run setup:engines`. Release builds verify sources, hashes, architecture, the dynamic-library closure, and Brush CLI compatibility.

Third-party licenses and notices are in [`licenses/`](licenses/):

- FFmpeg: LGPL-2.1-or-later; the selected build disables GPL/nonfree components.
- COLMAP: BSD-3-Clause.
- Brush: Apache-2.0.

## Local Development

### Download COLMAP Development Packages

Manually run **COLMAP-only development runtime** in Actions, selecting `all` or
one platform. Download the successful job's Artifact, extract its outer ZIP and
inner runtime archive, and copy the top-level directory's **contents** into:

- Windows: `engines/colmap/` (`bin/colmap.exe`).
- Linux: `engines/linux/colmap/` (`bin/colmap`).
- macOS: `engines/macos/arm64/colmap/` (`bin/colmap`).

Keep libraries, licenses and metadata together; do not add another wrapper
directory. Keep your existing FFmpeg, then use `npm run dev:local` or
`npm run build:local`. These commands automatically prepare and cache pinned
OOOBrush, but do not install, download or verify COLMAP, or modify formal manifests. Application runtime health
checks and strict normal release verification remain unchanged.

Artifacts expire after 30 days. The manual workflow must exist on the default
branch before it can be dispatched. See the [engine development package guide](engines/README.md#download-colmap-only-development-builds).

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

Ubuntu setup restores verified COLMAP under `engines/linux/colmap/` and Brush under `engines/linux/brush/`. FFmpeg/FFprobe remain system packages. The `.deb` embeds COLMAP/Brush and does not depend on system COLMAP.

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
dist-artifacts\OOOSplat-0.6.0-x64-windows.exe
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

# Use the same command with a folder for an image sequence
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- generate "D:\Photos\object" --projects-root "D:\Splat Projects" --quality balanced
```

For development or diagnostics, use the global `--engine-dir <path>` argument or the `OOOSPLAT_ENGINE_DIR` environment variable to override the default engine directory.

Linux FFmpeg/FFprobe retain their individual environment/PATH discovery. Brush defaults to managed `linux/brush/brush_app`, with no PATH fallback; explicit `OOOSPLAT_BRUSH` diagnostic overrides must support the new CLI. COLMAP is resolved only as `linux/colmap/bin/colmap` below the managed root, with no individual `OOOSPLAT_COLMAP` or PATH fallback.

## FAQ

### Why is COLMAP using the CPU instead of the GPU?

OOOSplat enables COLMAP GPU acceleration only when the bundled CUDA runtime is healthy and it can confirm an NVIDIA driver version of at least 580.00 and Compute Capability 7.5 or higher. Caspar additionally requires a successful tiny BA execution probe. A failed Caspar probe selects Ceres CPU without disabling otherwise available SIFT GPU acceleration. Detection failures select CPU and report the reason. Brush is independent of COLMAP and selects an available graphics backend at runtime.

### How does OOOSplat handle different GPU combinations?

COLMAP and Brush are separate stages. COLMAP requires a compatible NVIDIA CUDA environment for GPU acceleration. Brush uses Vulkan on Windows and Linux and Metal on Apple Silicon macOS. Common configurations are handled as follows; rows without an explicit platform refer to Windows:

| Platform and GPU configuration | COLMAP | Brush training | Detailed Auto Optimize VRAM profile |
|---|---|---|---|
| Windows: one NVIDIA discrete GPU | Uses CUDA when the driver and Compute Capability requirements are met | Explicitly selects the first discrete GPU and runs through Vulkan | Uses that NVIDIA GPU's total VRAM to select Low, Standard, or Large |
| Intel integrated GPU + one NVIDIA discrete GPU | Uses the compatible NVIDIA GPU through CUDA | Explicitly selects the first discrete GPU instead of the Intel integrated GPU | Uses NVIDIA total VRAM |
| AMD integrated GPU + one NVIDIA discrete GPU | Uses the compatible NVIDIA GPU through CUDA | Explicitly selects the first discrete GPU and bypasses the AMD Switchable Graphics implicit layer for the Brush child process only | Uses NVIDIA total VRAM |
| AMD only, or Intel integrated GPU + AMD discrete GPU | Falls back to CPU | Vulkan automatically selects an AMD GPU; the AMD graphics layer is not disabled | Currently uses the conservative Low profile |
| Intel integrated or discrete GPU only | Falls back to CPU | Vulkan automatically selects an Intel GPU | Currently uses the conservative Low profile |
| Multiple NVIDIA GPUs | Selects the compatible GPU with the highest Compute Capability, then the lower NVIDIA index on a tie | Does not force a Brush device index because Vulkan and CUDA indices cannot be mapped reliably; Vulkan selects automatically | Uses the total VRAM of the GPU selected for COLMAP |
| AMD discrete GPU + NVIDIA discrete GPU | Uses a compatible NVIDIA GPU for COLMAP | Currently enters the single-NVIDIA preference path, but cross-vendor Vulkan ordering cannot be guaranteed; also set `brush_app.exe` to High performance in Windows Graphics settings and verify the actual adapter in the task log | Uses NVIDIA total VRAM |
| Old NVIDIA driver, unsupported NVIDIA model, or failed detection | Falls back to CPU | Brush still attempts to select an available GPU through Vulkan but does not force NVIDIA | Currently uses the conservative Low profile |
| No usable GPU graphics backend | Uses CPU for COLMAP | Brush may not start; CPU-only software graphics has not been validated end to end | Not applicable |
| macOS: Apple Silicon M series, including Pro, Max, and Ultra | Currently uses the CPU; CUDA is not enabled | Automatically uses the M-series Metal GPU and unified memory | Currently stays on the conservative Low profile instead of scaling from unified-memory capacity |
| Ubuntu Alpha: NVIDIA, AMD, or Intel GPU | Currently uses the CPU | Vulkan automatically selects an available GPU | Currently uses the conservative Low profile |

Forced adapter selection and the AMD implicit-layer workaround are passed only to the individual `brush_app.exe` child process. OOOSplat does not modify the system environment, driver configuration, or other applications, and it does not switch Brush to D3D12. AMD-only systems do not receive the AMD implicit-layer workaround.

If the Brush log reports `Device Lost`, OOOSplat distinguishes it from an out-of-memory failure and shows guidance, but it does not automatically lower quality and retry. Connect the computer to power, close other GPU-heavy applications, and set `brush_app.exe` to High performance under Windows Settings → System → Display → Graphics. Detailed quality lowers its profile and retries at most once only after an explicit out-of-memory failure.

M-series CPUs and GPUs share unified memory, but the current Planner does not treat unified memory as dedicated VRAM. Consequently, even an M-series Pro, Max, or Ultra with substantial memory currently uses the Detailed Low profile. This is a conservative compatibility policy; it does not mean Metal training can access only a small portion of unified memory.

### Why does OOOSplat warn about a low registration rate?

A low registration rate usually means COLMAP could not find enough continuous, overlapping viewpoints. The task still continues to Brush, but result quality may be affected. Use an orbit video with stable exposure, clear frames, continuous movement, and sufficient viewpoint overlap. Avoid fast rotation, strong reflections, large plain-color areas, and moving subjects.

### Why does a project use so much disk space?

Each project keeps a source-video copy, extracted frames, COLMAP data, and Brush intermediates for diagnostics and traceability. After confirming the result, use “Delete” in Task History to move the complete project to the Recycle Bin.

### Can I view final.ply directly in OOOSplat?

Yes. Select “Preview” on a completed project in “02 Task History” to open its `.ply` in the dedicated preview workspace. “Adjust” supports whole-model Transform plus rectangle, sphere, and box Gaussian selection, with non-destructive deletion, cropping, undo, and redo. Edit state is stored in the project; “Save” creates or updates the single `edit.ply`, while the original `final.ply` remains unchanged. “Animation” adds a 5-second reveal, an 8-second shockwave, a continuous orbit, and watermarked portrait MP4 export. `.sog` and `.spz` are not supported yet.

## Technology

The preview Export menu supports portrait/landscape video and single-file offline HTML. HTML initially displays the complete edited result, with manual animation playback that pauses on mouse interaction. It prefers WebGPU and falls back to WebGL2, and contains no project paths, source media, logs, or telemetry. Development and build commands generate the offline runtime first; before running Rust alone, run `npm run build:html-viewer` if needed.

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
