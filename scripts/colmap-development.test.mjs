import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { localConfiguration, localInvocation } from "./local-tauri.mjs";
import { artifactSummary, placements } from "./colmap-artifact-summary.mjs";
import { dependencyPath, deploymentVersion, versionAtMost } from "./verify-macos-colmap-runtime.mjs";
import { collectComponentNotices, isLicenseNotice } from "./collect-macos-component-notices.mjs";
import { sha256, buildLock } from "./colmap-runtime.mjs";

const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
// Git's Windows checkout uses CRLF; source-policy assertions must be identical.
const read = file => fs.readFileSync(path.join(workspace, file), "utf8").replace(/\r\n/g, "\n");
function matrixSelector(workflow) {
  const match = workflow.replace(/\r\n/g, "\n").match(/node -e '\n([\s\S]+?)\n          '/);
  assert.ok(match, "Workflow must contain the embedded platform selector");
  return match[1].replace(/^ {12}/gm, "");
}
// Tauri merges inline configurations using JSON Merge Patch (RFC 7396).
function mergePatch(target, patch) {
  if (!patch || typeof patch !== "object" || Array.isArray(patch)) return patch;
  const result = { ...target };
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) delete result[key];
    else result[key] = mergePatch(result[key], value);
  }
  return result;
}

test("local Tauri invocation bypasses COLMAP checks and does not modify manifests", () => {
  for (const platform of ["win32", "linux", "darwin"]) {
    for (const action of ["dev", "build"]) {
      const invocation = localInvocation(action, platform, ["--help"]);
      assert.equal(invocation.command, process.execPath);
      assert.equal(invocation.args[1], action);
      assert.equal(invocation.args[2], "--config");
      assert.equal(JSON.parse(invocation.args[3]).build.beforeBuildCommand, "npm run build");
      assert.equal(invocation.args.at(-1), "--help");
      assert.doesNotMatch(JSON.stringify(invocation), /verify:engines|setup:engines|lock-engine|archiveSha256/);
    }
  }
  assert.throws(() => localInvocation("install", "win32"), /Usage/);
  assert.throws(() => localConfiguration("unsupported"), /Unsupported/);
  const source = read("scripts/local-tauri.mjs");
  assert.match(source, /prepareBrush\(hostPlatform\(\)\)/);
  assert.doesNotMatch(source, /setup-colmap|setup:engines|verify:engines|setup-engines/);
});

test("local macOS config replaces old COLMAP with the separate dependency directory", () => {
  const base = mergePatch(JSON.parse(read("src-tauri/tauri.conf.json")), JSON.parse(read("src-tauri/tauri.macos.conf.json")));
  const original = JSON.stringify(base);
  const local = mergePatch(base, localConfiguration("darwin"));
  const resources = local.bundle.resources;
  const root = "../engines/macos/arm64";
  assert.equal(resources[`${root}/colmap/bin/`], "engines/macos/arm64/colmap/bin/");
  assert.equal(resources[`${root}/colmap/lib/`], "engines/macos/arm64/colmap/lib/");
  assert.equal(resources[`${root}/bin/`], undefined);
  assert.equal(resources[`${root}/SHA256SUMS`], undefined);
  for (const executable of ["ffmpeg", "ffprobe", "brush_app"]) assert.ok(resources[`${root}/bin/${executable}`]);
  assert.ok(resources[`${root}/lib/`], "Other engines retain their existing libraries");
  assert.ok(resources["../.cache/html-viewer/runtime.js"]);
  assert.equal(local.bundle.macOS.signingIdentity, "-");
  assert.equal(JSON.stringify(base), original, "Formal config is not modified");
});

test("Windows and Linux local builds retain native resource maps and normal builds retain checks", () => {
  for (const [platform, name] of [["win32", "windows"], ["linux", "linux"]]) {
    const base = mergePatch(JSON.parse(read("src-tauri/tauri.conf.json")), JSON.parse(read(`src-tauri/tauri.${name}.conf.json`)));
    const local = mergePatch(base, localConfiguration(platform));
    assert.deepEqual(local.bundle.resources, base.bundle.resources);
    assert.match(base.build.beforeBuildCommand, /verify:engines|build:bundle/);
    assert.doesNotMatch(local.build.beforeBuildCommand, /verify|setup/);
  }
});

