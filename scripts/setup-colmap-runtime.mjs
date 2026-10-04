import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { workspace, assertHashPin, fileHash, runtimeLock, verifyColmap } from "./colmap-runtime.mjs";

export function archiveEntriesSafe(listing) {
  const entries = listing.split(/\r?\n/).filter(Boolean);
  return entries.length > 0 && entries.every(entry => {
    const normalized = entry.replace(/\\/g, "/").replace(/\/$/, "");
    return normalized && !normalized.startsWith("/") && !/^[a-z]:/i.test(normalized)
      && !normalized.split("/").some(part => !part || part === "." || part === "..");
  });
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { windowsHide: true, ...options });
  if (result.error || result.status !== 0) throw new Error(`${command} failed: ${result.error?.message ?? result.stderr ?? result.status}`);
  return result;
}

function safeDestination(relative) {
  const root = path.resolve(workspace, "engines");
  const destination = path.resolve(workspace, relative);
  if (!destination.startsWith(`${root}${path.sep}`)) throw new Error("Unsafe runtime destination");
  return destination;
}

export function commitColmapRuntime(staged, destination, verify, rename = fs.renameSync) {
  const temporary = path.dirname(staged);
  const backup = path.join(temporary, "previous-runtime");
  const failed = path.join(temporary, "failed-runtime");
  let installed = false;
  if (fs.existsSync(destination)) rename(destination, backup);
  try {
    rename(staged, destination);
    installed = true;
    verify(destination);
  } catch (error) {
    if (installed && fs.existsSync(destination)) rename(destination, failed);
    if (fs.existsSync(backup)) rename(backup, destination);
    throw error;
  }
}

export function installColmapRuntime(platform, { localArchive = process.env.OOOSPLAT_COLMAP_ENGINE_ARCHIVE } = {}) {
  const expected = runtimeLock.platforms[platform];
  if (!expected) throw new Error("Expected windows, linux or macos");
  for (const [label, hash] of [["COLMAP archive", expected.archiveSha256], ["COLMAP SHA256SUMS", expected.integritySha256], ["COLMAP BUILD-INFO", expected.buildInfoSha256]]) assertHashPin(hash, label);
  const destination = safeDestination(expected.destination);
  try {
    verifyColmap(destination, platform, expected.integritySha256, { requireRelease: true });
    console.log(`Ready: locked COLMAP ${runtimeLock.releaseTag} runtime`);
    return destination;
  } catch { /* Install the reviewed archive; never use PATH or a stale runtime. */ }

  const cache = path.join(workspace, ".cache", "engines", "colmap", platform);
  fs.mkdirSync(cache, { recursive: true });
  const archive = path.join(cache, expected.archiveName);
  if (localArchive) {
    const source = path.resolve(localArchive);
    if (source !== path.resolve(archive)) fs.copyFileSync(source, archive);
  } else if (!fs.existsSync(archive) || fileHash(archive).toLowerCase() !== expected.archiveSha256) {
    const download = `${archive}.download`;
    try {
      run(process.platform === "win32" ? "curl.exe" : "curl", ["--fail", "--location", "--retry", "3", "--connect-timeout", "20", expected.sourceUrl, "--output", download], { stdio: "inherit", timeout: 20 * 60_000 });
      if (fileHash(download).toLowerCase() !== expected.archiveSha256) throw new Error("COLMAP archive SHA-256 mismatch");
      fs.rmSync(archive, { force: true });
      fs.renameSync(download, archive);
    } finally {
      if (fs.existsSync(download)) fs.rmSync(download, { force: true });
    }
  }
  if (fileHash(archive).toLowerCase() !== expected.archiveSha256) throw new Error("COLMAP archive SHA-256 mismatch");
  const listing = run("tar", ["-tf", archive], { encoding: "utf8", timeout: 2 * 60_000 }).stdout;
  if (!archiveEntriesSafe(listing)) throw new Error("Unsafe or empty COLMAP archive layout");

  const temporary = fs.mkdtempSync(path.join(cache, "install-"));
  const staged = path.join(temporary, "runtime");
  try {
    fs.mkdirSync(staged);
    run("tar", ["-xf", archive, "-C", staged], { stdio: "inherit", timeout: 10 * 60_000 });
    verifyColmap(staged, platform, expected.integritySha256, { requireRelease: true });
    const readme = path.join(destination, "README.md");
    if (fs.existsSync(readme)) fs.copyFileSync(readme, path.join(staged, "README.md"));
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    commitColmapRuntime(staged, destination, installedRoot => verifyColmap(installedRoot, platform, expected.integritySha256, { requireRelease: true }));
    console.log(`Installed verified ${runtimeLock.releaseTag} into ${expected.destination}`);
    return destination;
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  installColmapRuntime(process.argv[2]);
}
