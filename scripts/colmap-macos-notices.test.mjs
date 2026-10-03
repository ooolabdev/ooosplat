import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const common = path.join(workspace, "scripts", "colmap-macos-common.sh");
const bash = process.env.OOOSPLAT_TEST_BASH || "bash";
const portable = file => file.replaceAll("\\", "/");
function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-macos-notices-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const stage = path.join(root, "stage"), build = path.join(root, "build"), brewRoot = path.join(root, "brew");
  const prefix = path.join(brewRoot, "Cellar", "ffmpeg", "8.0");
  // Windows checkout may use CRLF although the real macOS runner uses LF.
  // Run exactly the same shell functions with checkout-independent line ends.
  const normalizedCommon = path.join(root, "colmap-macos-common.sh");
  fs.writeFileSync(normalizedCommon, fs.readFileSync(common, "utf8").replace(/\r\n/g, "\n"));
  for (const directory of [path.join(stage, "lib"), path.join(stage, "licenses"), build, path.join(prefix, "lib")]) fs.mkdirSync(directory, { recursive: true });
  fs.writeFileSync(path.join(prefix, "COPYING.GPLv3"), "Installed upstream GPL FFmpeg license notice\n");
  fs.writeFileSync(path.join(prefix, "lib", "libavcodec.63.dylib"), "Required runtime library");
  fs.copyFileSync(path.join(prefix, "lib", "libavcodec.63.dylib"), path.join(stage, "lib", "libavcodec.63.dylib"));
  return { root, stage, build, brewRoot, prefix, normalizedCommon, origins: path.join(build, "origins.tsv") };
}
function run(f, script) {
  return spawnSync(bash, ["-c", `set -euo pipefail
    source "$COMMON_SCRIPT"
    workspace="$WORKSPACE_DIR"
    stage="$STAGE_DIR"
    build="$BUILD_DIR"
    brew_root="$BREW_ROOT"
    dependency_origins="$ORIGINS_FILE"
    node() { "$NODE_EXECUTABLE" "$@"; }
    brew() {
      case "$1" in
        info) printf '%s' "$FORMULA_INFO" ;;
        --prefix) printf '%s' "$FORMULA_PREFIX" ;;
        *) echo 'Unexpected brew invocation' >&2; return 1 ;;
      esac
    }
    ${script}`], {
    encoding: "utf8", windowsHide: true,
    env: { ...process.env, COMMON_SCRIPT: portable(f.normalizedCommon), WORKSPACE_DIR: portable(workspace),
      STAGE_DIR: portable(f.stage), BUILD_DIR: portable(f.build), BREW_ROOT: portable(f.brewRoot),
      ORIGINS_FILE: portable(f.origins), FORMULA_PREFIX: portable(f.prefix), NODE_EXECUTABLE: portable(process.execPath),
      FORMULA_INFO: JSON.stringify({ formulae: [{ name: "ffmpeg", license: "GPL-3.0-or-later", homepage: "https://ffmpeg.org/", versions: { stable: "8.0" } }] }),
    },
  });
}
function successful(result) {
  assert.equal(result.status, 0, result.error?.message || result.stderr);
}

test("COLMAP-only transitive Homebrew FFmpeg retains its real formula notices, not the mixed-build LGPL notice", t => {
  const f = fixture(t);
  fs.writeFileSync(f.origins, `libavcodec.63.dylib\t${portable(path.join(f.prefix, "lib", "libavcodec.63.dylib"))}\n`);
  successful(run(f, "collect_macos_runtime_component_notices"));
  const inventory = JSON.parse(fs.readFileSync(path.join(f.stage, "BUNDLED-COMPONENTS.json"), "utf8"));
  assert.equal(inventory.components.length, 1);
  const component = inventory.components[0];
  assert.equal(component.source, "homebrew");
  assert.equal(component.license, "GPL-3.0-or-later");
  assert.deepEqual(component.files, ["lib/libavcodec.63.dylib"]);
  assert.ok(component.licenseFiles.includes("homebrew/ffmpeg/COPYING.GPLv3"));
  assert.ok(component.licenseFiles.includes("homebrew/ffmpeg/FORMULA-INFO.json"));
  assert.ok(component.licenseFiles.every(file => fs.existsSync(path.join(f.stage, "licenses", file))));
  assert.ok(!component.licenseFiles.includes("FFmpeg-LGPL-2.1.txt"));
  assert.equal(fs.existsSync(path.join(f.stage, "licenses", "FFmpeg-LGPL-2.1.txt")), false);
  const formula = JSON.parse(fs.readFileSync(path.join(f.stage, "licenses", "homebrew", "ffmpeg", "FORMULA-INFO.json"), "utf8"));
  assert.equal(formula.versions.stable, "8.0");
});