test("manual workflow only builds selected platforms and never publishes a Release", () => {
  const workflow = read(".github/workflows/colmap-engines.yml");
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /default: all/);
  assert.doesNotMatch(workflow, /^  (push|pull_request|schedule):/m);
  assert.doesNotMatch(workflow, /contents: write|gh release|build-engines-macos\.sh|npm run tauri/);
  assert.match(workflow, /fail-fast: false/);
  assert.match(workflow, /retention-days: 30/);
  assert.match(workflow, /short_sha.*github\.run_number.*github\.run_attempt/);
  assert.match(workflow, /build-colmap-macos\.sh/);
  // Exercise the actual embedded matrix selector, rather than a duplicate.
  for (const newline of ["\n", "\r\n"]) {
    const script = matrixSelector(workflow.replace(/\n/g, newline));
    for (const requested of ["all", "windows", "linux", "macos"]) {
      let output;
      const env = { REQUESTED_PLATFORM: requested, GITHUB_SHA: "a".repeat(40), GITHUB_OUTPUT: "mock" };
      const fakeFs = { appendFileSync: (_file, value) => { output = value; } };
      Function("require", "process", script)(() => fakeFs, { env });
      const matrix = JSON.parse(output.split("\n")[0].slice("matrix=".length));
      assert.equal(matrix.include.length, requested === "all" ? 3 : 1);
      if (requested !== "all") assert.equal(matrix.include[0].platform, requested);
      assert.match(output, /short_sha=aaaaaaaa/);
    }
  }
});

test("CUDA installation pins the patch release without network/apt drift", () => {
  const workflow = read(".github/workflows/colmap-engines.yml");
  assert.match(workflow, /cuda: '13\.2\.0'/);
  assert.match(workflow, /method: local/);
  assert.match(workflow, /linux-local-args: '\["--toolkit"\]'/);
  assert.doesNotMatch(workflow, /method: network/);
  const linux = read("scripts/build-colmap-linux.sh");
  assert.match(linux, /Missing build prerequisite/);
  assert.match(linux, /export PATH="\$cuda_root\/bin:\$PATH"/);
  assert.match(linux, /node "\$workspace\/scripts\/verify-cuda-toolkit\.mjs" linux "\$cuda_root"\n/);
  assert.match(linux, /node "\$workspace\/scripts\/verify-cuda-toolkit\.mjs" linux "\$cuda_root" "\$build"/);
  assert.doesNotMatch(linux, /require\(process\.argv\[1\]\)\.cuda\.version|check_version CUDA/);
  const windows = read("scripts/build-colmap-windows.ps1");
  assert.match(windows, /'verify-cuda-toolkit\.mjs'\), 'windows', \$env:CUDA_PATH\)/);
  assert.match(windows, /'verify-cuda-toolkit\.mjs'\), 'windows', \$env:CUDA_PATH, \$build\)/);
  assert.doesNotMatch(windows, /\$cudaInfo\.cuda\.version|V13\\\.2\\\.\\d\+/);
  for (const builder of [linux, windows]) {
    assert.match(builder, /-DCMAKE_CUDA_ARCHITECTURES:STRING=/);
    assert.match(builder, /-DCMAKE_CUDA_COMPILER:FILEPATH=/);
    assert.match(builder, /-DCUDAToolkit_ROOT:PATH=/);
  }
});

