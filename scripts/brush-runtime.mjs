import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const brushCommit = "866a7d65b4d6a591ba0d52a88f8b20ea3465c1e5";
export const brushTag = "ooo-v1.0.0";
export const brushVersion = "brush-cli 1.0.0";
export const requiredBrushFlags = ["--total-train-iters", "--max-resolution", "--refine-every", "--max-splats",
  "--growth-grad-threshold", "--growth-select-fraction", "--growth-stop-iter", "--export-every", "--export-path", "--export-name"];
const metadataFiles = ["LICENSE", "BUILDINFO.json", "UPSTREAM-SHA256SUMS", "UPSTREAM-README.md", "INSTALL-INFO.json"];
const platforms = {
  windows: { host: "win32", arch: "x64", target: "x86_64-pc-windows-msvc", sourceBinary: "brush-cli.exe", viewer: "brush.exe",
    root: "engines/brush", binary: "brush_app.exe", metadata: ".", manifest: "engines/manifest.json" },
  linux: { host: "linux", arch: "x64", target: "x86_64-unknown-linux-gnu", sourceBinary: "brush-cli", viewer: "brush",
    root: "engines/linux/brush", binary: "brush_app", metadata: ".", manifest: "engines/manifest.linux.json" },
  macos: { host: "darwin", arch: "arm64", target: "aarch64-apple-darwin", sourceBinary: "brush-cli", viewer: "brush",
    root: "engines/macos/arm64", binary: "bin/brush_app", metadata: "licenses/OOOBrush", manifest: "engines/manifest.macos.json" },
};

export const sha256 = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
export const fileSha256 = file => sha256(fs.readFileSync(file));
export function hostPlatform(platform = process.platform, arch = process.arch) {
  const key = Object.keys(platforms).find(key => platforms[key].host === platform && platforms[key].arch === arch);
  if (!key) throw new Error(`OOOBrush has no locked runtime for ${platform}/${arch}`);
  return key;
}
export function brushLock(platform, base = workspace) {
  const details = platforms[platform];
  if (!details) throw new Error(`Unsupported Brush platform: ${platform}`);
  const manifest = JSON.parse(fs.readFileSync(path.join(base, details.manifest), "utf8"));
  const entry = platform === "linux" ? manifest.brush : manifest.engines.find(engine => engine.name === "Brush");
  const lock = { ...details, ...entry, archiveSha256: entry.archiveSha256 ?? entry.sourceSha256 };
  if (lock.version !== "1.0.0" || lock.releaseTag !== brushTag || lock.commit !== brushCommit || lock.target !== details.target) {
    throw new Error(`Invalid pinned OOOBrush identity in ${details.manifest}`);
  }
  for (const key of ["archiveSha256", "binarySha256", "buildInfoSha256", "licenseSha256"]) {
    if (!/^[a-f0-9]{64}$/i.test(lock[key] ?? "")) throw new Error(`Missing Brush ${key} in ${details.manifest}`);
  }
  const expectedArchive = `OOOBrush-${details.target}.${platform === "windows" ? "zip" : "tar.gz"}`;
  if (lock.archiveName !== expectedArchive || lock.sourceUrl !== `https://github.com/ooolabdev/OOOBrush/releases/download/${brushTag}/${expectedArchive}`) {
    throw new Error(`OOOBrush source must be the locked Release asset: ${details.manifest}`);
  }
  return lock;
}

