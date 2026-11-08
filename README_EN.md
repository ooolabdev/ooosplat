<p align="center">
  <img src="assets/readme-logo.svg" alt="OOOSplat Logo" width="180">
</p>

<h1 align="center">OOOSplat</h1>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-blue?style=flat" alt="License: Apache-2.0"></a>
  <a href="https://github.com/ooolabdev/ooosplat/releases/tag/0.7.1"><img src="https://img.shields.io/badge/release-v0.7.1-blue?style=flat" alt="Release: v0.7.1"></a>
  <a href="#system-requirements"><img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-blue?style=flat" alt="Platforms: Windows, Linux, macOS"></a>
</p>

<p align="center">
  <a href="https://github.com/ooolabdev/ooosplat/releases/tag/0.7.1"><strong>Download</strong></a> | <a href="README.md">中文</a> | <a href="README_EN.md">English</a> | <a href="README_JA.md">日本語</a>
</p>

<p align="center">
  <a href="https://trendshift.io/repositories/177239?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-177239" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/177239/daily?language=Rust" alt="OOOSplat — Trendshift Rust Repository of the Day" width="250" height="55" /></a>
  <a href="https://trendshift.io/repositories/177239?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-177239" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/177239/weekly?language=Rust" alt="OOOSplat — Trendshift Rust Repository of the Week" width="250" height="55" /></a>
</p>

OOOSplat is a local desktop application that turns ordinary or panorama videos and image sequences into 3D Gaussian Splatting projects. Choose source media, a project directory, and a quality preset, then generate, preview, edit, and export the result.

Windows and the Apple Silicon macOS Alpha provide FFmpeg, FFprobe, COLMAP, and Brush with the application. Linux support remains limited to an Ubuntu 24.04 LTS x86_64 Alpha. Every generation stage runs on the user's own CPU and GPU; input media, project data, models, and logs do not need to be uploaded to a cloud reconstruction or training service. The React interface calls the local Rust backend directly. Normal generation and preview do not require a separate HTTP server.

See the [OOOSplat Roadmap](ROADMAP_EN.md) for planned work.

> 0.7.1 updates generation defaults, automatic retries, and Detailed splat budgets, adds stepped frame sampling and movement-speed controls, and improves reshoots, image GPS assistance, exports, and error diagnostics.

The features below describe the current `main` source tree; consult each release's notes for installer contents.

## Why OOOSplat

- **One-click Gaussian generation**: Select an input video or image-sequence folder, project directory, and quality preset. OOOSplat then runs image preparation, COLMAP camera reconstruction, Brush training, and `final.ply` publishing without manual engine setup or command-line orchestration.
- **Cross-platform compatibility**: Supports Windows, macOS, and Linux, bringing local Gaussian Splat generation to all three major desktop platforms. See the compatibility section below for specific OS and processor requirements.
- **Security and privacy protection**: Source media, extracted frames, camera reconstruction data, Gaussian models, and logs stay in the user-selected local project directory by default. The core generation workflow runs on the user's machine, so original videos, images, and models do not need to be uploaded to third-party reconstruction or training platforms. This reduces exposure risks during network transfer, cloud retention, and unauthorized access.
- **Fully local compute**: Reconstruction and training run on the user's own machine without remote compute services. COLMAP automatically uses a compatible local NVIDIA GPU when available and falls back to CPU otherwise, keeping both processing and data under the user's control.

## Demo Video

https://github.com/user-attachments/assets/5b9e8cef-4c71-4bfa-ba23-641fcdd37659

## Interface Preview

### Create and Manage Tasks

![OOOSplat create-task and task-list workspace](assets/screenshots/task-workspace.png)

### Preview and Adjust Gaussian Splats

![OOOSplat Gaussian Splat preview and transform workspace](assets/screenshots/gaussian-preview.png)

## Key Features