test("Linux preflight reports missing tools, checks build tools and delegates CUDA identity", {
  skip: process.platform !== "linux" && !process.env.OOOSPLAT_TEST_BASH,
}, t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-preflight-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const source = read("scripts/build-colmap-linux.sh");
  // Execute the builder's real preflight with deterministic tool responses.
  // CUDA field validation uses complete official fixtures in cuda-toolkit.test;
  // this test covers shell prerequisites and the actual shared checker call.
  const preflight = source.slice(source.indexOf("cuda_root="), source.indexOf('cache="$workspace'));
  const script = `set -Eeuo pipefail
    workspace="$MOCK_WORKSPACE"
    read_lock() {
      case "$1" in
        cmakeVersion) echo '${buildLock.cmakeVersion}' ;;
        ninjaVersion) echo '${buildLock.ninjaVersion}' ;;
        cudaVersion) echo '${buildLock.cudaVersion}' ;;
        *) exit 99 ;;
      esac
    }
    command() { [[ "$2" != "\${MOCK_MISSING_TOOL:-}" ]]; }
    cmake() { printf 'cmake version %s\\nMore version information\\n' "$MOCK_CMAKE_VERSION"; }
    ninja() { echo "$MOCK_NINJA_VERSION"; }
    node() {
      [[ "$1" == "$workspace/scripts/verify-cuda-toolkit.mjs" && "$2" == linux && "$3" == "$cuda_root" ]] || return 91
      echo CUDA_SHARED_CHECK_CALLED
    }
    ${preflight}
    echo PRECHECK_PASSED`;
  const run = ({ cmake = buildLock.cmakeVersion, ninja = buildLock.ninjaVersion, missing = "" } = {}) => spawnSync(process.env.OOOSPLAT_TEST_BASH || "bash", ["-c", script], {
    encoding: "utf8", windowsHide: true,
    env: { ...process.env, CUDA_PATH: root.replaceAll("\\", "/"), MOCK_WORKSPACE: workspace.replaceAll("\\", "/"), MOCK_CMAKE_VERSION: cmake, MOCK_NINJA_VERSION: ninja, MOCK_MISSING_TOOL: missing },
  });
  const good = run();
  assert.equal(good.status, 0, good.error?.message || good.stderr);
  assert.match(good.stdout, /CUDA_SHARED_CHECK_CALLED/);
  assert.match(good.stdout, /PRECHECK_PASSED/);
  for (const [name, options] of [["CMake", { cmake: "4.0.0" }], ["Ninja", { ninja: "1.0.0" }]]) {
    const drift = run(options);
    assert.equal(drift.status, 1);
    assert.match(drift.stderr, new RegExp(`${name} version differs from build lock`));
    assert.doesNotMatch(drift.stdout, /CUDA_SHARED_CHECK_CALLED|PRECHECK_PASSED/);
  }
  const missing = run({ missing: "patchelf" });
  assert.equal(missing.status, 1);
  assert.match(missing.stderr, /Missing build prerequisite: patchelf/);
});

test("macOS-only and mixed builders use the same compiler/relocation functions", () => {
  const mixed = read("scripts/build-engines-macos.sh"), only = read("scripts/build-colmap-macos.sh");
  for (const script of [mixed, only]) {
    assert.match(script, /source "\$workspace\/scripts\/colmap-macos-common\.sh"/);
    for (const functionName of ["build_macos_colmap", "collect_macos_colmap_notices", "bundle_macos_runtime"]) assert.match(script, new RegExp(`^${functionName}$`, "m"));
  }
  assert.doesNotMatch(only, /brush_archive|ffmpeg_archive|engine_field (?:Brush|'FFmpeg)|setup-engines-macos/);
  assert.match(only, /package-colmap-macos-runtime\.mjs/);
  const common = read("scripts/colmap-macos-common.sh");
  assert.match(common, /-DCUDA_ENABLED=OFF/);
  assert.match(common, /-DCASPAR_ENABLED=OFF/);
  assert.match(common, /codesign --force --sign -/);
  assert.doesNotMatch(common, /download_verified.*(?:Brush|FFmpeg)/);
});

test("artifact summaries show exact folders, optional hashes and GPU acceptance status", () => {
  const report = { archive: "runtime.zip", archiveSha256: "b".repeat(64), preTrimBytes: 1024, runtimeBytes: 512, compressedBytes: 256 };
  for (const platform of Object.keys(placements)) {
    const summary = artifactSummary(platform, report, "https://github.com/ooolabdev/ooosplat/actions/runs/1/artifacts/2");
    assert.ok(summary.includes(placements[platform].destination));
    assert.ok(summary.includes(placements[platform].binary));
    assert.match(summary, /not required locally/);
    assert.match(summary, /GPU execution validated: \*\*false\*\*/);
    assert.match(summary, /npm run dev:local/);
    assert.match(summary, /npm run build:local/);
  }
});

test("Mach-O deployment targets include legacy load commands and reject newer dependencies", () => {
  assert.equal(deploymentVersion("cmd LC_BUILD_VERSION\nminos 15.0\nsdk 15.5"), "15.0");
  assert.equal(deploymentVersion("cmd LC_VERSION_MIN_MACOSX\nversion 11.0\nsdk 15.0"), "11.0");
  assert.throws(() => deploymentVersion("sdk 15.5"), /Cannot read/);
  assert.ok(versionAtMost("15.0.0", "15.0"));
  assert.ok(versionAtMost("14.6", "15.0"));
  assert.equal(versionAtMost("15.1", "15.0"), false);
});

test("macOS dependency closure rejects host paths and paths outside the runtime", () => {
  const root = path.resolve("runtime"), target = path.join(root, "bin", "colmap");
  assert.equal(dependencyPath("/usr/lib/libSystem.B.dylib", target, root), null);
  assert.equal(dependencyPath("@rpath/libceres.dylib", target, root), path.join(root, "lib", "libceres.dylib"));
  assert.equal(dependencyPath("@executable_path/../lib/libceres.dylib", target, root), path.join(root, "lib", "libceres.dylib"));
  assert.equal(dependencyPath("@loader_path/libceres.dylib", path.join(root, "lib", "libglog.dylib"), root), path.join(root, "lib", "libceres.dylib"));
  assert.throws(() => dependencyPath("/opt/homebrew/lib/libceres.dylib", target, root), /Non-relocatable/);
  assert.throws(() => dependencyPath("@rpath/../../outside.dylib", target, root), /escapes/);
});

test("dependency notices retain installed copyright files without downloading", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-notices-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const prefix = path.join(root, "installed"), destination = path.join(root, "stage", "licenses");
  fs.mkdirSync(path.join(prefix, "share", "doc"), { recursive: true });
  fs.writeFileSync(path.join(prefix, "share", "doc", "COPYING"), "Copyright test dependency\nBSD test notice");
  fs.writeFileSync(path.join(prefix, "share", "doc", "example.txt"), "Not a runtime/license resource");
  collectComponentNotices({ name: "test-dependency", license: "BSD-3-Clause" }, prefix, destination, path.join(root, "cache"), () => { throw new Error("Must not download installed notices"); });
  assert.match(fs.readFileSync(path.join(destination, "share", "doc", "COPYING"), "utf8"), /Copyright test/);
  assert.equal(fs.existsSync(path.join(destination, "share", "doc", "example.txt")), false);
  assert.ok(fs.existsSync(path.join(destination, "FORMULA-INFO.json")));
  for (const name of ["LICENSE", "LICENSE_1_0.txt", "COPYING.LESSER", "Copyright", "NOTICE.md"]) assert.ok(isLicenseNotice(name));
  assert.equal(isLicenseNotice("licensed-plugin.dylib"), false);
});

test("unresolved dependency licensing blocks publication without removing the library", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-notices-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const prefix = path.join(root, "installed");
  fs.mkdirSync(prefix);
  const library = path.join(prefix, "libunknown.dylib");
  fs.writeFileSync(library, "preserve this runtime library");
  assert.throws(() => collectComponentNotices({ name: "unknown" }, prefix, path.join(root, "notices"), path.join(root, "cache")), /No installed notices or pinned source archive/);
  assert.ok(fs.existsSync(library));
});

