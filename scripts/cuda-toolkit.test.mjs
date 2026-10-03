import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { buildLock } from "./colmap-runtime.mjs";
import { validateCudaMetadata, verifyCudaToolkit, verifyCudaCmake } from "./verify-cuda-toolkit.mjs";

const fixtures = path.join(path.dirname(fileURLToPath(import.meta.url)), "fixtures", "cuda");
const quiet = () => {};
const hostPlatform = process.platform === "win32" ? "windows" : "linux";
const fixtureText = (platform, version = "13.2.0") => fs.readFileSync(path.join(fixtures, `${platform}-${version}.json`), "utf8").replace(/\r\n/g, "\n");
const metadata = (platform, version) => JSON.parse(fixtureText(platform, version));
const nvccOutput = version => `nvcc: NVIDIA (R) Cuda compiler driver\nCuda compilation tools, release 13.2, V${version}\n`;

function toolkit(t, platform = hostPlatform, newline = "\n") {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-cuda-test-"));
  t.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const root = path.join(temporary, "中文 toolkit");
  const compiler = path.join(root, "bin", platform === "windows" ? "nvcc.exe" : "nvcc");
  fs.mkdirSync(path.dirname(compiler), { recursive: true });
  fs.writeFileSync(compiler, "mock compiler; tests inject execution");
  fs.writeFileSync(path.join(root, "version.json"), fixtureText(platform).replace(/\n/g, newline));
  const execute = (command, args) => {
    assert.equal(command, compiler, "Never resolve an unrelated nvcc through PATH");
    assert.deepEqual(args, ["--version"]);
    return { status: 0, stdout: nvccOutput("13.2.51"), stderr: "" };
  };
  return { root, compiler, execute };
}

function cmakeBuild(root, compiler, newline = "\n") {
  const build = path.join(root, "build");
  const compilerFile = path.join(build, "CMakeFiles", buildLock.cmakeVersion, "CMakeCUDACompiler.cmake");
  fs.mkdirSync(path.dirname(compilerFile), { recursive: true });
  const cmakePath = value => value.split(path.sep).join("/");
  const cache = [
    `CMAKE_CUDA_COMPILER:FILEPATH=${cmakePath(compiler)}`,
    `CMAKE_CUDA_ARCHITECTURES:STRING=${buildLock.cudaArchitectures.join(";")}`,
    `CUDAToolkit_ROOT:PATH=${cmakePath(root)}`,
    `CUDAToolkit_NVCC_EXECUTABLE:FILEPATH=${cmakePath(compiler)}`,
  ].join(newline) + newline;
  const compilerText = [
    `set(CMAKE_CUDA_COMPILER "${cmakePath(compiler)}")`,
    'set(CMAKE_CUDA_COMPILER_VERSION "13.2.51")',
    'set(CMAKE_CUDA_COMPILER_TOOLKIT_VERSION "13.2.51")',
    `set(CMAKE_CUDA_COMPILER_TOOLKIT_ROOT "${cmakePath(root)}")`,
    `set(CMAKE_CUDA_COMPILER_LIBRARY_ROOT "${cmakePath(root)}")`,
  ].join(newline) + newline;
  fs.writeFileSync(path.join(build, "CMakeCache.txt"), cache);
  fs.writeFileSync(compilerFile, compilerText);
  return { build, compilerFile, cache, compilerText };
}

test("official platform metadata identifies the same CUDA 13.2.0 components despite different SDK labels", () => {
  for (const platform of ["windows", "linux"]) {
    const actual = validateCudaMetadata(platform, metadata(platform), { log: quiet });
    assert.equal(actual.releaseVersion, "13.2.0");
    assert.equal(actual.metadataVersion, platform === "linux" ? "13.2.20260303" : "13.2.0");
    assert.deepEqual(actual.components, {
      cuda_nvcc: "13.2.51", cuda_cudart: "13.2.51", cuda_crt: "13.2.51", libcurand: "10.4.2.51",
    });
  }
});

test("official 13.2.2 metadata and a mislabeled newer compiler are rejected", () => {
  for (const platform of ["windows", "linux"]) {
    assert.throws(() => validateCudaMetadata(platform, metadata(platform, "13.2.2"), { log: quiet }));
    const mixed = metadata(platform);
    mixed.cuda_nvcc.version = "13.2.86";
    assert.throws(() => validateCudaMetadata(platform, mixed, { log: quiet }), /cuda_nvcc/);
  }
});

test("missing CUDA identity fields and mixed component builds fail closed", () => {
  for (const platform of ["windows", "linux"]) {
    for (const name of ["cuda", "cuda_nvcc", "cuda_cudart", "cuda_crt", "libcurand"]) {
      const absent = metadata(platform);
      delete absent[name];
      assert.throws(() => validateCudaMetadata(platform, absent, { log: quiet }));
      const missingVersion = metadata(platform);
      delete missingVersion[name].version;
      assert.throws(() => validateCudaMetadata(platform, missingVersion, { log: quiet }));
    }
    for (const name of ["cuda_nvcc", "cuda_cudart", "cuda_crt", "libcurand"]) {
      const mixed = metadata(platform);
      mixed[name].version = metadata(platform, "13.2.2")[name].version;
      assert.throws(() => validateCudaMetadata(platform, mixed, { log: quiet }), new RegExp(name));
    }
  }
  assert.throws(() => validateCudaMetadata("macos", metadata("linux"), { log: quiet }));
  assert.throws(() => validateCudaMetadata("linux", metadata("windows"), { log: quiet }));
});