- **One-click generation**: Create a Gaussian Splat from MP4/MOV video or a JPG, JPEG, and PNG image folder. OOOSplat handles image preparation, camera reconstruction, training, and PLY publishing.
- **Panorama media**: Supports stitched, monoscopic, full 360°×180°, 2:1 ERP MP4 and image folders. Media types are detected automatically; ambiguous inputs require an inline choice.
- **Image-order confirmation**: For ordinary or panorama folders with more than 100 images, confirm capture order before generation. Ordered sets use sequential matching; unordered sets and sets of up to 100 images use exhaustive matching.
- **Image GPS**: Detect embedded location metadata and assist matching and reconstruction on the validated Windows x64 path. Reliable observations provide estimated north orientation and metric scale; otherwise the original workflow is used. [Scope and validation](docs/IMAGE_GPS.md)
- **External model import**: Import standard/compressed PLY, SOG, SPZ, SPLAT, and KSPLAT from the Completed group. Complete COLMAP data is detected automatically; eligible projects can be reshot after compatibility checks. External files remain read-only.
- **Three quality presets**: Reconstruction optimizations and frame backfill are standard. Adjust video sampling in discrete steps, with at most 20% extra frames for manually sampled runs. Detailed preserves source training resolution and budgets ordinary-media splats from effective pixels and reconstruction tracks (coefficient 0.5). Panoramas retain a separate budget.
- **Automatic retry**: Enabled by default; permits a lower splat ceiling after GPU OOM or a reshoot-plan fallback. Completed details mark retries that actually ran.
- **Transparent media**: Transparent MOV and PNG files retain their original color and Alpha while OOOSplat creates the black-and-white masks required by COLMAP.
- **Multi-task workspace**: Group tasks into New Tasks, Completed, and Unfinished. Keep multiple drafts, reorder them, and automatically run the next task. Only one generation task runs at a time; tasks can be cancelled or resumed.
- **Reshoots**: Choose reconstruction mode and whether to reuse the original model, with the normal training recipe. Supports mixed-size ordinary inputs and full ERP reshoots in a separate derived project.
- **Preview and editing**: Supports WebGPU/WebGL2, translation axes, rotation rings, cropping, deletion, inversion, undo, and redo. Static views stop redrawing; interactions run at up to 60 FPS. Use horizontal WASD at 0.1×–5.0× speed or return to the first training view.
- **Multiple export formats**: Export standard PLY, compressed PLY, SPZ v4, bundled SOG, portrait or landscape MP4, and standalone offline HTML. Out-of-range SPZ coordinates can be adjusted on the export copy after confirmation.
- **Local processing**: COLMAP reconstruction and Brush training run on the user's computer, using GPU acceleration when available.
- **Cross-platform and bilingual**: Supports Windows, Apple Silicon macOS, and Ubuntu, with Simplified Chinese and English interfaces.

Each panorama batch needs at least two same-sized images; a 2:1 ratio alone does not confirm ERP. Dual-fisheye, stereo, VR180, cropped panoramas, private originals, and mixed ordinary/panorama reshoots are unsupported. Panorama caches have no fixed 20 GiB cap; reserve enough disk space. The per-image decode limit remains 512 MiB. See the [video](docs/PANORAMA_VIDEO_V1.md), [image](docs/PANORAMA_IMAGES_V1.md), and [reshoot notes](docs/RESHOOT_V2.md) for verification limits.

Compressed PLY, SPZ, and SOG exports no longer reject models based on estimated memory, but actual RAM, disk space, and format constraints still apply. Removing these gates does not guarantee large-model exports will succeed. See [export notes and verification](docs/model-export-verification.md).

### Gaussian Editing Shortcuts

- In Rectangle Select, drag with the left mouse button to replace the selection, use `Shift + drag` to add, and `Ctrl + drag` to remove. Use the middle mouse button to orbit, the right mouse button to pan, and the wheel to zoom.
- Yellow highlights show the temporary selection. Press `Delete` or `Backspace` to remove selected Gaussians non-destructively, or `Esc` to clear the temporary selection.
- In rectangle, sphere, or box selection, choose Invert selection or press `Ctrl + I` (`⌘ + I` on macOS) to switch which points are selected or retained.
- Sphere and box selection enter an orthographic view automatically. Switch between side, front, and top views, then adjust the volume in the viewport or through numeric fields.
- Use “Scene gizmo” in the Transform panel to switch between Move and Rotate; click the selected button again to turn the gizmo off. Left-drag an axis or ring to edit the model; drag empty space to orbit the camera. Both rings and numeric rotation pivot around the model center. Releasing commits one edit; `Esc` cancels the current gizmo drag. The gizmo mode is saved locally.
- Use `WASD` to move on the horizontal plane and `Shift` for 3× speed. Completed edits return canvas focus; the bottom-right slider adjusts speed from 0.1× to 5.0× and remembers this device's preference. Speed follows zoom; perspective views only.
- Use `Ctrl + Z` to undo and `Ctrl + Shift + Z` or `Ctrl + Y` to redo. Transform, crop, and deletion changes share one history; camera movement and temporary highlights are not recorded.

