# Native engine runtime

Native binaries are not stored in Git. Exact upstream URLs, archive hashes,
installation rules, reported versions, and executable hashes are pinned in
`manifest.json` (Windows), `manifest.linux.json` (Ubuntu 24.04 Alpha,
x86_64 only), and `manifest.macos.json` (macOS 15+ Apple Silicon Alpha).

Restore the pinned release inputs before normal release packaging. For manually
placed development COLMAP packages, use the local commands described below:

```text
npm run setup:engines
npm run verify:engines
```

Downloaded archives are cached under `.cache/engines/`, and extracted runtimes
are placed in this directory. Both are ignored by Git. The finished NSIS
installer still embeds the complete verified runtimes, so Windows end users do
not need to download or configure engines. For Ubuntu 24.04 Alpha, setup restores pinned COLMAP under
`engines/linux/colmap` and Brush under `engines/linux/brush`. The `.deb`
embeds both; only FFmpeg/FFprobe remain system dependencies. COLMAP is never
resolved from apt or PATH.
Other Linux distributions remain outside the current delivery scope.

The macOS Alpha restores a complete self-contained runtime under
`engines/macos/arm64/`. Its FFmpeg/FFprobe and CPU CLI-only COLMAP builds are
produced from pinned upstream sources; Brush uses its pinned OOOBrush arm64
archive. The packaged app never writes into its resources and never falls back
to Homebrew or system `PATH`. Maintainers install the pinned build formulae
with `npm run setup:build-deps:macos` and rebuild the release archive with
`npm run build:engines:macos` on an Apple Silicon Mac.

## COLMAP 4.2.1 source/build lock

All platforms use commit `bd1fcf654d2dd8fefa1466999c190a246f83f4b9`, with a
verified source archive SHA-256 in `colmap-build.json`. Windows/Linux use CUDA
13.2.0, Caspar f32 and Ceres; Apple Silicon uses Ceres CPU. SIFT acceleration
and BA backend selection are independent. Caspar is selected only after a
bounded execution probe; otherwise mapper retains the original Ceres behavior.

The official GUI, OpenGL, MVS, ONNX, CGAL, LSD, download and test features are
disabled. Runtime DLL/shared-library dependencies and image plugins are kept
conservatively, including unresolved-use components; unclassified licenses
block packaging rather than causing a library to be deleted. Headers, static
development libraries, test executables, debug symbols and vocabulary/model
resources are not shipped. NVIDIA runtime files are kept unmodified.

## Download COLMAP-only development builds

The manual workflow `.github/workflows/colmap-engines.yml` builds only COLMAP
and necessary runtime dependencies. It does not build/download Brush or FFmpeg,
package OOOSplat, publish Releases, or modify engine manifests.

1. Once the workflow exists on the default branch, open **Actions → COLMAP-only
   development runtime → Run workflow**. Choose the source branch and select
   `all`, `windows`, `linux`, or `macos`. GitHub requires manual workflows to
   exist on the default branch before they can be dispatched:
   [GitHub manual workflow documentation](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow).
2. Download the matching successful job's Artifact (retained for 30 days).
   Extract its outer ZIP, then extract the inner `.zip` or `.tar.xz` runtime
   archive using a tool that preserves Unix executable permissions.
3. Copy the **contents** of the archive's top-level directory into the folder
   below. Keep `bin`, libraries, licenses and metadata together; do not add
   another wrapper directory. Remove obsolete files from a previously installed
   COLMAP package when replacing it, without touching Brush or FFmpeg.

| Platform | Destination in the checkout | Expected executable |
| --- | --- | --- |
| Windows x64 | `engines/colmap/` | `engines/colmap/bin/colmap.exe` |
| Ubuntu 24.04 x64 | `engines/linux/colmap/` | `engines/linux/colmap/bin/colmap` |
| macOS 15+ arm64 | `engines/macos/arm64/colmap/` | `engines/macos/arm64/colmap/bin/colmap` |

