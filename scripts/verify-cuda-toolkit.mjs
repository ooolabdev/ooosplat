import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
// Read this directly: packaging modules also use this verifier and must not
// acquire a dependency cycle through colmap-runtime.mjs.
const defaultLock = JSON.parse(fs.readFileSync(path.join(workspace, "engines/colmap-build.json"), "utf8"));

function exact(label, actual, expected, log) {
  log(`${label}: ${actual ?? "<missing>"} (locked: ${expected})`);
  if (typeof expected !== "string" || !expected || actual !== expected) {
    throw new Error(`${label} differs from build lock: expected ${expected}, got ${actual ?? "<missing>"}`);
  }
}

function realPath(file, label) {
  try {
    return fs.realpathSync(file);
  } catch (error) {
    throw new Error(`Missing or inaccessible ${label}: ${file}`, { cause: error });
  }
}

function insideRoot(root, file, label) {
  const resolved = realPath(file, label);
  const relative = path.relative(root, resolved);
  if (relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error(`${label} escapes selected CUDA toolkit root: ${resolved} (root: ${root})`);
  }
  return resolved;
}

function compilerPath(root, file, log, label) {
  const resolved = insideRoot(root, file, label);
  if (!fs.statSync(resolved).isFile()) throw new Error(`${label} is not a file: ${resolved}`);
  const name = path.basename(resolved);
  if (!/^nvcc(?:\.exe)?$/i.test(name)) throw new Error(`${label} is not an NVIDIA nvcc executable: ${resolved}`);
  const selected = insideRoot(root, path.join(root, "bin", name), "selected CUDA compiler");
  exact(label, resolved, selected, log);
  return resolved;
}

export function validateCudaMetadata(platform, metadata, { lock = defaultLock, log = console.log } = {}) {
  if (!["windows", "linux"].includes(platform)) throw new Error(`Unsupported CUDA platform: ${platform}`);
  const identity = lock.cudaToolkitIdentity;
  const expected = identity?.platforms?.[platform];
  if (!expected || ["cuda_nvcc", "cuda_cudart", "cuda_crt", "libcurand"].some(name => typeof identity.components?.[name] !== "string" || !identity.components[name])) {
    throw new Error("Missing exact CUDA toolkit identity in build lock");
  }
  log(`CUDA release: ${lock.cudaVersion} (${platform}; official metadata: ${expected.metadataUrl})`);
  exact(`CUDA ${platform} metadata version`, metadata?.cuda?.version, expected.metadataVersion, log);
  const components = {};
  for (const [name, version] of Object.entries(identity.components)) {
    exact(`CUDA component ${name}`, metadata?.[name]?.version, version, log);
    components[name] = version;
  }
  return { releaseVersion: lock.cudaVersion, metadataVersion: expected.metadataVersion, components };
}

export function verifyCudaToolkit(platform, cudaRoot, { lock = defaultLock, execute = spawnSync, log = console.log } = {}) {
  if (!["windows", "linux"].includes(platform)) throw new Error(`Unsupported CUDA platform: ${platform}`);
  const root = realPath(path.resolve(cudaRoot), "CUDA toolkit root");
  log(`Selected CUDA toolkit root: ${root}`);
  const metadataFile = insideRoot(root, path.join(root, "version.json"), "CUDA toolkit metadata");
  const metadata = validateCudaMetadata(platform, JSON.parse(fs.readFileSync(metadataFile, "utf8")), { lock, log });
  const compiler = compilerPath(root, path.join(root, "bin", platform === "windows" ? "nvcc.exe" : "nvcc"), log, "CUDA nvcc path");
  const result = execute(compiler, ["--version"], { encoding: "utf8", timeout: 15000, windowsHide: true });
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
  if (result.error || result.status !== 0) {
    throw new Error(`CUDA nvcc --version failed (${compiler}): ${result.error?.message ?? result.signal ?? `exit ${result.status}`}\n${output}`);
  }
  const versions = [...output.matchAll(/\bV(\d+\.\d+\.\d+)\b/g)].map(match => match[1]);
  const version = versions.length === 1 ? versions[0] : undefined;
  exact("CUDA nvcc executable version", version, lock.cudaToolkitIdentity.components.cuda_nvcc, log);
  return { ...metadata, compiler: { path: compiler, version } };
}