test("missing bottle notices are read from the pinned source archive, not unverified metadata", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-notices-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const prefix = path.join(root, "installed"), cache = path.join(root, "cache"), destination = path.join(root, "notices");
  fs.mkdirSync(prefix);
  fs.mkdirSync(cache);
  const bytes = "mock source archive", checksum = sha256(bytes);
  fs.writeFileSync(path.join(cache, `example-${checksum}.archive`), bytes);
  const formula = { name: "example", license: "BSD-3-Clause", urls: { stable: { url: "https://example.com/source.tar.gz", checksum } } };
  const commands = [];
  collectComponentNotices(formula, prefix, destination, cache, (command, args) => {
    commands.push([command, ...args]);
    assert.equal(command, "tar", "A verified cached source does not download again");
    return { status: 0, stdout: args[0] === "-tf" ? "source/LICENSE\nsource/test.txt\n" : "Copyright upstream\nBSD license notice", stderr: "" };
  });
  assert.match(fs.readFileSync(path.join(destination, "0-LICENSE"), "utf8"), /Copyright upstream/);
  assert.equal(commands.length, 2);
});

test("a corrupt dependency source is rejected before its license contents are read", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-notices-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const prefix = path.join(root, "installed"), cache = path.join(root, "cache");
  fs.mkdirSync(prefix);
  const checksum = sha256("expected source"), formula = { name: "example", urls: { stable: { url: "https://example.com/source.tar.gz", checksum } } };
  assert.throws(() => collectComponentNotices(formula, prefix, path.join(root, "notices"), cache, (command, args) => {
    assert.equal(command, "curl", "tar must never read an unverified source");
    fs.writeFileSync(args.at(-1), "corrupt source");
    return { status: 0, stdout: "", stderr: "" };
  }), /license-source SHA-256 mismatch/);
});
