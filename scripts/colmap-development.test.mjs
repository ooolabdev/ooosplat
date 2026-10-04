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
import { compileWindowsCudaProbe, verifyWindowsCudaBuildFlags, windowsCudaFlags } from "./windows-cuda-preflight.mjs";

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
      assert.deepEqual(invocation.args.slice(2, 5), ["--features", "local-colmap", "--config"]);
      assert.equal(JSON.parse(invocation.args[5]).build.beforeBuildCommand, "npm run build");
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
  assert.equal(resources[`${root}/colmap/`], "engines/macos/arm64/colmap/");
  assert.equal(resources[`${root}/bin/`], undefined);
  assert.equal(resources[`${root}/SHA256SUMS`], undefined);
  for (const executable of ["ffmpeg", "ffprobe", "brush_app"]) assert.ok(resources[`${root}/bin/${executable}`]);
  assert.ok(resources[`${root}/lib/`], "Other engines retain their existing libraries");
  assert.ok(resources["../.cache/html-viewer/runtime.js"]);
  assert.equal(local.bundle.macOS.signingIdentity, "-");
  assert.equal(JSON.stringify(base), original, "Formal config is not modified");
});

test("Windows and Linux local builds replace strict COLMAP entries with the complete developer directory", () => {
  for (const [platform, name] of [["win32", "windows"], ["linux", "linux"]]) {
    const base = mergePatch(JSON.parse(read("src-tauri/tauri.conf.json")), JSON.parse(read(`src-tauri/tauri.${name}.conf.json`)));
    const local = mergePatch(base, localConfiguration(platform));
    const root = platform === "win32" ? "../engines/colmap" : "../engines/linux/colmap";
    const destination = platform === "win32" ? "engines/colmap/" : "engines/linux/colmap/";
    assert.equal(local.bundle.resources[`${root}/`], destination);
    assert.equal(local.bundle.resources[`${root}/bin/`], undefined);
    assert.ok(Object.keys(local.bundle.resources).some(resource => !resource.startsWith(root)), "Other engine resources remain mapped");
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

test("ordinary application workflows consume the pinned runtime and never compile COLMAP", () => {
  for (const workflowName of ["windows", "ubuntu", "macos"]) {
    const workflow = read(`.github/workflows/${workflowName}.yml`);
    assert.match(workflow, /setup:engines|build:engines:macos/);
    assert.doesNotMatch(workflow, /build-colmap-(?:windows|linux|macos)|build:engines:(?:windows|linux)|build:colmap:macos/);
  }
  const mixedMacos = read("scripts/build-engines-macos.sh");
  assert.doesNotMatch(mixedMacos, /^build_macos_colmap$|colmap_archive=/m);
  assert.match(read("scripts/setup-engines-macos.sh"), /setup-colmap-runtime\.mjs" macos/);
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

test("macOS mixed runtime no longer compiles COLMAP while the manual COLMAP-only builder remains", () => {
  const mixed = read("scripts/build-engines-macos.sh"), only = read("scripts/build-colmap-macos.sh");
  assert.match(mixed, /source "\$workspace\/scripts\/colmap-macos-common\.sh"/);
  assert.match(mixed, /^bundle_macos_runtime$/m);
  assert.doesNotMatch(mixed, /^build_macos_colmap$|^collect_macos_colmap_notices$|colmap_archive=/m);
  for (const functionName of ["build_macos_colmap", "collect_macos_colmap_notices", "bundle_macos_runtime"]) assert.match(only, new RegExp(`^${functionName}$`, "m"));
  assert.match(read("scripts/setup-engines-macos.sh"), /setup-colmap-runtime\.mjs" macos/);
  assert.doesNotMatch(only, /brush_archive|ffmpeg_archive|engine_field (?:Brush|'FFmpeg)|setup-engines-macos/);
  assert.match(only, /package-colmap-macos-runtime\.mjs/);
  const common = read("scripts/colmap-macos-common.sh");
  assert.match(common, /-DCUDA_ENABLED=OFF/);
  assert.match(common, /-DCASPAR_ENABLED=OFF/);
  assert.match(common, /codesign --force --sign -/);
  assert.doesNotMatch(common, /download_verified.*(?:Brush|FFmpeg)/);
});

test("Windows CCCL compile preflight precedes dependency work and actual compile flags are verified", () => {
  const source = read("scripts/build-colmap-windows.ps1");
  assert.ok(source.includes(`$env:CUDAFLAGS = '${windowsCudaFlags.join(" ")}'`));
  const probe = source.indexOf("'windows-cuda-preflight.mjs'), 'probe'");
  assert.ok(probe > 0);
  for (const dependency of ["$archive =", "'clone', '--no-checkout'", "bootstrap-vcpkg.bat", "Invoke-Checked 'cmake' $options"]) {
    assert.ok(probe < source.indexOf(dependency), `CCCL probe must precede ${dependency}`);
  }
  assert.match(source, /-DCMAKE_EXPORT_COMPILE_COMMANDS:BOOL=ON/);
  const verify = source.indexOf("'windows-cuda-preflight.mjs'), 'verify-build'");
  assert.ok(verify > source.indexOf("Invoke-Checked 'cmake' $options"));
  assert.ok(verify < source.indexOf("Invoke-Checked 'cmake' @('--build'"));
  const fixture = read("scripts/fixtures/cuda/windows-cccl-smoke.cu");
  assert.match(fixture, /#include <cuda\/std\/type_traits>/);
  assert.match(fixture, /_MSVC_TRADITIONAL != 0/);
  assert.match(fixture, /cooperative_groups::reduce/);
  assert.doesNotMatch(fixture, /int main|<<<|cudaLaunch|CCCL_IGNORE_MSVC_TRADITIONAL_PREPROCESSOR_WARNING/);
});

test("Windows CCCL probe uses the conforming host flags and compiles without running a GPU", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-windows-cccl-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const compiler = path.join(root, "toolkit space", "bin", "nvcc.exe");
  const outputDirectory = path.join(root, "中文 compile probe");
  const flags = windowsCudaFlags.join(" ");
  const result = compileWindowsCudaProbe(compiler, outputDirectory, { flags, log: () => {}, execute: (command, args, options) => {
    assert.equal(command, compiler);
    for (const flag of windowsCudaFlags) assert.ok(args.includes(flag));
    assert.ok(args.includes("--compile"));
    assert.ok(args.includes("--gpu-architecture=compute_75"));
    assert.ok(args.some(arg => arg.endsWith("windows-cccl-smoke.cu")));
    assert.ok(args.at(-1).endsWith(".obj"));
    assert.equal(options.timeout, 120000);
    assert.ok(!args.includes("--run") && !args.includes("-run"));
    fs.writeFileSync(args.at(-1), "compiled object fixture");
    return { status: 0, stdout: "compiler completed", stderr: "" };
  } });
  assert.equal(result.flags, flags);
  assert.ok(fs.existsSync(result.object));
  for (const response of [
    { status: 1, stdout: "", stderr: "fatal error C1189: traditional preprocessor" },
    { status: null, error: new Error("timed out"), stdout: "", stderr: "" },
  ]) {
    assert.throws(() => compileWindowsCudaProbe(compiler, outputDirectory, { flags, execute: () => response, log: () => {} }), /preflight failed/);
  }
  assert.throws(() => compileWindowsCudaProbe(compiler, path.join(root, "no object"), { flags, execute: () => ({ status: 0 }), log: () => {} }), /without producing an object/);
  for (const invalidFlags of ["-allow-unsupported-compiler", `${flags} -Xcompiler=/Zc:preprocessor-`, `${flags} -DCCCL_IGNORE_MSVC_TRADITIONAL_PREPROCESSOR_WARNING`]) {
    assert.throws(() => compileWindowsCudaProbe(compiler, outputDirectory, { flags: invalidFlags, execute: () => { throw new Error("Must not execute compiler"); }, log: () => {} }), /missing required|disables or bypasses/);
  }
});

test("Windows CMake compilation inventory must actually preserve the host preprocessor flag", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-windows-cuda-flags-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const flags = windowsCudaFlags.join(" ");
  const base = [
    { file: "D:/source/Symforce-Caspar/generated/f32/caspar_mappings.cu", command: `"C:/CUDA toolkit/bin/nvcc.exe" ${flags} -c source.cu` },
    { file: "D:/source/Symforce-Caspar/generated/f32/kernel_example.cu", arguments: ["nvcc.exe", ...windowsCudaFlags, "-c", "source.cu"] },
    { file: "D:/source/SiftGPU/ProgramCU.cu", command: `nvcc.exe ${flags} -c source.cu` },
  ];
  const writeBuild = (commands, cudaFlags = flags, newline = "\n") => {
    fs.writeFileSync(path.join(root, "CMakeCache.txt"), `CMAKE_CUDA_FLAGS:STRING=${cudaFlags}${newline}`);
    fs.writeFileSync(path.join(root, "compile_commands.json"), JSON.stringify(commands));
  };
  for (const newline of ["\n", "\r\n"]) {
    writeBuild(base, flags, newline);
    assert.deepEqual(verifyWindowsCudaBuildFlags(root, { log: () => {} }), { flags, cudaCommands: 3, casparCommands: 2 });
  }
  for (const commands of [[], base.slice(2), [{ ...base[0], command: "nvcc.exe -allow-unsupported-compiler -c source.cu" }], [{ ...base[0], command: `nvcc.exe ${flags} -Xcompiler=/Zc:preprocessor- -c source.cu` }]]) {
    writeBuild(commands);
    assert.throws(() => verifyWindowsCudaBuildFlags(root, { log: () => {} }), /Missing actual|missing required|disables or bypasses/);
  }
  writeBuild(base, "-allow-unsupported-compiler");
  assert.throws(() => verifyWindowsCudaBuildFlags(root, { log: () => {} }), /CMake cached CUDA flags/);
});

test("Windows builder stops before dependency work when the actual preflight command fails", { skip: process.platform !== "win32" }, t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-windows-preflight-order-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const source = read("scripts/build-colmap-windows.ps1");
  const invokeChecked = source.slice(source.indexOf("function Invoke-Checked"), source.indexOf("if (-not $env:CUDA_PATH)"));
  const preflight = source.slice(source.indexOf("if (-not $env:CUDA_PATH)"), source.indexOf("$archive =")).replaceAll("$PSScriptRoot", "$mockScriptRoot");
  const script = `
    $ErrorActionPreference = 'Stop'
    $workspace = $env:MOCK_WORKSPACE
    $mockScriptRoot = Join-Path $workspace 'scripts'
    $lock = $env:MOCK_BUILD_LOCK | ConvertFrom-Json
    function Get-Command { 'mock prerequisite' }
    function cmake { "cmake version $($lock.cmakeVersion)" }
    function ninja { $lock.ninjaVersion }
    function node {
      $global:LASTEXITCODE = 0
      if ($args[0] -like '*windows-cuda-preflight.mjs') {
        Write-Output 'CCCL_PROBE_CALLED'
        $global:LASTEXITCODE = [int]$env:MOCK_PREFLIGHT_EXIT
      }
    }
    ${invokeChecked}
    ${preflight}
    Write-Output 'DEPENDENCY_WORK_STARTED'
  `;
  for (const exit of [0, 1]) {
    const result = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], {
      encoding: "utf8", windowsHide: true, env: { ...process.env, CUDA_PATH: path.join(root, "toolkit"), MOCK_WORKSPACE: root, MOCK_BUILD_LOCK: JSON.stringify(buildLock), MOCK_PREFLIGHT_EXIT: String(exit) },
    });
    assert.equal(result.status, exit, result.error?.message || result.stderr);
    assert.match(result.stdout, /CCCL_PROBE_CALLED/);
    if (exit) assert.doesNotMatch(result.stdout, /DEPENDENCY_WORK_STARTED/);
    else assert.match(result.stdout, /DEPENDENCY_WORK_STARTED/);
  }
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