## Processing Pipeline

```text
Input video or image sequence
  │
  ├─ Video: inspect with FFprobe and extract at the preset target FPS or manual ratio and working resolution; emit RGBA frames and masks for transparent media
  ├─ Images: scan headers and naturally sort filenames, copy source media, prefer hard links for internal reuse, and create Alpha masks when needed
  ├─ COLMAP: auto-select CPU/CUDA for features; sequential video matching, exhaustive images by default, ordered matching above 100 images, and preset loop detection
  ├─ COLMAP: incremental reconstruction and registration validation, with bridge-frame recovery; two-view triangulation only for actual transparent images
  ├─ Panoramas: retain full ERP images for spherical reconstruction, then convert registered images to six-face training data
  ├─ Brush: train Gaussian Splats with an available GPU backend; Detailed plans training at source resolution
  └─ Validate the PLY and atomically publish final.ply
```

A normal generation task continues to Brush when COLMAP produces at least one registered image and valid 3D points. Registration below 80% produces a quality warning rather than stopping the task solely for low coverage. A reshoot also requires at least one newly added image to register.

Image-sequence preflight reads only JPG/PNG headers to check counts, dimensions, and Alpha channels, without decoding all pixels. An Alpha channel does not necessarily contain transparent pixels; generation checks actual transparency and enables masks only when needed. Projects still keep an independent source copy. Internal images use hard links when supported and fall back to copying, with progress and logs throughout import and preparation.

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
- Gatekeeper may block the unsigned Alpha on first launch. After confirming the app's source and integrity, follow [Apple's opening instructions](https://support.apple.com/en-us/102445) to use Open Anyway under System Settings → Privacy & Security. [macOS 15 and later no longer support the Control-click override](https://developer.apple.com/news/?id=saqachfa).

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

After downloading the `OOOSplat-0.7.1-x64-linux` Artifact from the main-branch GitHub Actions run, install it with:

```bash
sudo apt install ./OOOSplat-0.7.1-x64-linux.deb
```

The `.deb` installs only FFmpeg/FFprobe through Ubuntu's package manager; pinned COLMAP and Brush runtimes are included.

## Installation and Use

Sample media: [Download from Quark Drive](https://pan.quark.cn/s/1dde892a1324) to test or try the generation workflow.

1. On Windows, run `OOOSplat-0.7.1-x64-windows.exe`. On an Apple Silicon Mac, open `OOOSplat-0.7.1-arm64-macos.dmg` and drag OOOSplat into Applications. On Ubuntu 24.04, run `sudo apt install ./OOOSplat-0.7.1-x64-linux.deb`.
2. Start OOOSplat and confirm that the bundled engine status in the top bar is healthy. Use the `EN / 中文` action in the upper-right corner to switch the interface language instantly.
3. Select a draft under “Task List → New Tasks,” or choose Add, then select a video or image folder. Wait for detection; choose ordinary or panorama processing in the media box if the type is uncertain.
4. Choose the projects root; OOOSplat remembers the last location.
5. Select the Fast, Balanced, or Detailed quality preset.
6. Review COLMAP acceleration and select “Start Generation.” For more than 100 images, confirm whether filenames follow capture order; an answer is required before starting.
7. Follow the selected task's stages, progress, and logs on the left. Click any task on the right to switch details. Generated projects appear under Completed or Unfinished; completed projects offer Preview.
8. Edit the model in “Preview → Adjust.” Transforms, crops, and deletions are saved automatically in the project; each project's view is stored locally on this device. “Save edits” confirms that the current edits and view have been saved. Choose standard PLY, compressed PLY, SPZ, or SOG directly from “Export,” with no second format confirmation. Both Adjust and Animation support model export.
9. Switch to Animation, choose Portrait or Landscape, and use Export → Export video for a 30 FPS, 23-second MP4 at 1080×1920 or 1920×1080 respectively. Both Adjust and Animation can export offline HTML. Exports use the current edits, can be cancelled, and provide access to the output folder on completion.
10. To add detail, choose Reshoot on a completed project. Select the new media and type, optionally adjust quality/Automatic retry, then answer the model-condition question. Ordinary inputs also ask whether capture settings are the same; there is no separate plan confirmation. Enabling Automatic retry allows a slower fallback if needed. New and old views must overlap and show the same scene.

Usage notes:

- Drag the divider between the panels to resize them; double-click it to restore the default ratio.
- Use the percentage button in the lower-right corner to reduce, reset, or increase the interface scale.
- Drafts are saved locally and their media is rechecked after restart. Running-task parameters cannot be changed, but other drafts remain editable. A second task cannot run concurrently.
- Drag new tasks to reorder them. Auto-run next task is off by default and never skips an unready task. Image-order questions pause startup; cancelling disables the queue.
- New reshoots inherit source quality and default Automatic retry to enabled, editable before launch. Ordinary batches can differ in resolution, aspect ratio, device, and lens; full ERP batches can differ in size across batches. Original media cannot be submitted again. Missing trusted image/reconstruction snapshots prevent launch.
- Reusing the original model initializes from the unedited generated PLY and trains on old and new views with the normal full recipe, without shortened steps or reduced learning rates. Original intrinsics are reused only after confirmation and compatible single-camera dimensions/orientation checks. This is not an optimizer resume or a guarantee that only new regions change; deletion/crop edits are not inherited. Existing tasks keep their saved settings. See the [reshoot validation notes](docs/RESHOOT_V2.md).
- Task pages keep the latest 500 log entries; complete engine logs remain in the project's `logs/` directory. The training runtime monitor is hidden by default and can be enabled under Settings → Interface.
- Delete moves the complete project, including source copies and intermediate files, to the Recycle Bin. OOOSplat does not fall back to permanent deletion if that operation fails.

## Quality Presets

Sequential video and confirmed ordered-image matching use preset loop detection by default; valid loops are not guaranteed. Reshoots match old/new views separately, with window 10 and the preset loop policy among new ordered views. Backfill is independent of Automatic retry.

For new manually sampled runs, initial registration below 80% with a valid initial model can trigger one targeted bridge attempt in unregistered intervals. It adds at most 20% of the actual initial frame count, capped by available source frames: 360 initial frames permit at most 72 extra; 100% sampling adds none. The 30-frame minimum still applies, or every available frame for shorter sources. Default sampling retains the candidate budgets below. Interrupted older runs retain their saved rules. The 1.2× ceiling controls frame counts, not time or success probability. See [bridge rules and validation](docs/MANUAL_BRIDGE_V3.md).

Previous optimization behavior is now standard. Fast/Balanced retain working-resolution limits; Detailed uses source training resolution without upscaling. Images keep every valid input; video sampling uses discrete steps above the quality presets, including the exact source-FPS-derived default. See [generation defaults](docs/GENERATION_POLICY.md) and the [legacy configuration guide](docs/pipeline-optimization-config.en.md).

The table below describes ordinary inputs. Ordinary Detailed budgets splats from registered training views' effective pixels and mean reconstruction track length, with coefficient 0.5. Fully transparent pixels are excluded; initialization points are never silently removed. See the [Detailed budget formula and scope](docs/FINE_SPLAT_BUDGET.md).

Panoramas use dedicated training recipes and point budgets. Detailed retains native ERP and derives faces at `floor(ERP width / 4)` without upscaling. Point budgets use the original spherical model and are not multiplied by six. Caps guarantee neither achieved counts, quality, nor sufficient memory.

| Preset | Default initial / bridge-candidate target | Video working long edge | COLMAP long edge / maximum features | Brush training |
| --- | ---: | ---: | ---: | --- |
| Fast | 6 / 9 FPS | Up to 1,600 | 1,200 / 4,096 | 8,000 iterations, maximum resolution 1,600 |
| Balanced | 8 / 12 FPS | Up to 1,920 | 1,600 / 8,192 | 15,000 iterations, maximum resolution 1,920 |
| Detailed | 12 / 15 FPS | Source resolution | 3,200 / 16,384 | 30,000 iterations, source training resolution, effective-pixel/track budget (coefficient 0.5) |

Short videos target a minimum of 30 selected frames when the source provides enough frames. Initial and candidate sampling rates can therefore exceed the table's targets, but never exceed the source FPS or frame count.

Detailed training resolution no longer uses VRAM tiers. With Automatic retry enabled, all three presets may retry once after explicit GPU OOM, at 80% of the failed process's last trustworthy splat count, without lowering resolution. Missing counts, no actual reduction, or budgets below initialization fail directly. Backend compatibility fallbacks and frame backfill remain enabled independently.

Interrupted tasks retain frozen settings. Manually retrying a failed task uses the new policy and archives incompatible work caches, preserving source files, published models and edits.

## Project and File Locations

Each generation creates a separate directory under the projects root:

```text
<projects-root>\<yyyyMMdd-HHmmss_source-name>\
  final.ply             Final Gaussian Splatting file
  project.json          Project metadata and result metrics
  edit.ply              Exported edits; safely replaced on later PLY exports (optional)
  edit.compressed.ply   Compressed PLY export (optional)
  edit.spz              SPZ v4 export (optional)
  edit.sog              Bundled SOG export (optional)
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
    colmap_frames\      Dedicated COLMAP images, byte-identical to prepared input
    masks\              COLMAP masks for transparent input (when needed)
    reshoot-masks\      COLMAP masks for reshoot input (when needed)
    colmap\             COLMAP database and sparse reconstruction
    brush\              Brush dataset and training intermediates
  logs\                 Complete FFmpeg, COLMAP, and Brush engine logs
```

Windows-invalid characters are removed from project names. Name collisions receive suffixes such as `-2` and `-3`.

Application settings and the project index are stored in:

```text
%LOCALAPPDATA%\SplatStudio\settings.json
%LOCALAPPDATA%\SplatStudio\project-index.json
%LOCALAPPDATA%\SplatStudio\tasks.json
%LOCALAPPDATA%\SplatStudio\telemetry.json
```

These are Windows paths; macOS and Linux use their local application-data directories. Unstarted drafts, UI preferences, and preview views live in local WebView storage and do not travel with project files.

## Anonymous Usage Statistics

OOOSplat collects anonymous usage statistics by default to track stability and per-stage timings. Turn it off at any time under **Settings -> Privacy** in the top right; no anonymous analytics requests are sent once it is off.

What is sent:

| Field | Description |
| --- | --- |
| Install ID | A random UUID generated on first launch. No hardware serial, MAC address, or device fingerprint is read |
| App version, OS, CPU architecture | For example `0.7.1` / `windows` / `x86_64` |
| Event name | `daily_active`, `generation_started`, `generation_completed`, `generation_failed`, `pipeline_stage_completed`, `planner_evaluation`. `daily_active` is sent at most once a day, and once more on the day the app version changes |
| Quality preset and input type | Enumerated values such as `balanced` / `video`; an image-sequence input reports `images` |
| Stage and total durations | Milliseconds |
| Frame count and video duration in basic generation events | Bucketed values; Planner evaluation also includes actual media and reconstruction counts |
| Auto Optimize effectiveness metrics | Anonymous media dimensions and counts, plan, registered images and 3D points, bridge recovery, Brush configuration, Splat count, PLY size, and stage timings. Generation, resume, and reshoot all record them, preserving existing field semantics rather than substituting retry consent for planner status |
| Planner GPU information | Vendor enumeration and detected total VRAM, without full adapter names or device serials. Unavailable values remain empty or unknown |
| Failing stage and error code | Enumerated values such as `colmap_mapper_failed`; no raw error text |

Anonymous analytics never sends: source media, including videos, images, and PLY files; file names, paths, and project names; logs and command output; user names or any personal information.

### Voluntary error reports

After generation fails, choose **Send error report**, review the complete redacted report, then choose **Agree and send**. Reports include the failure reason, error details, the latest failure-time log excerpt (up to 200 lines / 64 KiB), app version, system version, and all detectable graphics cards. They exclude media, models, device serials, and anonymous installation identifiers. Paths, filenames, and usernames are removed. Unavailable hardware fields are left empty; the actual GPU is never guessed.

Consent is separate and one-time: you can send a report even with anonymous analytics disabled, without enabling it. Cancelling the preview uploads nothing; failures can only be retried manually, and successful delivery shows a report ID. Local reports and previews remain valid for the current app session without a review time limit, and are cleared when the app closes. Reports received by the server are retained for 30 days. If the receiving service has not been deployed or the network is unavailable, the app reports delivery failure rather than claiming success. See the [receiving service integration guide](server/diagnostics/README.md).

## Bundled Engines

| Engine | Pinned version/build | Purpose |
| --- | --- | --- |
| FFmpeg / FFprobe | Windows x64 pinned 8.1-series LGPL shared month-end build (n8.1.3-9-g29e619e767); macOS arm64 8.1.2 LGPL shared; Ubuntu system packages | Video analysis and frame extraction |
| COLMAP | 4.2.1 same commit; Windows/Linux CUDA + Caspar/Ceres; macOS Ceres CPU | Feature extraction, matching, and camera reconstruction |
| Brush | OOOBrush ooo-v1.0.0; Windows/Linux x64, macOS arm64 | Headless Gaussian Splatting training and PLY export |

Windows, Ubuntu, and macOS source and integrity policies are recorded in [`engines/manifest.json`](engines/manifest.json), [`engines/manifest.linux.json`](engines/manifest.linux.json), and [`engines/manifest.macos.json`](engines/manifest.macos.json). Large engine files are not committed to Git; developers restore them with `npm run setup:engines`. Release builds verify sources, hashes, architecture, the dynamic-library closure, and Brush CLI compatibility.

Third-party licenses and notices are in [`licenses/`](licenses/):

- FFmpeg: LGPL-3.0-or-later on Windows, LGPL-2.1-or-later on macOS; both builds disable GPL/nonfree components.
- COLMAP: BSD-3-Clause.
- Brush: Apache-2.0.

## Local Development

### Download COLMAP Development Packages

On Windows and Ubuntu, `npm run setup:engines` downloads and verifies COLMAP and Brush. Windows also restores pinned FFmpeg/FFprobe; Ubuntu uses system FFmpeg/FFprobe. macOS prepares its runtime through the Apple Silicon development workflow below.

- COLMAP is downloaded from [**ooosplat-colmap**](https://github.com/ooolabdev/ooosplat-colmap).
- Brush is downloaded from [**OOOBrush**](https://github.com/ooolabdev/OOOBrush).

For manual installation, download the package for your platform from each repository's Releases page. Copy the extracted COLMAP contents into:

- Windows: `engines/colmap/` (`bin/colmap.exe`).
- Linux: `engines/linux/colmap/` (`bin/colmap`).
- macOS: `engines/macos/arm64/colmap/` (`bin/colmap`).

Keep the runtime libraries, licenses, and metadata together without adding another wrapper directory. `npm run dev:local` and `npm run build:local` also prepare and verify the pinned OOOBrush release.

See the [engine development package guide](engines/README.md) for directory layout and verification details.

### Windows Development Environment

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

The download-and-start workflow above requires the mixed macOS runtime to be hash-locked. Its distribution hashes in `engines/manifest.macos.json` are currently empty, so regular `setup:engines` rejects installation. Initial local development must follow the [macOS CI workflow](.github/workflows/macos.yml) to build and verify the FFmpeg/Brush archive. COLMAP is installed from the separately pinned `ooosplat-colmap` Release. This restriction concerns source-development setup, not use of an installer that already contains the engines.

Homebrew supplies source-build prerequisites through `npm run setup:build-deps:macos`, followed by `npm run build:engines:macos`. macOS CI verifies that run's local archive before creating an unsigned app and DMG. Review and pin the archive and integrity-file hashes before enabling normal downloads of the manifest's archive.

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
dist-artifacts\OOOSplat-0.7.1-x64-windows.exe
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

## MCP (optional)

Enable it under Settings → MCP to let local AI agents such as Codex inspect ordinary/panorama inputs, collect generation and reshoot answers, start/resume tasks, and read status and logs. Agents can also export saved edits as standard PLY, compressed PLY, SPZ, or SOG without opening preview; files stay in the project's `exports/` directory, and SPZ range adjustments require user approval. The service is off by default, listens only on `127.0.0.1`, and needs no token.

See the [MCP documentation](docs/mcp-v1.md) for connection and configuration examples.

## FAQ

### Why is COLMAP using the CPU instead of the GPU?

On Windows and Ubuntu, OOOSplat enables COLMAP GPU acceleration when the pinned CUDA runtime is healthy and NVIDIA driver 580.00 or newer and Compute Capability 7.5 or higher are confirmed. Caspar additionally requires a successful tiny BA probe; failure selects Ceres CPU without disabling otherwise available SIFT GPU acceleration. Configuration pages show detected capabilities; the running task's details and logs show its actual backend. A temporary background detection timeout does not switch an active task to CPU. macOS COLMAP uses the CPU, while Brush selects its graphics backend independently.

### How does OOOSplat handle different GPU combinations?

COLMAP and Brush are separate stages. COLMAP requires a compatible NVIDIA CUDA environment for GPU acceleration. Brush uses Vulkan on Windows and Linux and Metal on Apple Silicon macOS. Common configurations are handled as follows; rows without an explicit platform refer to Windows:

| Platform and GPU configuration | COLMAP | Brush training |
|---|---|---|
| Windows: one NVIDIA discrete GPU | Uses CUDA when the driver and Compute Capability requirements are met | Explicitly selects the first discrete GPU and runs through Vulkan |
| Intel integrated GPU + one NVIDIA discrete GPU | Uses the compatible NVIDIA GPU through CUDA | Explicitly selects the first discrete GPU instead of the Intel integrated GPU |
| AMD integrated GPU + one NVIDIA discrete GPU | Uses the compatible NVIDIA GPU through CUDA | Explicitly selects the first discrete GPU and bypasses the AMD Switchable Graphics implicit layer for the Brush child process only |
| AMD only, or Intel integrated GPU + AMD discrete GPU | Falls back to CPU | Vulkan automatically selects an AMD GPU; the AMD graphics layer is not disabled |
| Intel integrated or discrete GPU only | Falls back to CPU | Vulkan automatically selects an Intel GPU |
| Multiple NVIDIA GPUs | Selects the compatible GPU with the highest Compute Capability, then the lower NVIDIA index on a tie | Does not force a Brush device index because Vulkan and CUDA indices cannot be mapped reliably; Vulkan selects automatically |
| AMD discrete GPU + NVIDIA discrete GPU | Uses a compatible NVIDIA GPU for COLMAP | Currently enters the single-NVIDIA preference path, but cross-vendor Vulkan ordering cannot be guaranteed; also set `brush_app.exe` to High performance in Windows Graphics settings and verify the actual adapter in the task log |
| Old NVIDIA driver, unsupported NVIDIA model, or failed detection | Falls back to CPU | Brush still attempts to select an available GPU through Vulkan but does not force NVIDIA |
| No usable GPU graphics backend | Uses CPU for COLMAP | Brush may not start; CPU-only software graphics has not been validated end to end |
| macOS: Apple Silicon M series, including Pro, Max, and Ultra | Currently uses the CPU; CUDA is not enabled | Automatically uses the M-series Metal GPU and unified memory |
| Ubuntu Alpha: compatible NVIDIA GPU | Uses CUDA when the runtime, driver, and Compute Capability checks pass, otherwise CPU | Vulkan selects automatically without Windows-specific adapter overrides |
| Ubuntu Alpha: AMD, Intel, or no detectable compatible NVIDIA GPU | Uses CPU | Vulkan selects an available GPU automatically |

Under the new policy, Detailed uses source training resolution rather than VRAM-based Low, Standard, or Large tiers. Ordinary and panorama budgets follow their respective rules in Quality Presets above. Being able to start training does not guarantee sufficient resources for every resolution or point count.

Forced adapter selection and the AMD implicit-layer workaround are passed only to the individual `brush_app.exe` child process. OOOSplat does not modify the system environment, driver configuration, or other applications, and it does not switch Brush to D3D12. AMD-only systems do not receive the AMD implicit-layer workaround.

If the Brush log reports `Device Lost`, OOOSplat distinguishes it from GPU OOM and does not apply an OOM retry. Connect the computer to power, close other GPU-heavy applications, and set `brush_app.exe` to High performance under Windows Settings → System → Display → Graphics. A single lower-point-cap retry requires explicit GPU OOM, Automatic retry enabled, and trustworthy counts, as described above.

M-series CPUs and GPUs share unified memory; the new policy does not downgrade Detailed to Low for that reason. Native-resolution training may still exceed available resources. Consult the validation records for macOS and Linux end-to-end hardware coverage of the new policy.

### Why does OOOSplat warn about a low registration rate?

A low registration rate usually means COLMAP could not find enough continuous, overlapping viewpoints. The task still continues to Brush, but result quality may be affected. Use an orbit video with stable exposure, clear frames, continuous movement, and sufficient viewpoint overlap. Avoid fast rotation, strong reflections, large plain-color areas, and moving subjects.

### Why does a project use so much disk space?

Each project keeps source copies, input images, COLMAP data, and Brush intermediates for diagnostics and resuming. Image sequences prefer internal hard links to reduce duplicate physical copies. After confirming the result, use Delete in Task List to move the complete project to the Recycle Bin.

### Can I view final.ply directly in OOOSplat?

Yes. Select “Preview” on a completed project in the task list to open its `.ply` in the dedicated preview workspace. “Adjust” supports whole-model Transform plus rectangle, sphere, and box Gaussian selection, with non-destructive deletion, cropping, inversion, undo, and redo. Edits are saved automatically in the project; “Save edits” confirms that the current edits and view have been saved without creating a model file. Choosing a model format directly from “Export” creates or safely replaces the edited model, while the original `final.ply` remains unchanged. Reopening restores the edits and the view saved on this device; yellow selections and undo history do not persist between sessions. “Animation” adds a 5-second reveal, an 8-second shockwave, a continuous orbit, watermarked portrait/landscape MP4 export, and offline HTML export. External standard/compressed PLY, SOG, SPZ, SPLAT, and KSPLAT files can be imported from “Completed → Import”; LCC/LCC2, Streamed SOG, 2DGS, and ordinary point-cloud PLY files are not supported.

`WASD` moves on the horizontal plane, by default at 15% of the current visible height per second. The bottom-right slider applies a 0.1×–5.0× multiplier; `Shift` gives a further 3× speed. Auto locate view returns from any view to the first training camera, using the earliest registered panorama's front face for panorama projects. It is disabled without a reliable training camera. Opening a preview still restores the saved view first. See the [navigation notes](docs/PREVIEW_NAVIGATION.md).

### Can I preview very large PLY files?

PLY files larger than 256 MiB load through Range requests of about 1 MB each to reduce full-file buffering during transfer. This does not compress, downsample, or discard SH data. Parsing, sorting, and GPU textures still need RAM and VRAM; whether a model opens depends on its size and the device, so not every large model is guaranteed to load.

### Which model formats can I export?

The preview Export menu supports models, portrait/landscape video, and single-file offline HTML. Model options:

| Format | Purpose and size |
| --- | --- |
| Standard PLY | Full precision, usually largest; archiving and continued editing |
| Compressed PLY | Smaller, lossy; requires a compatible viewer |
| SPZ v4 | Very small, lossy; sharing |
| Bundled SOG | Typically higher compression, lossy; web presentation |

SOG and SPZ are typically smaller; actual size varies and compression can affect detail. Processing starts only after selecting a format, without an intermediate PLY. The original `final.ply` stays unchanged, and failure or cancellation preserves existing exports. Compression is no longer blocked by an estimated-memory threshold, but large models can still fail if memory runs out. If SPZ coordinates exceed its range, explicit confirmation is required to center and, if needed, uniformly shrink only the export copy. Position, dimensions, and units may change; export still stops if safe encoding is impossible. See the [export notes and verification record](docs/model-export-verification.md).

Offline HTML preserves edits and supports viewing and animation playback, but not editing. Large models remain limited by device RAM and VRAM.

## Technology

- Desktop framework: Tauri 2
- Backend: Rust and Tokio
- Frontend: React 19, TypeScript, Vite, and Zustand
- Gaussian preview and animation: PlayCanvas Engine and PlayCanvas React
- Video encoding and muxing: WebCodecs and Mediabunny
- Native pipeline: FFmpeg / FFprobe, COLMAP, and Brush
- Process-tree management: Windows Job Object; Linux/macOS Unix process group

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
