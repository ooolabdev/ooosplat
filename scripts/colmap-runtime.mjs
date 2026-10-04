import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { validateCudaMetadata } from "./verify-cuda-toolkit.mjs";

export const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const buildLock = JSON.parse(fs.readFileSync(path.join(workspace, "engines/colmap-build.json"), "utf8"));
export const runtimeLock = JSON.parse(fs.readFileSync(path.join(workspace, "engines/colmap-runtime.json"), "utf8"));
export const sha256 = bytes => createHash("sha256").update(bytes).digest("hex");
export const fileHash = file => sha256(fs.readFileSync(file));

export function isLockedColmapHelp(help) {
  const candidates = [runtimeLock.sourceCommit.toLowerCase(), runtimeLock.sourceCommit.slice(0, 7).toLowerCase()];
  return candidates.some(commit => {
    const marker = `COLMAP ${runtimeLock.colmapVersion} (Commit ${commit}`;
    const index = help.toLowerCase().indexOf(marker.toLowerCase());
    if (index < 0) return false;
    const boundary = help[index + marker.length];
    return boundary === ")" || /\s/.test(boundary ?? "");
  });
}

export function assertHashPin(hash, label) {
  if (!/^[a-f0-9]{64}$/i.test(hash ?? "")) throw new Error(`${label} is not hash-locked`);
}

export function filesUnder(root) {
  const files = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const file = path.join(root, entry.name);
    if (entry.isDirectory()) files.push(...filesUnder(file));
    else if (entry.isFile()) files.push(file);
    else throw new Error(`Runtime symlinks/special files must be materialized before packaging: ${file}`);
  }
  return files.sort();
}

export function readIntegrity(root) {
  const covered = new Set();
  for (const line of fs.readFileSync(path.join(root, "SHA256SUMS"), "utf8").trim().split(/\r?\n/)) {
    const match = /^([a-f0-9]{64})  (.+)$/i.exec(line);
    if (!match) throw new Error("Malformed SHA256SUMS");
    const relative = match[2];
    if (relative.includes("\\") || relative.split("/").some(part => !part || part === "." || part === "..") || path.isAbsolute(relative) || relative.includes(":")) throw new Error("Unsafe integrity path");
    if (covered.has(relative)) throw new Error("Duplicate integrity entry");
    covered.add(relative);
    const target = path.resolve(root, ...relative.split("/"));
    if (!target.startsWith(`${path.resolve(root)}${path.sep}`) || fileHash(target).toLowerCase() !== match[1].toLowerCase()) throw new Error(`Runtime hash mismatch: ${relative}`);
  }
  return covered;
}

export function verifyIntegrity(root, pin) {
  assertHashPin(pin, "Runtime integrity pin");
  if (fileHash(path.join(root, "SHA256SUMS")).toLowerCase() !== pin.toLowerCase()) throw new Error("SHA256SUMS does not match the reviewed release lock");
  const covered = readIntegrity(root);
  for (const file of filesUnder(root)) {
    const relative = path.relative(root, file).split(path.sep).join("/");
    if (!["README.md", "SHA256SUMS"].includes(relative) && !covered.has(relative)) throw new Error(`Unverified runtime file: ${relative}`);
  }
  for (const file of ["BUILD-INFO.json", "BUNDLED-COMPONENTS.json"]) {
    if (!covered.has(file)) throw new Error(`Missing runtime inventory: ${file}`);
  }
  return covered;
}