test("the exact metadata label remains locked, not just the compiler major/minor", () => {
  const changed = metadata("linux");
  changed.cuda.version = "13.2.20260304";
  assert.throws(() => validateCudaMetadata("linux", changed, { log: quiet }));
  const customLock = structuredClone(buildLock);
  customLock.cudaToolkitIdentity.components.cuda_nvcc = "13.2.86";
  assert.throws(() => validateCudaMetadata("linux", metadata("linux"), { lock: customLock, log: quiet }), /cuda_nvcc/);
});

test("LF and CRLF metadata use the selected absolute nvcc in Chinese and spaced paths", t => {
  for (const platform of ["windows", "linux"]) {
    for (const newline of ["\n", "\r\n"]) {
      const f = toolkit(t, platform, newline);
      const actual = verifyCudaToolkit(platform, f.root, { execute: f.execute, log: quiet });
      assert.equal(actual.releaseVersion, "13.2.0");
      assert.equal(actual.compiler.version, "13.2.51");
      assert.equal(actual.compiler.path, f.compiler);
    }
  }
});

test("failed nvcc execution and a compiler different from metadata are rejected", t => {
  const f = toolkit(t);
  for (const response of [
    { status: 1, stdout: nvccOutput("13.2.51"), stderr: "compiler failed" },
    { status: null, error: new Error("timeout"), stdout: nvccOutput("13.2.51"), stderr: "" },
    { status: 0, stdout: nvccOutput("13.2.86"), stderr: "" },
    { status: 0, stdout: "release 13.2 without a full compiler version", stderr: "" },
  ]) {
    assert.throws(() => verifyCudaToolkit(hostPlatform, f.root, { execute: () => response, log: quiet }));
  }
});

test("a wrong toolkit root or a missing compiler cannot reuse PATH metadata", t => {
  const f = toolkit(t);
  const wrongRoot = path.join(f.root, "unselected toolkit");
  fs.mkdirSync(wrongRoot);
  assert.throws(() => verifyCudaToolkit(hostPlatform, wrongRoot, { execute: f.execute, log: quiet }));
  fs.unlinkSync(f.compiler);
  assert.throws(() => verifyCudaToolkit(hostPlatform, f.root, { execute: f.execute, log: quiet }));
});

test("CMake records the selected compiler, full version and locked STRING architectures with either newline", t => {
  for (const newline of ["\n", "\r\n"]) {
    const f = toolkit(t), b = cmakeBuild(f.root, f.compiler, newline);
    const actual = verifyCudaCmake(b.build, f.root, { log: quiet });
    assert.deepEqual(actual.architectures, buildLock.cudaArchitectures);
    assert.equal(actual.compiler.version, "13.2.51");
    assert.equal(actual.compiler.path, f.compiler);
  }
});

test("CMake UNINITIALIZED architectures, a reduced set or missing compiler metadata fail", t => {
  for (const mutation of [
    cache => cache.replace("CMAKE_CUDA_ARCHITECTURES:STRING=", "CMAKE_CUDA_ARCHITECTURES:UNINITIALIZED="),
    cache => cache.replace(buildLock.cudaArchitectures.join(";"), "75;80;86"),
    cache => cache.replace(/^CMAKE_CUDA_COMPILER:FILEPATH=.*\n/m, ""),
  ]) {
    const f = toolkit(t), b = cmakeBuild(f.root, f.compiler);
    fs.writeFileSync(path.join(b.build, "CMakeCache.txt"), mutation(b.cache));
    assert.throws(() => verifyCudaCmake(b.build, f.root, { log: quiet }));
  }
  const f = toolkit(t), b = cmakeBuild(f.root, f.compiler);
  fs.unlinkSync(b.compilerFile);
  assert.throws(() => verifyCudaCmake(b.build, f.root, { log: quiet }));
});