export function runCommand(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", windowsHide: true, timeout: 30_000, maxBuffer: 8 * 1024 * 1024, ...options });
  if (result.error || result.status !== 0) throw new Error(`${command} failed: ${result.error?.message ?? result.stderr ?? result.status}`);
  return result.stdout ?? "";
}
function assertHash(file, expected, description) {
  if (!fs.lstatSync(file).isFile() || fileSha256(file) !== expected.toLowerCase()) throw new Error(`${description} SHA-256 mismatch: ${file}`);
}
function checkBuildInfo(file, lock) {
  assertHash(file, lock.buildInfoSha256, "Brush BUILDINFO");
  const info = JSON.parse(fs.readFileSync(file, "utf8"));
  if (info.commit !== brushCommit || info.target !== lock.target || info.versions?.["brush-cli"] !== brushVersion) {
    throw new Error("Brush build commit, architecture or CLI version mismatch");
  }
}
export function verifyBrushCli(binary, lock, run = runCommand) {
  const version = run(binary, ["--version"]).trim();
  if (version !== brushVersion) throw new Error(`Expected ${brushVersion}, got ${version}`);
  const help = run(binary, ["--help"]);
  for (const flag of requiredBrushFlags) {
    if (!new RegExp(`${flag}(?=[\\s=,]|$)`).test(help)) throw new Error(`OOOBrush CLI is missing ${flag}`);
  }
  // Release binaries are self-contained apart from platform/driver libraries.
  // Unexpected private runtime dependencies must block installation, not disappear.
  if (lock.host === "linux") {
    const dependencies = run("ldd", [binary]);
    if (/not found/.test(dependencies)) throw new Error(`OOOBrush runtime dependency missing:\n${dependencies}`);
  } else if (lock.host === "darwin") {
    const dependencies = run("otool", ["-L", binary]).split("\n").slice(1).filter(line => line.trim());
    if (dependencies.some(line => !/^\s*(\/System\/Library\/|\/usr\/lib\/)/.test(line))) {
      throw new Error(`OOOBrush requires unbundled private libraries:\n${dependencies.join("\n")}`);
    }
    const build = run("vtool", ["-show-build", binary]);
    const minos = build.match(/\bminos\s+(\d+(?:\.\d+)*)/);
    if (!minos || Number(minos[1].split(".")[0]) > 15 || (Number(minos[1].split(".")[0]) === 15 && Number(minos[1].split(".")[1] ?? 0) > 0)) {
      throw new Error(`OOOBrush deployment target is incompatible with macOS 15.0:\n${build}`);
    }
    if (!/arm64/.test(run("file", [binary]))) throw new Error("OOOBrush is not Apple arm64");
  }
}

export function verifyBrushRuntime(root, lock, { run = runCommand, transformed = false } = {}) {
  const metadata = path.join(root, lock.metadata), binary = path.join(root, lock.binary);
  if (transformed && lock.host !== "darwin") throw new Error("Only a verified macOS mixed bundle may contain a re-signed Brush");
  if (!transformed) assertHash(binary, lock.binarySha256, "Brush CLI");
  if (lock.metadata === ".") {
    const allowed = [path.basename(lock.binary), ...metadataFiles, "README.md"];
    if (fs.readdirSync(root).some(file => !allowed.includes(file))) throw new Error("Unexpected Brush runtime files; viewer/development files must not be bundled");
  }
  checkBuildInfo(path.join(metadata, "BUILDINFO.json"), lock);
  assertHash(path.join(metadata, "LICENSE"), lock.licenseSha256, "Brush license");
  for (const relative of metadataFiles) {
    if (!fs.lstatSync(path.join(metadata, relative)).isFile()) throw new Error(`Missing Brush metadata: ${relative}`);
  }
  const installed = JSON.parse(fs.readFileSync(path.join(metadata, "INSTALL-INFO.json"), "utf8"));
  if (installed.commit !== brushCommit || installed.sourceArchiveSha256 !== lock.archiveSha256.toLowerCase()
      || installed.target !== lock.target || installed.sourceBinary !== lock.sourceBinary || installed.installedBinary !== lock.binary) {
    throw new Error("Brush installation provenance mismatch");
  }
  verifyBrushCli(binary, lock, run);
}

export function verifyExtractedBrush(directory, lock) {
  const expected = [lock.sourceBinary, lock.viewer, "BUILDINFO.json", "LICENSE", "README.md", "SHA256SUMS"].sort();
  const names = fs.readdirSync(directory).sort();
  if (JSON.stringify(names) !== JSON.stringify(expected)) throw new Error("Unexpected OOOBrush archive contents (review required)");
  const checksums = new Map();
  for (const line of fs.readFileSync(path.join(directory, "SHA256SUMS"), "utf8").trim().split(/\r?\n/)) {
    const match = line.match(/^([a-f0-9]{64})\s+\*?([^/\\]+)$/i);
    if (!match || checksums.has(match[2]) || !expected.includes(match[2]) || match[2] === "SHA256SUMS") throw new Error("Invalid OOOBrush internal checksum inventory");
    checksums.set(match[2], match[1]);
  }
  for (const file of expected.filter(name => name !== "SHA256SUMS")) {
    if (!checksums.has(file)) throw new Error(`OOOBrush checksum missing ${file}`);
    assertHash(path.join(directory, file), checksums.get(file), "Brush internal file");
  }
  assertHash(path.join(directory, lock.sourceBinary), lock.binarySha256, "Brush CLI");
  checkBuildInfo(path.join(directory, "BUILDINFO.json"), lock);
  assertHash(path.join(directory, "LICENSE"), lock.licenseSha256, "Brush license");
}