function verifyReleaseMetadata(root, platform, covered, lock) {
  const expected = lock.platforms[platform];
  if (!expected) throw new Error(`Unsupported COLMAP runtime platform: ${platform}`);
  const infoPath = path.join(root, "BUILD-INFO.json");
  if (fileHash(infoPath).toLowerCase() !== expected.buildInfoSha256.toLowerCase()) throw new Error("BUILD-INFO.json does not match the reviewed release lock");
  const info = JSON.parse(fs.readFileSync(infoPath, "utf8"));
  if (info.repository !== lock.repository || info.runtimeRevision !== lock.runtimeRevision
    || info.sourceCommit !== lock.sourceCommit || info.scriptCommit !== lock.scriptCommit
    || info.colmapVersion !== lock.colmapVersion) throw new Error("COLMAP release identity mismatch");
  if (info.platform !== platform || info.architecture !== expected.architecture) throw new Error("Runtime platform/architecture mismatch");
  const features = info.features;
  if (!features || lock.disabledFeatures.some(feature => features[feature] !== false)
    || features.cpuSift !== true || features.ceresCpuBA !== true
    || features.offlineLoopDetection !== true || features.vocabTreeMatching !== true
    || features.CUDA_ENABLED !== expected.cuda || features.CASPAR_ENABLED !== expected.caspar
    || features.CASPAR_USE_DOUBLE !== false) throw new Error("COLMAP feature policy mismatch");
  if (expected.cuda) {
    if (info.cudaRelease !== lock.cudaRelease || JSON.stringify(info.gpuArchitectures) !== JSON.stringify(lock.gpuArchitectures)) throw new Error("CUDA build lock mismatch");
  } else if (info.cudaRelease !== null || !Array.isArray(info.gpuArchitectures) || info.gpuArchitectures.length !== 0) {
    throw new Error("CPU-only runtime unexpectedly declares CUDA");
  }
  const vocabulary = lock.offlineVocabulary;
  if (JSON.stringify(info.offlineVocabulary && {
    path: info.offlineVocabulary.path,
    sha256: info.offlineVocabulary.sha256,
    bytes: info.offlineVocabulary.bytes,
  }) !== JSON.stringify(vocabulary)) throw new Error("Offline vocabulary identity mismatch");
  const vocabularyPath = path.join(root, ...vocabulary.path.split("/"));
  if (!covered.has(vocabulary.path) || fileHash(vocabularyPath).toLowerCase() !== vocabulary.sha256 || fs.statSync(vocabularyPath).size !== vocabulary.bytes) throw new Error("Offline vocabulary is missing or corrupt");

  const inventory = JSON.parse(fs.readFileSync(path.join(root, "BUNDLED-COMPONENTS.json"), "utf8"));
  if (inventory.schemaVersion !== 1 || !Array.isArray(inventory.components) || inventory.components.length === 0) throw new Error("Missing component/license inventory");
  const runtimeFiles = new Set();
  for (const component of inventory.components) {
    if (!component.name || !Array.isArray(component.licenseFiles) || !Array.isArray(component.runtimeFiles)) throw new Error("Malformed component/license inventory");
    for (const relative of component.licenseFiles) {
      if (!relative.startsWith("licenses/") || !covered.has(relative)) throw new Error(`Missing component license file: ${relative}`);
    }
    for (const relative of component.runtimeFiles) {
      if (!covered.has(relative)) throw new Error(`Missing component runtime file: ${relative}`);
      runtimeFiles.add(relative);
    }
  }
  const binaryName = platform === "windows" ? "colmap.exe" : "colmap";
  for (const required of [`bin/${binaryName}`, vocabulary.path, "lib/validation/model/cameras.txt", "lib/validation/model/images.txt", "lib/validation/model/points3D.txt"]) {
    if (!covered.has(required) || !runtimeFiles.has(required)) throw new Error(`Required COLMAP runtime resource is not inventoried: ${required}`);
  }
  for (const file of filesUnder(root)) {
    const relative = path.relative(root, file).split(path.sep).join("/");
    if (/\.(?:dll|so(?:\..*)?|dylib)$/i.test(file) && !runtimeFiles.has(relative)) throw new Error(`Missing dependency license inventory: ${relative}`);
    const developmentDirectory = /(?:^|\/)(?:include|cmake|pkgconfig|tests)(?:\/|$)/i.test(relative) && !relative.startsWith("licenses/");
    const developmentFile = /(?:_test(?:\.exe)?|\.pdb)$/i.test(relative)
      || (!relative.startsWith("licenses/") && /(?:\.a|\.lib)$/i.test(relative));
    if (developmentDirectory || developmentFile) throw new Error(`Development artifact shipped: ${relative}`);
  }
}

