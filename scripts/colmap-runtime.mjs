import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const buildLock = JSON.parse(fs.readFileSync(path.join(workspace, "engines/colmap-build.json"), "utf8"));
export const sha256 = bytes => createHash("sha256").update(bytes).digest("hex");
export const fileHash = file => sha256(fs.readFileSync(file));
export function assertHashPin(hash, label) {
  if (!/^[a-f0-9]{64}$/i.test(hash ?? "")) throw new Error(`${label}: first build is not hash-locked. Build/review the engine archive and run scripts/lock-engine-archive.mjs; do not reuse the old runtime.`);
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
export function verifyIntegrity(root, pin) {
  assertHashPin(pin, "Runtime integrity pin");
  if (fileHash(path.join(root, "SHA256SUMS")).toLowerCase() !== pin.toLowerCase()) throw new Error("SHA256SUMS does not match the reviewed manifest");
  const covered = new Set();
  for (const line of fs.readFileSync(path.join(root, "SHA256SUMS"), "utf8").trim().split(/\r?\n/)) {
    const match = /^([a-f0-9]{64})  (.+)$/i.exec(line);
    if (!match) throw new Error("Malformed SHA256SUMS");
    const relative = match[2];
    if (relative.includes("\\") || relative.split("/").some(p => !p || p === "." || p === "..") || path.isAbsolute(relative) || relative.includes(":")) throw new Error("Unsafe integrity path");
    if (covered.has(relative)) throw new Error("Duplicate integrity entry");
    covered.add(relative);
    if (fileHash(path.join(root, relative)).toLowerCase() !== match[1].toLowerCase()) throw new Error(`Runtime hash mismatch: ${relative}`);
  }
  for (const file of filesUnder(root)) {
    const relative = path.relative(root, file).split(path.sep).join("/");
    if (!["README.md", "SHA256SUMS"].includes(relative) && !covered.has(relative)) throw new Error(`Unverified runtime file: ${relative}`);
  }
  for (const file of ["BUILD-INFO.json", "BUNDLED-COMPONENTS.json", "licenses/COLMAP-LICENSE.txt"]) {
    if (!covered.has(file)) throw new Error(`Missing runtime inventory: ${file}`);
  }
}
export function verifyColmap(root, platform, pin, { run = true } = {}) {
  verifyIntegrity(root, pin);
  const info = JSON.parse(fs.readFileSync(path.join(root, "BUILD-INFO.json"), "utf8"));
  const source = info.colmap ?? info.sources?.find(source => source.name === "COLMAP");
  if (source?.commit !== buildLock.commit || source.version !== buildLock.version || source.sourceSha256 !== buildLock.sourceSha256) throw new Error("COLMAP source identity mismatch");
  if (info.platform !== platform) throw new Error("Runtime platform mismatch");
  const features = info.colmapFeatures;
  if (!features || buildLock.disabledFeatures.some(feature => features[feature] !== false) || features.CASPAR !== (platform !== "macos") || features.CUDA !== (platform !== "macos") || features.CERES !== true || features.CASPAR_USE_DOUBLE !== false) throw new Error("COLMAP feature policy mismatch");
  if (platform !== "macos" && (info.cudaVersion !== buildLock.cudaVersion || JSON.stringify(info.cudaArchitectures) !== JSON.stringify(buildLock.cudaArchitectures))) throw new Error("CUDA build lock mismatch");
  const binary = path.join(root, "bin", platform === "windows" ? "colmap.exe" : "colmap");
  const covered = new Set(fs.readFileSync(path.join(root, "SHA256SUMS"), "utf8").split(/\r?\n/).filter(Boolean).map(line => line.slice(66)));
  if (!covered.has(`bin/${path.basename(binary)}`)) throw new Error("COLMAP executable is not hash locked");
  const inventory = JSON.parse(fs.readFileSync(path.join(root, "BUNDLED-COMPONENTS.json"), "utf8"));
  if (!inventory.components?.length || !inventory.sourceLicenseFiles?.length) throw new Error("Missing component/license inventory");
  for (const license of inventory.sourceLicenseFiles) {
    if (!covered.has(`licenses/${license}`)) throw new Error(`Unverified source license: ${license}`);
  }
  for (const component of inventory.components) {
    if (platform !== "macos" && !component.licenseFiles?.length) throw new Error(`Missing component license: ${component.name}`);
    for (const license of component.licenseFiles ?? []) if (!covered.has(`licenses/${license}`)) throw new Error(`Missing component license file: ${license}`);
  }
  const licensed = new Set(inventory.components.flatMap(component => component.files));
  for (const file of filesUnder(root)) {
    const relative = path.relative(root, file).split(path.sep).join("/");
    if (/\.(?:dll|so(?:\..*)?|dylib)$/i.test(file) && !licensed.has(relative)) throw new Error(`Missing dependency license inventory: ${relative}`);
    if (/(?:^|\/)(?:include|share|cmake|pkgconfig|tests)(?:\/|$)|(?:_test\.exe|\.pdb|\.a|\.lib|vocab_tree.*\.bin)$/i.test(relative)) throw new Error(`Development/unused artifact shipped: ${relative}`);
  }
  if (!run) return;
  const environment = { ...process.env, PATH: platform === "windows" ? path.join(process.env.SystemRoot ?? "C:\\Windows", "System32") : "/usr/bin:/bin:/usr/sbin:/sbin" };
  delete environment.LD_LIBRARY_PATH;
  delete environment.DYLD_LIBRARY_PATH;
  delete environment.DYLD_FALLBACK_LIBRARY_PATH;
  for (const command of ["feature_extractor", "sequential_matcher", "exhaustive_matcher", "matches_importer", "mapper", "bundle_adjuster"]) {
    const result = spawnSync(binary, [command, "-h"], { encoding: "utf8", timeout: 15000, windowsHide: true, env: environment });
    const help = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
    if (result.error || result.status !== 0 || !help.includes(`COLMAP ${buildLock.version}`) || !help.includes(buildLock.commit)) throw new Error(`Wrong COLMAP executable or failed ${command}: ${result.error ?? help}`);
    if (command === "bundle_adjuster" && help.includes("--BundleAdjustmentCaspar.gpu_index") !== (platform !== "macos")) throw new Error("Wrong compiled Caspar capability");
    if (command === "bundle_adjuster" && !help.includes("--BundleAdjustmentCeres.use_gpu")) throw new Error("Missing Ceres fallback capability");
    if (command === "feature_extractor" && !help.includes("--FeatureExtraction.use_gpu")) throw new Error("Missing modern SIFT CLI");
    if (command === "feature_extractor" && /with cuda/i.test(help) !== (platform !== "macos")) throw new Error("Wrong compiled CUDA capability");
    if (command === "mapper" && !["--Mapper.ba_local_backend", "--Mapper.ba_global_backend", "--Mapper.ba_gpu_index"].every(flag => help.includes(flag))) throw new Error("Missing mapper backend options");
  }
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [root, platform, pin] = process.argv.slice(2);
  verifyColmap(path.resolve(root), platform, pin);
  console.log(`Verified hash-locked COLMAP ${buildLock.version} ${buildLock.commit} (${platform}).`);
}