function readCache(cache) {
  const entries = new Map();
  for (const line of fs.readFileSync(cache, "utf8").split(/\r?\n/)) {
    const match = /^([^#/:][^:]*):([^=]+)=(.*)$/.exec(line);
    if (match) entries.set(match[1], { type: match[2], value: match[3] });
  }
  return entries;
}

function cmakeSet(source, name) {
  const match = new RegExp(`^\\s*set\\(\\s*${name}\\s+"((?:\\\\.|[^"\\\\])*)"\\s*\\)`, "m").exec(source);
  // CMake writes paths with forward slashes; accept its quoted-string escapes
  // without interpreting ordinary Windows backslashes as JS escape sequences.
  return match?.[1].replace(/\\([\\";])/g, "$1");
}

export function verifyCudaCmake(build, cudaRoot, { lock = defaultLock, log = console.log } = {}) {
  const root = realPath(path.resolve(cudaRoot), "CUDA toolkit root");
  const buildRoot = path.resolve(build);
  const cache = readCache(path.join(buildRoot, "CMakeCache.txt"));
  const configuredCompiler = cache.get("CMAKE_CUDA_COMPILER")?.value;
  if (!configuredCompiler) throw new Error("CMake cache is missing CMAKE_CUDA_COMPILER");
  const compiler = compilerPath(root, configuredCompiler, log, "CMake cached CUDA compiler");
  const compilerFile = path.join(buildRoot, "CMakeFiles", lock.cmakeVersion, "CMakeCUDACompiler.cmake");
  const source = fs.readFileSync(compilerFile, "utf8");
  const detectedCompiler = cmakeSet(source, "CMAKE_CUDA_COMPILER");
  if (!detectedCompiler) throw new Error(`Missing CMAKE_CUDA_COMPILER in ${compilerFile}`);
  exact("CMake detected CUDA compiler", compilerPath(root, detectedCompiler, log, "CMake detected CUDA compiler path"), compiler, log);
  const version = cmakeSet(source, "CMAKE_CUDA_COMPILER_VERSION");
  exact("CMake CUDA compiler version", version, lock.cudaToolkitIdentity.components.cuda_nvcc, log);
  const toolkitVersion = cmakeSet(source, "CMAKE_CUDA_COMPILER_TOOLKIT_VERSION");
  if (toolkitVersion !== undefined) exact("CMake CUDA toolkit version", toolkitVersion, lock.cudaToolkitIdentity.components.cuda_nvcc, log);
  const toolkitRoot = cmakeSet(source, "CMAKE_CUDA_COMPILER_TOOLKIT_ROOT");
  if (toolkitRoot) exact("CMAKE_CUDA_COMPILER_TOOLKIT_ROOT", realPath(toolkitRoot, "CMAKE_CUDA_COMPILER_TOOLKIT_ROOT"), root, log);
  for (const name of ["CMAKE_CUDA_COMPILER_LIBRARY_ROOT", "CMAKE_CUDA_COMPILER_TOOLKIT_LIBRARY_ROOT"]) {
    const value = cmakeSet(source, name);
    if (value) log(`${name}: ${insideRoot(root, value, name)} (within selected CUDA root: ${root})`);
  }
  const visibleNvcc = cache.get("CUDAToolkit_NVCC_EXECUTABLE")?.value;
  if (visibleNvcc !== undefined) exact("CMake discovered nvcc", compilerPath(root, visibleNvcc, log, "CMake CUDAToolkit_NVCC_EXECUTABLE"), compiler, log);
  const visibleVersion = cache.get("CUDAToolkit_VERSION")?.value;
  if (visibleVersion !== undefined) exact("CMake CUDAToolkit_VERSION", visibleVersion, lock.cudaToolkitIdentity.components.cuda_nvcc, log);
  const visibleRoot = cache.get("CUDAToolkit_ROOT")?.value;
  if (visibleRoot !== undefined) exact("CMake CUDAToolkit_ROOT", realPath(visibleRoot, "CMake CUDAToolkit_ROOT"), root, log);
  // CMake versions expose different subsets of these variables in the cache.
  // Inspect paths that are actually present, not non-cached result variables.
  for (const [name, { value }] of cache) {
    const cudaPath = /^(?:CUDAToolkit_(?:INCLUDE_DIRS?|INCLUDE_DIRECTORIES|LIBRARY_DIR)|CUDA_(?:CUDART|LIBRARY_DIR|.*_LIBRARY))$/i.test(name);
    if (!cudaPath || !value || value.endsWith("-NOTFOUND")) continue;
    for (const selectedPath of value.split(";")) {
      if (selectedPath) log(`CMake ${name}: ${insideRoot(root, selectedPath, `CMake ${name}`)} (within selected CUDA root: ${root})`);
    }
  }
  const architecture = cache.get("CMAKE_CUDA_ARCHITECTURES");
  exact("CMake CUDA architecture cache type", architecture?.type, "STRING", log);
  exact("CMake CUDA architectures", architecture?.value, lock.cudaArchitectures.join(";"), log);
  return { compiler: { path: compiler, version }, architectures: [...lock.cudaArchitectures] };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [platform, root, build, ...extra] = process.argv.slice(2);
  try {
    if (!platform || !root || extra.length) throw new Error("Usage: node scripts/verify-cuda-toolkit.mjs <windows|linux> <root> [build]");
    verifyCudaToolkit(platform, root);
    if (build) verifyCudaCmake(build, root);
    console.log(`Verified exact CUDA ${defaultLock.cudaVersion} toolkit identity${build ? " and CMake selection" : ""} (${platform}).`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
