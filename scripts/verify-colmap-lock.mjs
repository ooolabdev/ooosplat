import fs from "node:fs";
import path from "node:path";
import { workspace, buildLock, runtimeLock, assertHashPin } from "./colmap-runtime.mjs";

assertHashPin(buildLock.sourceSha256, "COLMAP source");
if (buildLock.version !== runtimeLock.colmapVersion || buildLock.commit !== runtimeLock.sourceCommit) throw new Error("Builder and published runtime source locks differ");
if (buildLock.cudaVersion !== runtimeLock.cudaRelease || JSON.stringify(buildLock.cudaArchitectures) !== JSON.stringify(runtimeLock.gpuArchitectures)) throw new Error("Builder and published CUDA policies differ");
if (runtimeLock.repository !== "ooolabdev/ooosplat-colmap" || runtimeLock.releaseTag !== "colmap-4.2.1-runtime.1" || runtimeLock.runtimeRevision !== 1) throw new Error("Unexpected COLMAP runtime Release");
if (runtimeLock.scriptCommit !== "4c9b2bf412a2af4225b4ec9f3ffb9f2392c66aa1") throw new Error("Unexpected COLMAP runtime build-script commit");

const manifests = ["manifest.json", "manifest.linux.json", "manifest.macos.json"].map(name => JSON.parse(fs.readFileSync(path.join(workspace, "engines", name), "utf8")));
const entries = [manifests[0].engines.find(entry => entry.name === "COLMAP"), manifests[1].colmap, manifests[2].engines.find(entry => entry.name === "COLMAP")];
for (const [index, platform] of ["windows", "linux", "macos"].entries()) {
  const entry = entries[index], expected = runtimeLock.platforms[platform];
  if (entry.version !== runtimeLock.colmapVersion || entry.commit !== runtimeLock.sourceCommit || entry.releaseTag !== runtimeLock.releaseTag) throw new Error(`${platform} manifest identity differs from the runtime lock`);
  if (entry.sourceUrl !== expected.sourceUrl || entry.archiveSha256?.toLowerCase() !== expected.archiveSha256) {
    // macOS retains sourceSha256 for compatibility with its mixed-engine license schema.
    if (platform !== "macos" || entry.sourceUrl !== expected.sourceUrl || entry.sourceSha256?.toLowerCase() !== expected.archiveSha256) throw new Error(`${platform} manifest archive differs from the runtime lock`);
  }
}
if (manifests[1].systemEngines.includes("colmap")) throw new Error("Linux system COLMAP is forbidden");
for (const entry of entries.slice(0, 2)) {
  const cuda = entry.cudaCompatibility;
  if (cuda.toolkitVersion !== runtimeLock.cudaRelease || cuda.architecturePolicy !== runtimeLock.gpuArchitectures.join(";")
    || cuda.minimumComputeCapability !== buildLock.minimumComputeCapability
    || (cuda.minimumDriver ?? cuda.minimumWindowsDriver) !== buildLock.minimumDriver) throw new Error("Platform CUDA lock differs");
}
for (const [platform, expected] of Object.entries(runtimeLock.platforms)) {
  assertHashPin(expected.archiveSha256, `${platform} COLMAP runtime archive`);
  assertHashPin(expected.integritySha256, `${platform} COLMAP integrity inventory`);
  assertHashPin(expected.buildInfoSha256, `${platform} COLMAP BUILD-INFO`);
  if (!expected.sourceUrl.startsWith(`https://github.com/${runtimeLock.repository}/releases/download/${runtimeLock.releaseTag}/`)) throw new Error(`${platform} runtime URL is not pinned to the fork Release`);
}

console.log(`Verified source builder plus three-platform ${runtimeLock.releaseTag} runtime lock.`);
