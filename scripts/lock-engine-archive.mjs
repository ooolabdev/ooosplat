// Maintainer-only promotion step, after reviewing the independently built
// artifact. Normal setup never obtains its trust pin from a downloaded sidecar.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { workspace, fileHash, verifyColmap } from "./colmap-runtime.mjs";
const [platform, archiveArg] = process.argv.slice(2);
if (!["windows", "linux", "macos"].includes(platform) || !archiveArg) throw new Error("Usage: node scripts/lock-engine-archive.mjs windows|linux|macos ARCHIVE");
const archive = path.resolve(archiveArg);
const manifestPath = path.join(workspace, `engines/manifest${platform === "windows" ? "" : `.${platform}`}.json`);
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
const engine = platform === "windows" ? manifest.engines.find(e => e.name === "COLMAP") : platform === "linux" ? manifest.colmap : manifest.distribution;
if (path.basename(archive) !== (engine.install?.archiveName ?? engine.archiveName)) throw new Error("Archive filename differs from locked release");
const listing = spawnSync("tar", ["-tf", archive], { encoding: "utf8", windowsHide: true });
if (listing.status !== 0 || listing.stdout.split(/\r?\n/).some(p => /(^[/\\]|(^|[/\\])\.\.([/\\]|$)|:)/.test(p))) throw new Error("Unsafe archive");
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-engine-review-"));
try {
  const result = spawnSync("tar", ["-xf", archive, "-C", temporary], { stdio: "inherit", windowsHide: true });
  if (result.status !== 0) throw new Error("Extraction failed");
  const root = path.join(temporary, platform === "macos" ? "ooosplat-engines-macos-arm64" : `ooosplat-colmap-${platform}-x64`);
  const pin = fileHash(path.join(root, "SHA256SUMS"));
  verifyColmap(root, platform, pin, { run: false });
  engine.archiveSha256 = fileHash(archive);
  engine.buildStatus = "hash-locked";
  if (platform === "windows") manifest.requiredFiles.find(f => f.path === "engines/colmap/SHA256SUMS").sha256 = pin;
  else engine.integritySha256 = pin;
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");
  console.log(`Locked ${platform} archive ${engine.archiveSha256}. CLI, clean-host and GPU acceptance remain required.`);
} finally { fs.rmSync(temporary, { recursive: true, force: true }); }
