# Native recipe E2E verification — updated 2026-11-17

Branch: `test/playwright-recipe-e2e`, based on `main` at `64c98c2`. Local Windows/WebView2 only; no commit, push, installer, real generation engine, GPU training or online test upload.

## Automated checks

| Check | Result |
| --- | --- |
| Frontend Vitest | 88 files, 976 tests passed |
| Existing diagnostic protocol tests | 7 passed |
| Frontend production build | Passed; existing bundle-size/WASM/browser-external warnings remain |
| E2E TypeScript check | Passed |
| Ordinary Rust all-target regression | 1123 library + 1 CLI tests passed; 48 library + 1 CLI tests intentionally ignored |
| E2E adapter Rust tests | 7 passed: path/field isolation, positional dataset, selected device, fail-closed executable, valid tiny DB/model/PLY, full-path input identity and no invented source frames |
| E2E-feature shared saver tests | 28 passed, including actual open-reader replacement, real sharing/readonly refusal and 5000 consecutive saves |
| Clippy | Ordinary and `--features e2e`, all targets, `-D warnings` passed |
| rustfmt | Passed |
| Documentation links | Passed |

The sandbox initially denied three existing Windows symlink fixtures (OS1314). Re-running the complete Rust suite with the required local privileges passed; no test was changed or silently skipped to hide these permission failures.

## Windows state-save follow-up — 2026-11-17

The two 42/44 baseline failures share the state-save chain, not a recipe or geometry failure:

- `Alpha pixels true`: Brush completed; `final.ply` contained 64 valid Splats and its SHA-256 matched the **Committed** transaction. A subsequent view save failed.
- `reshoot appearance`: the ordinary source project's creation/preparation save failed **before any engine call or reshoot**.

An isolated comparison reproduced `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` failing with OS5 while an ordinary reader, including delete sharing, held the old target open. The old primitive succeeded 2000 times without a reader but failed on the first replacement with a reader. Rust 1.100's standard rename compatibility path passed 5000 replacements with 946079 concurrent reads, zero invalid records and zero read errors. True non-delete-sharing locks and read-only targets still failed and preserved the original file. This proves this contention mechanism, not the cause of every historical OS5 report.

The production JSON saver now uses that standard-library path on Windows, on the existing blocking executor. Serialization, unique same-directory staging, `write_all`, `flush`, `sync_all`, exact length verification and handle closure are retained. No generic OS5 sleep/retry, ACL/read-only alteration, target deletion or copy-overwrite fallback was introduced. Large-file publication and non-Windows replacement behavior are unchanged. I/O details retain the original OS error plus fixed save-operation/substep/target-role labels; private paths are not included.

New ordinary Rust regressions prove held readers retain complete old snapshots while new opens see complete new JSON; 5000 real public-saver calls with concurrent reads never expose missing/partial records. Actual sharing/read-only errors preserve old JSON. Creation failure/restart reuses the same project and owner intent. Failed views with an intact Commit reconcile through the real TaskService with normal readers held open, unchanged run history/training options/model bytes and a generation executor that would panic if invoked.

Alpha, all four reshoot-answer cases and Prepared/Committed recovery now set `sharedJsonReaders: true`: a test-only read-only handle is held across each **real** save. Polling frequency, assertions and `retries: 0` are unchanged. This observer does not manufacture a successful write, pause readers or retry failures.

Frontend tests/protocol tests/build and test TypeScript check passed. Ordinary/e2e all-target Clippy with `-D warnings`, rustfmt and documentation checks passed. Ordinary GUI/CLI binaries were rebuilt without the test feature; the tested E2E control/environment/error markers are absent, while the new JSON diagnostic marker is present.

The first restricted native attempt could not connect to WebView2's loopback CDP endpoint; it was stopped after two startup timeouts, not counted as a green run. Native verification requires the desktop/loopback permissions subsequently granted. No assertion or test was skipped to compensate.

### Three consecutive complete native runs

| Run identity | Result | Duration |
| --- | --- | --- |
| `20261117015210863-f1baf47d-2ce6-4de5-a5ba-f03eaace3e31` | 44 passed, 0 failed, 0 skipped | 4.6 minutes |
| `20261117015740950-43acc85b-4542-4666-9d90-573ab2a720f3` | 44 passed, 0 failed, 0 skipped | 4.1 minutes |
| `20261117020236848-75216382-9113-4ab5-b3b0-709f971ff0cf` | 44 passed, 0 failed, 0 skipped | 4.1 minutes |

All 132 scenario executions passed with one worker and zero retries, including both original failure contexts and Prepared/Commit recovery with deterministic shared readers. Each report and actual execution snapshot remains under its invocation's `.cache/e2e/artifacts/<run>/` directory. No assertions were weakened, sources reordered or polling delayed. These are local Windows results, not remote CI, real-engine or production-success-rate evidence.

## Historical native UI runs — 2026-11-16 baseline