export function extractBrushArchive(archive, destination, lock, run = runCommand) {
  assertHash(archive, lock.archiveSha256, "Brush archive");
  // List before extraction. The hash-locked releases contain only six root files.
  const listed = run("tar", ["-tf", archive]).trim().split(/\r?\n/)
    .filter(name => name !== "." && name !== "./")
    .map(name => name.replace(/^\.\//, ""));
  const expected = [lock.sourceBinary, lock.viewer, "BUILDINFO.json", "LICENSE", "README.md", "SHA256SUMS"].sort();
  if (JSON.stringify(listed.sort()) !== JSON.stringify(expected)) throw new Error("Unsafe or unexpected OOOBrush archive paths");
  run("tar", ["-xf", archive, "-C", destination]);
  verifyExtractedBrush(destination, lock);
}

function isInside(base, target) {
  const relative = path.relative(base, target);
  return relative !== "" && relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
}
function assertSafeTarget(base, target) {
  if (!isInside(base, target)) throw new Error(`Brush destination escapes workspace: ${target}`);
  let candidate = target;
  while (candidate !== base) {
    if (fs.existsSync(candidate) && fs.lstatSync(candidate).isSymbolicLink()) throw new Error(`Brush destination contains a symlink: ${candidate}`);
    candidate = path.dirname(candidate);
  }
}

// Replace only the managed Brush files, with a rollback if any rename fails.
// macOS bin/ and licenses/ are shared with COLMAP/FFmpeg and are never replaced.
export function commitBrushRuntime(staged, destination, lock, rename = fs.renameSync) {
  const relatives = lock.host === "darwin" ? [lock.metadata, lock.binary] : ["."];
  const backupRoot = fs.mkdtempSync(path.join(path.dirname(destination), ".brush-backup-"));
  const operations = [];
  let preserveBackup = false;
  try {
    for (const [index, relative] of relatives.entries()) {
      const from = path.resolve(staged, relative), to = path.resolve(destination, relative), backup = path.join(backupRoot, String(index));
      fs.mkdirSync(path.dirname(to), { recursive: true });
      const operation = { to, backup, hadPrevious: fs.existsSync(to), installed: false };
      operations.push(operation);
      if (operation.hadPrevious) rename(to, backup);
      rename(from, to);
      operation.installed = true;
    }
  } catch (error) {
    try {
      for (const operation of operations.reverse()) {
        if (operation.installed) fs.rmSync(operation.to, { recursive: true, force: true });
        if (fs.existsSync(operation.backup)) fs.renameSync(operation.backup, operation.to);
      }
    } catch (rollbackError) {
      preserveBackup = true;
      throw new Error(`Brush install failed (${error.message}) and rollback failed (${rollbackError.message}); recover previous files from ${backupRoot}`);
    }
    throw error;
  } finally {
    if (!preserveBackup) fs.rmSync(backupRoot, { recursive: true, force: true });
  }
}

export function prepareBrush(platform, { base = workspace, destination, cacheDirectory, force = false,
  run = runCommand, extract = extractBrushArchive, rename = fs.renameSync } = {}) {
  const lock = brushLock(platform, base);
  const runtime = path.resolve(base, destination ?? lock.root);
  assertSafeTarget(base, runtime);
  if (runtime !== path.resolve(base, lock.root) && !isInside(path.join(base, ".cache"), runtime)) {
    throw new Error("Brush may replace only its managed runtime or a workspace .cache staging directory");
  }
  for (const relative of [lock.binary, lock.metadata]) assertSafeTarget(base, path.resolve(runtime, relative));
  const cache = path.resolve(base, cacheDirectory ?? ".cache/engines/ooobrush");
  assertSafeTarget(base, cache);
  fs.mkdirSync(path.dirname(runtime), { recursive: true });
  fs.mkdirSync(cache, { recursive: true });
  // A cache-local lock excludes parallel setup/local invocations on this host.
  const guard = path.join(cache, `${platform}.install.lock`);
  let descriptor;
  try { descriptor = fs.openSync(guard, "wx"); }
  catch (error) { throw new Error(`Cannot acquire Brush setup lock ${guard}: ${error.message}. Another setup may be running; do not remove a live lock.`); }
  let temporary;
  try {
    temporary = fs.mkdtempSync(path.join(cache, `${platform}-stage-`));
    if (!force) {
      try { verifyBrushRuntime(runtime, lock, { run }); return runtime; }
      catch { /* Missing/old/tampered installation: prepare a verified replacement. */ }
    }
    const archive = path.join(cache, lock.archiveName);
    if (!fs.existsSync(archive)) {
      const download = path.join(temporary, lock.archiveName);
      run("curl", ["--fail", "--location", "--retry", "3", "--connect-timeout", "20", lock.sourceUrl, "--output", download], { timeout: 15 * 60_000 });
      assertHash(download, lock.archiveSha256, "Brush download");
      fs.renameSync(download, archive);
    }
    const extracted = path.join(temporary, "source"), staged = path.join(temporary, "runtime");
    fs.mkdirSync(extracted);
    extract(archive, extracted, lock, run);
    const metadata = path.join(staged, lock.metadata), binary = path.join(staged, lock.binary);
    fs.mkdirSync(metadata, { recursive: true });
    fs.mkdirSync(path.dirname(binary), { recursive: true });
    fs.copyFileSync(path.join(extracted, lock.sourceBinary), binary);
    if (lock.host !== "win32") fs.chmodSync(binary, 0o755);
    for (const [source, target] of [["LICENSE", "LICENSE"], ["BUILDINFO.json", "BUILDINFO.json"],
      ["SHA256SUMS", "UPSTREAM-SHA256SUMS"], ["README.md", "UPSTREAM-README.md"]]) {
      fs.copyFileSync(path.join(extracted, source), path.join(metadata, target));
    }
    fs.writeFileSync(path.join(metadata, "INSTALL-INFO.json"), JSON.stringify({ schemaVersion: 1, name: "OOOBrush", releaseTag: brushTag,
      version: "1.0.0", commit: brushCommit, target: lock.target, sourceUrl: lock.sourceUrl, sourceArchiveSha256: lock.archiveSha256.toLowerCase(),
      sourceBinary: lock.sourceBinary, installedBinary: lock.binary, binarySha256: lock.binarySha256.toLowerCase(),
      note: "Headless brush-cli installed under the compatible OOOSplat name; viewer excluded. GPU training acceptance is separate." }, null, 2) + "\n");
    if (lock.metadata === "." && fs.existsSync(path.join(runtime, "README.md"))) fs.copyFileSync(path.join(runtime, "README.md"), path.join(staged, "README.md"));
    verifyBrushRuntime(staged, lock, { run });
    commitBrushRuntime(staged, runtime, lock, rename);
    return runtime;
  } finally {
    try {
      if (temporary) fs.rmSync(temporary, { recursive: true, force: true });
    } finally {
      fs.closeSync(descriptor);
      fs.unlinkSync(guard);
    }
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [action, ...args] = process.argv.slice(2);
    const platform = args[0] && !args[0].startsWith("--") ? args.shift() : hostPlatform();
    if (platform !== hostPlatform()) throw new Error("Native Brush validation must run on the matching platform/architecture");
    const lock = brushLock(platform);
    if (action === "prepare") {
      const options = {};
      for (let i = 0; i < args.length; i++) {
        if (args[i] === "--force") options.force = true;
        else if (["--destination", "--cache"].includes(args[i]) && args[i + 1]) {
          const key = args[i] === "--cache" ? "cacheDirectory" : "destination";
          options[key] = args[++i];
        }
        else throw new Error(`Unexpected Brush argument: ${args[i]}`);
      }
      console.log(`Ready: OOOBrush ${brushTag} (${lock.target}) -> ${prepareBrush(platform, options)}`);
    } else if (action === "verify" || action === "verify-macos-bundle") {
      if (args.length > 1) throw new Error("Unexpected Brush verification arguments");
      const transformed = action === "verify-macos-bundle";
      if (transformed && platform !== "macos") throw new Error("Only macOS has a transformed mixed runtime");
      const root = path.resolve(args[0] ?? path.join(workspace, lock.root));
      if (transformed) {
        // Authenticate the containing bundle before permitting re-signed bytes.
        // Build verification is constrained to the existing explicit local-archive mode.
        const manifest = JSON.parse(fs.readFileSync(path.join(workspace, lock.manifest), "utf8"));
        const expected = process.env.OOOSPLAT_ENGINE_BUILD_VERIFY === "1" && process.env.OOOSPLAT_MACOS_ENGINE_ARCHIVE
          ? fileSha256(path.join(root, "SHA256SUMS")) : manifest.distribution.integritySha256;
        assertHash(path.join(root, "SHA256SUMS"), expected ?? "", "macOS bundle inventory");
        runCommand("shasum", ["-a", "256", "-c", "SHA256SUMS"], { cwd: root });
        const entries = fs.readFileSync(path.join(root, "SHA256SUMS"), "utf8").split(/\r?\n/).map(line => line.replace(/^[a-f0-9]{64}\s+\*?/i, ""));
        for (const relative of [lock.binary, ...metadataFiles.map(name => `${lock.metadata}/${name}`)]) {
          if (!entries.includes(relative)) throw new Error(`macOS bundle does not authenticate ${relative}`);
        }
      }
      verifyBrushRuntime(root, lock, { transformed });
      console.log(`Verified OOOBrush ${brushTag} (${lock.target}) CLI, provenance, license and dependencies.`);
    } else throw new Error("Usage: node scripts/brush-runtime.mjs prepare|verify|verify-macos-bundle [windows|linux|macos] [options]");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
