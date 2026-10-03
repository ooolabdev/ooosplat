import fs from "node:fs";
import path from "node:path";
import { workspace, buildLock, assertHashPin } from "./colmap-runtime.mjs";
assertHashPin(buildLock.sourceSha256, "COLMAP source");
if (buildLock.version !== "4.2.1" || buildLock.commit !== "bd1fcf654d2dd8fefa1466999c190a246f83f4b9") throw new Error("Unexpected COLMAP release");
const manifests = ["manifest.json", "manifest.linux.json", "manifest.macos.json"].map(name => JSON.parse(fs.readFileSync(path.join(workspace, "engines", name), "utf8")));
const entries = [manifests[0].engines.find(e => e.name === "COLMAP"), manifests[1].colmap, manifests[2].engines.find(e => e.name === "COLMAP")];
for (const entry of entries) if (entry.version !== buildLock.version || entry.commit !== buildLock.commit) throw new Error("Platforms must use the same COLMAP source commit");
if (entries[2].sourceSha256 !== buildLock.sourceSha256 || entries[2].sourceUrl !== buildLock.sourceUrl) throw new Error("macOS source lock differs");
if (manifests[1].systemEngines.includes("colmap")) throw new Error("Linux system COLMAP is forbidden");
for (const entry of entries.slice(0, 2)) {
  const cuda = entry.cudaCompatibility;
  if (cuda.toolkitVersion !== buildLock.cudaVersion || cuda.architecturePolicy !== buildLock.cudaArchitectures.join(";") || cuda.minimumComputeCapability !== buildLock.minimumComputeCapability || (cuda.minimumDriver ?? cuda.minimumWindowsDriver) !== buildLock.minimumDriver) throw new Error("Platform CUDA lock differs");
  if (!entry.sourceUrl.startsWith("https://github.com/ooolabdev/ooosplat/releases/download/")) throw new Error("Expected self-built versioned COLMAP release");
}
if (process.argv.includes("--require-archives")) {
  for (const entry of entries.slice(0, 2)) assertHashPin(entry.archiveSha256, "COLMAP runtime archive");
  assertHashPin(manifests[0].requiredFiles.find(f => f.path === "engines/colmap/SHA256SUMS")?.sha256, "Windows integrity inventory");
  assertHashPin(entries[1].integritySha256, "Linux integrity inventory");
  assertHashPin(manifests[2].distribution.archiveSha256, "macOS runtime archive");
  assertHashPin(manifests[2].distribution.integritySha256, "macOS integrity inventory");
}
console.log(`Verified three-platform COLMAP source/CUDA policy lock: ${buildLock.version} ${buildLock.commit}`);