// Manual COLMAP-only builders still validate freshly produced legacy metadata.
// Formal application setup additionally requires the reviewed fork schema.
function verifyBuilderMetadata(root, platform) {
  const info = JSON.parse(fs.readFileSync(path.join(root, "BUILD-INFO.json"), "utf8"));
  const source = info.colmap ?? info.sources?.find(item => item.name === "COLMAP");
  if (source?.commit !== buildLock.commit || source.version !== buildLock.version || source.sourceSha256 !== buildLock.sourceSha256) throw new Error("COLMAP source identity mismatch");
  if (info.platform !== platform) throw new Error("Runtime platform mismatch");
  const features = info.colmapFeatures;
  if (!features || buildLock.disabledFeatures.some(feature => features[feature] !== false)
    || features.CASPAR !== (platform !== "macos") || features.CUDA !== (platform !== "macos")
    || features.CERES !== true || features.CASPAR_USE_DOUBLE !== false) throw new Error("COLMAP feature policy mismatch");
  if (platform !== "macos") {
    if (info.cudaVersion !== buildLock.cudaVersion || JSON.stringify(info.cudaArchitectures) !== JSON.stringify(buildLock.cudaArchitectures)) throw new Error("CUDA build lock mismatch");
    const actual = info.cudaToolkit;
    if (!actual || actual.releaseVersion !== buildLock.cudaVersion) throw new Error("Missing actual CUDA toolkit identity");
    const metadata = { cuda: { version: actual.metadataVersion } };
    for (const [name, version] of Object.entries(actual.components ?? {})) metadata[name] = { version };
    validateCudaMetadata(platform, metadata, { log: () => {} });
  }
  const inventory = JSON.parse(fs.readFileSync(path.join(root, "BUNDLED-COMPONENTS.json"), "utf8"));
  if (!inventory.components?.length || !inventory.sourceLicenseFiles?.length) throw new Error("Missing component/license inventory");
}

export function verifyColmap(root, platform, pin, { run = true, requireRelease = false, releaseLock = runtimeLock } = {}) {
  const covered = verifyIntegrity(root, pin);
  const info = JSON.parse(fs.readFileSync(path.join(root, "BUILD-INFO.json"), "utf8"));
  const published = info.repository === releaseLock.repository && Object.hasOwn(info, "runtimeRevision");
  if (requireRelease && !published) throw new Error("COLMAP runtime is not the locked fork Release");
  if (published) verifyReleaseMetadata(root, platform, covered, releaseLock);
  else verifyBuilderMetadata(root, platform);

  const binary = path.join(root, "bin", platform === "windows" ? "colmap.exe" : "colmap");
  if (!covered.has(`bin/${path.basename(binary)}`)) throw new Error("COLMAP executable is not hash locked");
  if (!run) return;
  const environment = { ...process.env, PATH: platform === "windows" ? path.join(process.env.SystemRoot ?? "C:\\Windows", "System32") : "/usr/bin:/bin:/usr/sbin:/sbin" };
  delete environment.LD_LIBRARY_PATH;
  delete environment.DYLD_LIBRARY_PATH;
  delete environment.DYLD_FALLBACK_LIBRARY_PATH;
  for (const command of ["feature_extractor", "sequential_matcher", "exhaustive_matcher", "matches_importer", "mapper", "bundle_adjuster"]) {
    const result = spawnSync(binary, [command, "-h"], { encoding: "utf8", timeout: 15000, windowsHide: true, env: environment });
    const help = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
    if (result.error || result.status !== 0 || !isLockedColmapHelp(help)) throw new Error(`Wrong COLMAP executable or failed ${command}: ${result.error ?? help}`);
    if (command === "bundle_adjuster" && help.includes("--BundleAdjustmentCaspar.gpu_index") !== (platform !== "macos")) throw new Error("Wrong compiled Caspar capability");
    if (command === "bundle_adjuster" && !help.includes("--BundleAdjustmentCeres.use_gpu")) throw new Error("Missing Ceres fallback capability");
    if (command === "feature_extractor" && !help.includes("--FeatureExtraction.use_gpu")) throw new Error("Missing modern SIFT CLI");
    if (command === "feature_extractor" && /with cuda/i.test(help) !== (platform !== "macos")) throw new Error("Wrong compiled CUDA capability");
    if (command === "mapper" && !["--Mapper.ba_local_backend", "--Mapper.ba_global_backend", "--Mapper.ba_gpu_index"].every(flag => help.includes(flag))) throw new Error("Missing mapper backend options");
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [root, platform, pin, mode] = process.argv.slice(2);
  if (mode === "integrity") {
    verifyIntegrity(path.resolve(root), pin);
    console.log(`Verified hash-locked ${platform} runtime inventory.`);
  } else {
    verifyColmap(path.resolve(root), platform, pin, { requireRelease: mode === "release" });
    console.log(`Verified hash-locked COLMAP ${runtimeLock.colmapVersion} ${runtimeLock.sourceCommit} (${platform}).`);
  }
}