test("CMake cannot mix compiler paths, toolkit roots or newer compiler versions", t => {
  for (const location of ["cache compiler", "cache nvcc", "cache toolkit root", "compiler path", "compiler toolkit root", "compiler library root", "compiler version", "compiler toolkit version"]) {
    const f = toolkit(t), b = cmakeBuild(f.root, f.compiler);
    const otherRoot = path.join(path.dirname(f.root), "different toolkit").split(path.sep).join("/");
    fs.mkdirSync(path.join(otherRoot, "bin"), { recursive: true });
    fs.writeFileSync(path.join(otherRoot, "bin", "nvcc"), "unselected compiler");
    let cache = b.cache, compilerText = b.compilerText;
    if (location === "cache compiler") cache = cache.replace(/^CMAKE_CUDA_COMPILER:FILEPATH=.*$/m, `CMAKE_CUDA_COMPILER:FILEPATH=${otherRoot}/bin/nvcc`);
    if (location === "cache nvcc") cache = cache.replace(/^CUDAToolkit_NVCC_EXECUTABLE:FILEPATH=.*$/m, `CUDAToolkit_NVCC_EXECUTABLE:FILEPATH=${otherRoot}/bin/nvcc`);
    if (location === "cache toolkit root") cache = cache.replace(/^CUDAToolkit_ROOT:PATH=.*$/m, `CUDAToolkit_ROOT:PATH=${otherRoot}`);
    if (location === "compiler path") compilerText = compilerText.replace(/^set\(CMAKE_CUDA_COMPILER .*$/m, `set(CMAKE_CUDA_COMPILER "${otherRoot}/bin/nvcc")`);
    if (location === "compiler toolkit root") compilerText = compilerText.replace(/^set\(CMAKE_CUDA_COMPILER_TOOLKIT_ROOT .*$/m, `set(CMAKE_CUDA_COMPILER_TOOLKIT_ROOT "${otherRoot}")`);
    if (location === "compiler library root") compilerText = compilerText.replace(/^set\(CMAKE_CUDA_COMPILER_LIBRARY_ROOT .*$/m, `set(CMAKE_CUDA_COMPILER_LIBRARY_ROOT "${otherRoot}")`);
    if (location === "compiler version") compilerText = compilerText.replace('CMAKE_CUDA_COMPILER_VERSION "13.2.51"', 'CMAKE_CUDA_COMPILER_VERSION "13.2.86"');
    if (location === "compiler toolkit version") compilerText = compilerText.replace('CMAKE_CUDA_COMPILER_TOOLKIT_VERSION "13.2.51"', 'CMAKE_CUDA_COMPILER_TOOLKIT_VERSION "13.2.86"');
    fs.writeFileSync(path.join(b.build, "CMakeCache.txt"), cache);
    fs.writeFileSync(b.compilerFile, compilerText);
    assert.throws(() => verifyCudaCmake(b.build, f.root, { log: quiet }), undefined, location);
  }
});

test("CMake allows target-specific CUDA library directories and rejects external headers or runtime libraries", t => {
  const f = toolkit(t), b = cmakeBuild(f.root, f.compiler);
  const target = path.join(f.root, "targets", "x86_64-linux");
  const include = path.join(target, "include"), lib = path.join(target, "lib");
  fs.mkdirSync(include, { recursive: true });
  fs.mkdirSync(lib);
  const cudart = path.join(lib, "libcudart.so");
  fs.writeFileSync(cudart, "selected CUDA runtime");
  const cmakePath = value => value.split(path.sep).join("/");
  fs.writeFileSync(b.compilerFile, b.compilerText
    .replace(/^set\(CMAKE_CUDA_COMPILER_LIBRARY_ROOT .*$/m, `set(CMAKE_CUDA_COMPILER_LIBRARY_ROOT "${cmakePath(target)}")`)
    + `set(CMAKE_CUDA_COMPILER_TOOLKIT_LIBRARY_ROOT "${cmakePath(target)}")\n`);
  const cache = b.cache + [
    `CUDAToolkit_INCLUDE_DIR:PATH=${cmakePath(include)}`,
    `CUDAToolkit_LIBRARY_DIR:PATH=${cmakePath(lib)}`,
    `CUDA_CUDART:FILEPATH=${cmakePath(cudart)}`,
    "CUDA_nppc_LIBRARY:FILEPATH=CUDA_nppc_LIBRARY-NOTFOUND",
  ].join("\n") + "\n";
  fs.writeFileSync(path.join(b.build, "CMakeCache.txt"), cache);
  assert.doesNotThrow(() => verifyCudaCmake(b.build, f.root, { log: quiet }));

  const external = path.join(path.dirname(f.root), "external CUDA");
  fs.mkdirSync(external);
  const externalCudart = path.join(external, "libcudart.so");
  fs.writeFileSync(externalCudart, "unselected CUDA runtime");
  for (const [original, replacement] of [[include, external], [cudart, externalCudart]]) {
    fs.writeFileSync(path.join(b.build, "CMakeCache.txt"), cache.replace(cmakePath(original), cmakePath(replacement)));
    assert.throws(() => verifyCudaCmake(b.build, f.root, { log: quiet }), /escapes selected CUDA toolkit root/);
  }
  fs.writeFileSync(path.join(b.build, "CMakeCache.txt"), cache);
  fs.writeFileSync(b.compilerFile, b.compilerText + `set(CMAKE_CUDA_COMPILER_TOOLKIT_LIBRARY_ROOT "${cmakePath(external)}")\n`);
  assert.throws(() => verifyCudaCmake(b.build, f.root, { log: quiet }), /escapes selected CUDA toolkit root/);
});