test("mixed build explicitly registered FFmpeg and Homebrew FFmpeg keep separate provenance and notices", t => {
  const f = fixture(t);
  fs.writeFileSync(path.join(f.stage, "licenses", "FFmpeg-LGPL-2.1.txt"), "Mixed build LGPL notice\n");
  fs.writeFileSync(path.join(f.stage, "lib", "libavcodec.62.dylib"), "Mixed build library");
  fs.writeFileSync(f.origins, [
    `libavcodec.62.dylib\t${portable(path.join(f.stage, "lib", "libavcodec.62.dylib"))}\tbuilt-ffmpeg`,
    `libavcodec.63.dylib\t${portable(path.join(f.prefix, "lib", "libavcodec.63.dylib"))}`,
  ].join("\n") + "\n");
  successful(run(f, "collect_macos_runtime_component_notices"));
  const inventory = JSON.parse(fs.readFileSync(path.join(f.stage, "BUNDLED-COMPONENTS.json"), "utf8"));
  assert.equal(inventory.components.length, 2);
  const built = inventory.components.find(component => component.source === "built-ffmpeg");
  const bottled = inventory.components.find(component => component.source === "homebrew");
  assert.equal(built.name, "ffmpeg");
  assert.deepEqual(built.licenseFiles, ["FFmpeg-LGPL-2.1.txt"]);
  assert.deepEqual(built.files, ["lib/libavcodec.62.dylib"]);
  assert.equal(built.license, "LGPL-2.1-or-later");
  assert.equal(bottled.name, "ffmpeg");
  assert.ok(bottled.licenseFiles.every(file => file.startsWith("homebrew/ffmpeg/")));
  assert.deepEqual(bottled.files, ["lib/libavcodec.63.dylib"]);
});

test("an unregistered staged COLMAP dependency cannot be mislabeled as mixed FFmpeg", t => {
  const f = fixture(t);
  const library = path.join(f.stage, "lib", "libavcodec.63.dylib");
  fs.writeFileSync(f.origins, `libavcodec.63.dylib\t${portable(library)}\n`);
  const result = run(f, "collect_macos_runtime_component_notices");
  assert.equal(result.status, 1, result.error?.message);
  assert.match(result.stderr, /Unregistered staged dependency origin/);
  assert.ok(fs.existsSync(library), "Unknown required dependency must not be removed");
  assert.equal(fs.existsSync(path.join(f.stage, "BUNDLED-COMPONENTS.json")), false);
});

test("explicit mixed FFmpeg provenance still requires the real mixed notice", t => {
  const f = fixture(t);
  fs.writeFileSync(f.origins, `libavcodec.63.dylib\t${portable(path.join(f.stage, "lib", "libavcodec.63.dylib"))}\tbuilt-ffmpeg\n`);
  const result = run(f, "collect_macos_runtime_component_notices");
  assert.equal(result.status, 1, result.error?.message);
  assert.match(result.stderr, /Missing notice for explicitly registered mixed-build FFmpeg/);
});

test("Homebrew opt origins remain Homebrew even if given an unrelated built marker", t => {
  const f = fixture(t);
  successful(run(f, `classify_macos_component_origin "$brew_root/opt/ffmpeg/lib/libavcodec.63.dylib" built-ffmpeg
    [[ "$component" == ffmpeg && "$component_source" == homebrew ]]`));
});

test("repeated staged rpath dependencies retain existing Homebrew provenance", () => {
  const text = fs.readFileSync(common, "utf8").replace(/\r\n/g, "\n");
  assert.match(text, /existing_origin="\$\(awk -F '\\t'/);
  assert.match(text, /elif \[\[ "\$source_dependency" != "\$destination_dependency" \]\]/);
  assert.doesNotMatch(text, /printf[^\n]*built-ffmpeg/);
  const mixed = fs.readFileSync(path.join(workspace, "scripts", "build-engines-macos.sh"), "utf8");
  assert.match(mixed, /printf '%s\\t%s\\tbuilt-ffmpeg\\n'/);
});
