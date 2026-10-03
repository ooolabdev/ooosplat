import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { buildLock, workspace, fileHash, filesUnder, verifyColmap } from "./colmap-runtime.mjs";
import { verifyMacosRuntime } from "./verify-macos-colmap-runtime.mjs";

const [stageArg, buildArg, bytesArg] = process.argv.slice(2);
const stage = path.resolve(stageArg), build = path.resolve(buildArg);
if (!stage.startsWith(`${path.join(workspace, ".cache", "colmap-macos-build")}${path.sep}`)) throw new Error("Unsafe staging directory");
const cache = fs.readFileSync(path.join(build, "colmap", "CMakeCache.txt"), "utf8");
for (const feature of [...buildLock.disabledFeatures, "CUDA", "CASPAR"]) {
  if (!cache.split(/\r?\n/).includes(`${feature}_ENABLED:BOOL=OFF`)) throw new Error(`Actual build did not disable ${feature}`);
}
if (!cache.includes("CASPAR_USE_DOUBLE:BOOL=OFF") || !cache.includes("CMAKE_OSX_ARCHITECTURES:STRING=arm64")) throw new Error("Actual macOS precision/architecture differs from policy");
if (fs.readdirSync(path.join(stage, "bin")).join() !== "colmap") throw new Error("COLMAP-only archive contains another executable");
if (filesUnder(stage).some(file => /(?:cuda|cudnn|cudart|curand)/i.test(path.relative(stage, file)))) throw new Error("GPU resource in CPU-only macOS runtime");
const run = (command, args, timeout = 30000) => {
  const result = spawnSync(command, args, { encoding: "utf8", timeout });
  if (result.error || result.status !== 0) throw new Error(`${command} failed: ${result.error ?? result.stderr}`);
  return result.stdout.trim();
};
const manifest = JSON.parse(fs.readFileSync(path.join(workspace, "engines", "manifest.macos.json"), "utf8"));
const colmapFeatures = Object.fromEntries(buildLock.disabledFeatures.map(feature => [feature, false]));
Object.assign(colmapFeatures, { CUDA: false, CASPAR: false, CERES: true, CASPAR_USE_DOUBLE: false });
const info = {
  schemaVersion: 1, platform: "macos", architecture: "arm64", minimumSystemVersion: "15.0", generatedAt: new Date().toISOString(),
  colmap: { version: buildLock.version, commit: buildLock.commit, sourceUrl: buildLock.sourceUrl, sourceSha256: buildLock.sourceSha256 },
  colmapFeatures, compiler: run("clang", ["--version"]), cmakeVersion: run("cmake", ["--version"]), ninjaVersion: run("ninja", ["--version"]),
  dependencies: { homebrewCoreCommit: manifest.buildEnvironment.homebrewCoreCommit,
    installedFormulae: run("brew", ["list", "--versions", ...manifest.buildEnvironment.runtimeFormulae]) },
  trimPolicy: buildLock.trimPolicy,
};
fs.writeFileSync(path.join(stage, "BUILD-INFO.json"), JSON.stringify(info, null, 2) + "\n");
fs.writeFileSync(path.join(stage, "SHA256SUMS"), filesUnder(stage).filter(file => path.basename(file) !== "SHA256SUMS")
  .map(file => `${fileHash(file)}  ${path.relative(stage, file).split(path.sep).join("/")}`).join("\n") + "\n");
verifyColmap(stage, "macos", fileHash(path.join(stage, "SHA256SUMS")));
const inventory = JSON.parse(fs.readFileSync(path.join(stage, "BUNDLED-COMPONENTS.json"), "utf8"));
if (inventory.components.some(component => !component.licenseFiles?.length)) throw new Error("Missing dependency notices in COLMAP-only runtime");
verifyMacosRuntime(stage);
const smoke = spawnSync(process.execPath, [path.join(workspace, "scripts", "smoke-colmap-image-io.mjs"), path.join(stage, "bin", "colmap")], { stdio: "inherit" });
if (smoke.error || smoke.status !== 0) throw new Error("COLMAP image/CPU SIFT smoke failed");

const archive = path.join(workspace, "dist-engines", "ooosplat-colmap-macos-arm64-v0.1.0.tar.xz");
fs.mkdirSync(path.dirname(archive), { recursive: true });
run("tar", ["-cJf", archive, "-C", path.dirname(stage), path.basename(stage)], 0);
fs.writeFileSync(`${archive}.sha256`, `${fileHash(archive)}  ${path.basename(archive)}\n`);
const runtimeBytes = filesUnder(stage).reduce((sum, file) => sum + fs.statSync(file).size, 0);
const removedDevelopmentBytes = Number(bytesArg);
if (!Number.isSafeInteger(removedDevelopmentBytes) || removedDevelopmentBytes < 0) throw new Error("Invalid trim measurement");
const report = {
  platform: "macos", archive: path.basename(archive), archiveSha256: fileHash(archive), integritySha256: fileHash(path.join(stage, "SHA256SUMS")),
  preTrimBytes: runtimeBytes + removedDevelopmentBytes, removedDevelopmentBytes,
  compressedBytes: fs.statSync(archive).size, runtimeBytes,
  features: colmapFeatures, gpuExecutionValidated: false,
  note: "Ceres CPU-only Apple Silicon runtime; no Brush/FFmpeg executables, CUDA or Caspar. Pre-trim bytes include final dependencies/notices plus measured removed COLMAP development artifacts.",
};
fs.writeFileSync(`${archive}.build-report.json`, JSON.stringify(report, null, 2) + "\n");
console.log(JSON.stringify(report, null, 2));
