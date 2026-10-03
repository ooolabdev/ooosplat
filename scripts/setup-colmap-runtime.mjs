import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { workspace, assertHashPin, fileHash, verifyColmap } from "./colmap-runtime.mjs";

const platform = process.argv[2];
if (!["windows", "linux"].includes(platform)) throw new Error("Expected windows or linux");
const manifest = JSON.parse(fs.readFileSync(path.join(workspace, `engines/manifest${platform === "linux" ? ".linux" : ""}.json`), "utf8"));
const engine = platform === "linux" ? manifest.colmap : manifest.engines.find(e => e.name === "COLMAP");
const integrityPin = platform === "linux" ? engine.integritySha256 : manifest.requiredFiles.find(f => f.path === "engines/colmap/SHA256SUMS")?.sha256;
assertHashPin(engine.archiveSha256, "COLMAP archive");
assertHashPin(integrityPin, "COLMAP SHA256SUMS");
const destination = path.resolve(workspace, engine.install?.destination ?? engine.destination);
if (!destination.startsWith(`${path.join(workspace, "engines")}${path.sep}`)) throw new Error("Unsafe runtime destination");
try {
  verifyColmap(destination, platform, integrityPin);
  console.log("Ready: locked COLMAP runtime");
  process.exit(0);
} catch { /* Install the reviewed archive, never use the old binary or PATH. */ }
const cache = path.join(workspace, ".cache", "engines", platform);
fs.mkdirSync(cache, { recursive: true });
const archive = path.join(cache, engine.install?.archiveName ?? engine.archiveName);
const local = process.env.OOOSPLAT_COLMAP_ENGINE_ARCHIVE;
if (local) fs.copyFileSync(local, archive);
else if (!fs.existsSync(archive) || fileHash(archive).toLowerCase() !== engine.archiveSha256.toLowerCase()) {
  const result = spawnSync(platform === "windows" ? "curl.exe" : "curl", ["--fail", "--location", "--retry", "3", engine.sourceUrl, "--output", archive], { stdio: "inherit", windowsHide: true });
  if (result.error || result.status !== 0) throw new Error("COLMAP download failed");
}
if (fileHash(archive).toLowerCase() !== engine.archiveSha256.toLowerCase()) throw new Error("COLMAP archive SHA-256 mismatch");
const listing = spawnSync("tar", ["-tf", archive], { encoding: "utf8", windowsHide: true });
if (listing.error || listing.status !== 0 || listing.stdout.split(/\r?\n/).some(p => /(^[/\\]|(^|[/\\])\.\.([/\\]|$)|:)/.test(p))) throw new Error("Unsafe archive layout");
const temporary = fs.mkdtempSync(path.join(cache, "install-"));
try {
  const extraction = spawnSync("tar", ["-xf", archive, "-C", temporary], { stdio: "inherit", windowsHide: true });
  if (extraction.error || extraction.status !== 0) throw new Error("COLMAP extraction failed");
  const staged = path.join(temporary, `ooosplat-colmap-${platform}-x64`);
  verifyColmap(staged, platform, integrityPin);
  if (fs.existsSync(path.join(destination, "README.md"))) fs.copyFileSync(path.join(destination, "README.md"), path.join(staged, "README.md"));
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  const backup = path.join(temporary, "previous-runtime");
  if (fs.existsSync(destination)) fs.renameSync(destination, backup);
  try {
    fs.renameSync(staged, destination);
    verifyColmap(destination, platform, integrityPin);
  } catch (error) {
    if (fs.existsSync(destination)) fs.renameSync(destination, path.join(temporary, "failed-runtime"));
    if (fs.existsSync(backup)) fs.renameSync(backup, destination);
    throw error;
  }
  console.log(`Installed verified COLMAP into ${destination}`);
} finally { fs.rmSync(temporary, { recursive: true, force: true }); }