The inner directories are named `ooosplat-colmap-windows-x64`,
`ooosplat-colmap-linux-x64`, and `ooosplat-colmap-macos-arm64`, respectively.
macOS COLMAP is kept separate from the mixed FFmpeg/Brush libraries. Existing
mixed-runtime COLMAP remains the fallback when no standalone executable exists.

With your existing FFmpeg available, run:

```text
npm run dev:local
npm run build:local
```

These commands automatically prepare the pinned OOOBrush CLI, with verified
download and offline cache reuse. They do not install/download COLMAP, verify
its package hashes or version manifests, or edit formal manifests. FFmpeg is
unchanged. The application's runtime health
checks and GPU/BA probes still run normally. Missing resources fail directly
during compilation/packaging or startup. `build:local` creates the normal host
platform test bundles; extra Tauri arguments can follow `--`, for example
`npm run build:local -- --no-bundle`. macOS local bundles use ad-hoc signing.
In Windows PowerShell, use `npm.cmd` when forwarding extra arguments, for example
`npm.cmd run build:local -- --no-bundle`.
Normal `npm run tauri -- build` and release CI retain strict verification.

## OOOBrush ooo-v1.0.0

All three platforms download the precompiled headless `brush-cli` from
[OOOBrush ooo-v1.0.0](https://github.com/ooolabdev/OOOBrush/releases/tag/ooo-v1.0.0),
commit `866a7d65b4d6a591ba0d52a88f8b20ea3465c1e5`. The release's viewer
`brush` is not installed or bundled. For path compatibility the CLI is renamed:

| Platform | Installed CLI | Release metadata |
| --- | --- | --- |
| Windows x64 | `engines/brush/brush_app.exe` | `engines/brush/` |
| Linux x64 | `engines/linux/brush/brush_app` | `engines/linux/brush/` |
| macOS arm64 | `engines/macos/arm64/bin/brush_app` | `engines/macos/arm64/licenses/OOOBrush/` |

`npm run setup:brush` prepares Brush independently of COLMAP/FFmpeg;
`npm run verify:brush` checks the raw installed CLI. Formal macOS bundles use
the existing combined integrity inventory after relocation/signing instead.
Installation verifies the locked archive SHA-256, every internal checksum,
source commit/target, binary hash, license, actual version/help and runtime
dependencies before replacement. An invalid cached archive is rejected; remove
only the affected archive from `.cache/engines/ooobrush/` before retrying.
Do not delete a live `*.install.lock`; an interrupted setup may leave one that
must be removed only after confirming no Brush setup process is running.

The upstream README/checksums are retained with an `UPSTREAM-` prefix; those
checksums describe the original archive, not the reduced runtime. `INSTALL-INFO.json`
records the CLI alias and source provenance. Windows/Linux runtime resources
include these files; macOS stores them under its shared licenses directory.
Only managed Brush files are replaced, with rollback on an installation error;
macOS shared bin/lib directories and manually placed COLMAP are left intact.

Training uses `--total-train-iters`; existing presets and final PLY publication
remain unchanged. CLI/process INFO diagnostics are enabled alongside GPU logs,
and `RUST_BACKTRACE=1` is set only when absent. Diagnostic iterations do not
change the UI's existing estimated progress or establish successful export.
Version/help checks do not validate GPU training. Run `npm run test:brush:gpu`
on supported hardware for an opt-in four-iteration training test; reports and
logs are retained in `.cache/brush-acceptance/`. It uses isolated tiny images
and a COLMAP text model, with no user dataset writes or engine replacement.
Successful export is counted only with actual completed iterations, a nonempty
binary PLY, and runtime diagnostics confirming a physical GPU adapter.

Validation recorded on 2026-10-03 for the exact pinned Release:

| Platform | Real archive/internal checksums | Native CLI | Physical GPU short training |
| --- | --- | --- | --- |
| Windows x64 | Passed | Passed | Passed: RTX 3060 Ti, driver 616.92, Vulkan; 4 iterations, 64 exported splats |
| Linux x64 | Passed (static verification on Windows) | Not verified | Not verified |
| macOS arm64 | Passed (static verification on Windows) | Not verified | Not verified |

This is not full application reconstruction/high-quality/reshoot acceptance.
The fork emits an args-config merge warning for export paths containing spaces;
its fallback retains the explicit CLI config. The Windows test confirmed the
requested four iterations and successful Unicode/spaced-path export; OOOSplat
does not rely on an upstream args.txt merge for its presets.
Local Windows build reached the resource-copy stage after frontend compilation,
then stopped because the existing manually placed COLMAP lacks
`BUNDLED-COMPONENTS.json` (and other new development-package metadata).
Place the complete new COLMAP development archive before testing application
packaging; no placeholder metadata or relaxed formal verification is used.

Formal COLMAP/mixed-runtime archive hashes are still pending their first reviewed
build; preparing Brush does not bypass those normal release checks. No workflow
triggers or Release publication mechanisms are changed by Brush preparation.

Each Artifact contains the runtime archive, optional reference `.sha256`, and
`.build-report.json`. Job Summary links the download and shows exact placement,
source/build identity and sizes. Local users do **not** need to run checksum or
installation commands. Actual NVIDIA Caspar execution remains unvalidated until
separate real-hardware acceptance; macOS is CPU-only.

To stop using a development runtime, remove only the manually placed COLMAP
directory (keep its tracked README if present), then restore normal pinned
release inputs when available. No registration or uninstall command is needed.

## Formal release promotion (separate from local development)

The new archive/integrity digests deliberately remain `null` until a real
build is reviewed. Setup and packaging fail closed while these pins are
missing; the old official Windows zip and apt COLMAP are not fallbacks. The
explicit local commands above do not require these pins.

1. Build with `.github/workflows/colmap-engines.yml` (manual dispatch), or
   `npm run build:engines:windows` / `npm run build:engines:linux` on a matching
   host with the locked compiler tools. macOS uses the existing engine workflow
   and `npm run build:engines:macos`. The standalone macOS development archive
   is not the mixed archive consumed by the formal macOS installer/lock tool.
2. Review the archive, `BUILD-INFO.json`, `BUNDLED-COMPONENTS.json`, licenses,
   `SHA256SUMS`, and `*.build-report.json`. The report includes compressed and
   installed bytes; Windows/Linux also include staging bytes before artifact
   trimming. It does not claim an unmeasured old-release size reduction.
3. Run `node scripts/lock-engine-archive.mjs windows|linux|macos ARCHIVE` for
   each reviewed artifact, then review/commit the manifest-only digest changes.
   The tool re-extracts the archive and checks its source identity/inventory;
   downloaded checksum sidecars never establish the installer's trust pin.
4. Publish the identical artifacts under `engines-windows-x64-v0.1.0`,
   `engines-linux-x64-v0.1.0`, and `engines-macos-arm64-v0.3.0`. Versioned asset
   publication must not overwrite a previous release. The COLMAP-only workflow
   does not publish these assets. Ubuntu app CI consumes the pinned archive;
   the existing macOS app CI still builds its mixed engine runtime inline.
5. Run clean-host and real GPU acceptance before distributing the app. Standard
   CI validates CPU execution/CLI/package inputs, not GPU local/global BA.

Each builder gates its archive on JPEG/RGBA-PNG decoding, alpha masks and CPU
SIFT extraction from a Unicode working directory with developer library paths
removed. This is a codec/runtime smoke check, not reconstruction/GPU acceptance.
Before the first runtime archives exist, Rust unit-only checks can use a temporary
`TAURI_CONFIG={"bundle":{"resources":[]}}` environment override; never use this
override for app packaging or runtime verification.

### Acceptance matrix

On Windows/Linux, verify actual Caspar local/global mapper BA with compatible
NVIDIA hardware, and Ceres fallback with no GPU, incompatible driver and failed
probe. On macOS verify CPU execution. All platforms must cover ordinary and
high-quality reconstruction, bridging, incremental reshoots with fixed old
poses/intrinsics, JPEG/PNG, alpha masks, image lists, Unicode paths and existing
models. The managed runtime must work after relocation without system COLMAP
or developer libraries. Restore any trim whose effect cannot be established.