44 scenarios are implemented, including the assertion negative control. Read the newest invocation's report using `npm run test:e2e:report`; each invocation is retained independently under `.cache/e2e/artifacts/<run>/`. The earlier fixed report locations `.cache/e2e/report` and `.cache/e2e/results` retain pre-migration evidence.

Complete 44-scenario invocation: **43 passed, 1 failed**, in about 5 minutes. Report identity: `20261116131720551-0c405bb9-b820-4929-a4a5-09a6fa80c873`. Failure was the ordinary base-generation portion of `reshoot geometry`, with real `replace_state` OS5, **before the reshoot started**. This is not a green-suite result.

Final complete re-run with correct native file/folder button routing and strict PTS/source-frame checks: **42 passed, 2 failed**, in about 6 minutes (`20261116133042118-5a143eeb-1744-461f-9402-b5d47741623b`). Both failures were real `replace_state` OS5, in `Alpha pixels true` and the base-generation portion of `reshoot appearance`. All 44 cases have passed in at least one run, but the suite is **not stably green**. This distinction must be retained.

Afterwards, a targeted run of crash classification, reshoot geometry, ordinary fast video and 29.97/VFR passed **4/4**. It does not invalidate the full-run failure. Saved-GPU preferences, partial reshoot adoption and initial-Mapper geometry/I/O also passed a separate **4/4** run.

The video/ERP/Bridge/29.97 targeted re-run passed **10/10**. After tightening the explicit command whitelist (including COLMAP help), a three-case run passed Alpha and fast video, while the ordinary base generation in `reshoot appearance` again failed with real `replace_state` OS5 (**2/3**). No assertion or retry was relaxed in response.

Dedicated test executables (`ooo-splat-e2e.exe`, `splatstudio-e2e.exe`) then passed real GUI/MCP/CLI startup and recipe checks **3/3**. Ordinary debug executables were rebuilt without the feature after the earlier prototype builds; neither contains the tested E2E control/error markers. Future test runs build only the dedicated test executables.

Final publication-count contrast check passed **4/4** (JPEG, Prepared recovery, Commit recovery, ordinary fast): simulated progress reports **128** Splats while the actual validated PLY contains **64**; persisted publication remains **64**. The final negative-control check also passed **1/1**, including missing and duplicate command arguments.

Representative targeted runs passed for MCP/CLI, all four reshoot answers, per-source Bridge, transparency, mixed ERP, saved GPU settings, partial-source adoption and initial-Mapper geometric failure. These are not substitutes for a complete green invocation.

## Historical failure evidence — before the state-save fix

Earlier complete 40-scenario runs ended at 39 passed / 1 failed. The failing scenario moved between ordinary preparation, publication and the base-generation portion of a reshoot. The last reproduced message retained:

```text
文件读写失败：拒绝访问。 (os error 5)
e2e_save_substep=replace_state
```

The annotation identified a real replacement failure, not a simulated engine error or recipe assertion; by itself it did not distinguish JSON from large-file replacements. At that point the exact failed file and cause were still unconfirmed. The follow-up above records the subsequently verified JSON contexts and reader-contention fix. The test framework still does not add retries, serialize away production writes or replace the saver with a fake one.

Failed run directories and local traces are retained. New green runs do not erase the original failures or establish that unrelated production I/O errors have all been resolved.

## Contract evidence

- Independent constants: ordinary tiers use reviewed caps 6192 / 12385 / 15481 for 30×96×64 images and 32 sparse points; ERP high uses 1280. Exact Brush option sets also reject unexpected learning-rate/noise overrides.
- Wrong iteration count, FPS, Gaussian initialization presence and duplicate Brush launch fail the same assertions used by positive tests; the mock does not rewrite the recipe.
- Real image preparation, database/feature/model validation, Gaussian inspection and publication remain active. Final output count is independently validated as 64, not taken from a progress estimate.
- Warm-to-Cold numerical fallback archives initialization outside the dataset; export OS112 and process crash do not consume algorithm fallback. A Warm initialization's SHA-256 is compared with the source Gaussian hash.
- Candidate and Commit recovery tests use durable real transactions and require zero additional engine calls after restart.
- Normal frontend production output contains no dialog adapter markers; Rust test commands/module are feature-gated with default features empty and a release-build prohibition.

## Not certified / not yet native-matrix coverage

- Manual GitHub Windows workflow has been created, not dispatched or remotely verified. No PR/release workflow or merge gate was added.
- No real COLMAP, Brush, FFmpeg, GPU compatibility, registration quality, rendering quality, PSNR, VRAM usage or online success-rate certification.
- Old-strategy permutations, GPU disappearance, queue/late-event race combinations, Bridge-mid-restart combinations and every historical snapshot are still principally covered by the existing Rust/Vitest suites, not exhaustively repeated through native windows.
- The native toolchain is Windows-only. No Linux/macOS native E2E result is claimed.
