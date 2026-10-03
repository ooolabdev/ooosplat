import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { brushCommit, brushTag, brushVersion, brushLock, hostPlatform, requiredBrushFlags,
  sha256, fileSha256, prepareBrush, verifyBrushRuntime, verifyBrushCli, verifyExtractedBrush,
  extractBrushArchive, commitBrushRuntime, workspace } from "./brush-runtime.mjs";

function fixture(t, platform = "windows") {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), "OOOSplat 中文 Brush test "));
  t.after(() => fs.rmSync(base, { recursive: true, force: true }));
  const original = brushLock(platform);
  const source = path.join(base, "source"), cache = path.join(base, ".cache", "engines", "ooobrush");
  fs.mkdirSync(source, { recursive: true });
  fs.mkdirSync(cache, { recursive: true });
  const archiveBytes = "locked fake archive", binary = "locked fake headless binary";
  const contents = {
    [original.sourceBinary]: binary, [original.viewer]: "not shipped viewer",
    "BUILDINFO.json": JSON.stringify({ commit: brushCommit, target: original.target, versions: { "brush-cli": brushVersion } }),
    LICENSE: "Apache License\nVersion 2.0", "README.md": "Upstream CLI instructions",
  };
  for (const [file, bytes] of Object.entries(contents)) fs.writeFileSync(path.join(source, file), bytes);
  fs.writeFileSync(path.join(source, "SHA256SUMS"), Object.entries(contents).map(([file, bytes]) => `${sha256(bytes)}  ${file}`).join("\n") + "\n");
  const manifestPath = path.join(base, original.manifest);
  fs.mkdirSync(path.dirname(manifestPath), { recursive: true });
  const manifest = JSON.parse(fs.readFileSync(path.join(workspace, original.manifest), "utf8"));
  const entry = platform === "linux" ? manifest.brush : manifest.engines.find(engine => engine.name === "Brush");
  entry.archiveSha256 = sha256(archiveBytes);
  entry.binarySha256 = sha256(binary);
  entry.buildInfoSha256 = sha256(contents["BUILDINFO.json"]);
  entry.licenseSha256 = sha256(contents.LICENSE);
  fs.writeFileSync(manifestPath, JSON.stringify(manifest));
  const lock = brushLock(platform, base), archive = path.join(cache, lock.archiveName);
  fs.writeFileSync(archive, archiveBytes);
  const calls = [];
  const run = (command, args) => {
    calls.push([command, ...args]);
    if (args[0] === "--version") return brushVersion + "\n";
    if (args[0] === "--help") return requiredBrushFlags.join("\n");
    if (command === "ldd") return "libc.so.6 => /lib/libc.so.6";
    if (command === "otool") return `${args[1]}:\n /usr/lib/libSystem.B.dylib (compatibility version 1.0.0)`;
    if (command === "vtool") return "platform MACOS\n minos 15.0";
    if (command === "file") return "Mach-O 64-bit executable arm64";
    if (command === "curl") { fs.writeFileSync(args.at(-1), archiveBytes); return ""; }
    throw new Error(`Unexpected command ${command}: ${args}`);
  };
  const extract = (file, destination, requested) => {
    assert.equal(fileSha256(file), requested.archiveSha256.toLowerCase(), "unverified archive must not be extracted");
    for (const name of fs.readdirSync(source)) fs.copyFileSync(path.join(source, name), path.join(destination, name));
    verifyExtractedBrush(destination, requested);
  };
  const options = { base, run, extract };
  const runtime = path.join(base, lock.root);
  return { base, source, archive, cache, lock, calls, run, extract, options, runtime, contents };
}

test("all three manifests pin the fork, architecture and published archive digests", () => {
  const hashes = {
    windows: "2d02edd7bfe1f7a4b665b34b73c4dfb80a5464c543195c0380980754103b5301",
    linux: "eea89703d98b2fcc090309746ef4ef54271ef402c2b67e644ed7ac1390dcfb4e",
    macos: "5a47aabf185b9ca5697d760d30f9fbb9e6da7b199137bcc8c8fcf488f84c0869",
  };
  for (const [platform, digest] of Object.entries(hashes)) {
    const lock = brushLock(platform);
    assert.equal(lock.archiveSha256.toLowerCase(), digest);
    assert.equal(lock.commit, brushCommit);
    assert.equal(lock.releaseTag, brushTag);
    assert.match(lock.sourceBinary, /^brush-cli/);
  }
  assert.equal(hostPlatform("darwin", "arm64"), "macos");
  assert.throws(() => hostPlatform("darwin", "x64"), /no locked runtime/);
  assert.throws(() => hostPlatform("linux", "arm64"), /no locked runtime/);
});

for (const platform of ["windows", "linux", "macos"]) {
  test(`${platform}: first download installs only the CLI, licenses and provenance in Unicode paths`, t => {
    const f = fixture(t, platform);
    fs.unlinkSync(f.archive);
    if (platform === "macos") {
      fs.mkdirSync(path.join(f.runtime, "bin"), { recursive: true });
      fs.mkdirSync(path.join(f.runtime, "colmap", "lib"), { recursive: true });
      fs.writeFileSync(path.join(f.runtime, "bin", "ffmpeg"), "unchanged FFmpeg");
      fs.writeFileSync(path.join(f.runtime, "colmap", "lib", "test.dylib"), "unchanged COLMAP");
      fs.writeFileSync(path.join(f.runtime, "SHA256SUMS"), "existing mixed inventory");
    }
    assert.equal(prepareBrush(platform, f.options), f.runtime);
    assert.equal(f.calls.filter(([command]) => command === "curl").length, 1);
    assert.equal(fileSha256(path.join(f.runtime, f.lock.binary)), f.lock.binarySha256);
    const metadata = path.join(f.runtime, f.lock.metadata);
    assert.ok(fs.existsSync(path.join(metadata, "LICENSE")));
    assert.ok(fs.existsSync(path.join(metadata, "BUILDINFO.json")));
    assert.ok(fs.existsSync(path.join(metadata, "UPSTREAM-SHA256SUMS")));
    assert.ok(fs.existsSync(path.join(metadata, "UPSTREAM-README.md")));
    assert.equal(fs.existsSync(path.join(f.runtime, f.lock.viewer)), false);
    const info = JSON.parse(fs.readFileSync(path.join(metadata, "INSTALL-INFO.json")));
    assert.equal(info.sourceBinary, f.lock.sourceBinary);
    assert.equal(info.installedBinary, f.lock.binary);
    verifyBrushRuntime(f.runtime, f.lock, { run: f.run });
    if (platform === "macos") {
      assert.equal(fs.readFileSync(path.join(f.runtime, "bin", "ffmpeg"), "utf8"), "unchanged FFmpeg");
      assert.equal(fs.readFileSync(path.join(f.runtime, "colmap", "lib", "test.dylib"), "utf8"), "unchanged COLMAP");
      assert.equal(fs.readFileSync(path.join(f.runtime, "SHA256SUMS"), "utf8"), "existing mixed inventory");
    }
  });

  test(`${platform}: installed runtime works offline without archive/manifest writes`, t => {
    const f = fixture(t, platform);
    const manifestBefore = fs.readFileSync(path.join(f.base, f.lock.manifest));
    prepareBrush(platform, f.options);
    fs.unlinkSync(f.archive);
    prepareBrush(platform, { ...f.options, extract: () => { throw new Error("must not extract/download"); } });
    assert.equal(f.calls.some(([command]) => command === "curl"), false);
    assert.deepEqual(fs.readFileSync(path.join(f.base, f.lock.manifest)), manifestBefore);
  });
}

test("old runtime is replaced, tracked README preserved and viewer removed", t => {
  const f = fixture(t);
  fs.mkdirSync(f.runtime, { recursive: true });
  fs.writeFileSync(path.join(f.runtime, "brush_app.exe"), "old official Brush");
  fs.writeFileSync(path.join(f.runtime, "brush.exe"), "old unwanted viewer");
  fs.writeFileSync(path.join(f.runtime, "README.md"), "tracked OOOSplat placeholder");
  prepareBrush("windows", f.options);
  assert.equal(fs.readFileSync(path.join(f.runtime, "README.md"), "utf8"), "tracked OOOSplat placeholder");
  assert.equal(fs.existsSync(path.join(f.runtime, "brush.exe")), false);
  assert.equal(fileSha256(path.join(f.runtime, f.lock.binary)), f.lock.binarySha256);
});

test("corrupt archive or native CLI failure leaves the previous installation intact", t => {
  const f = fixture(t);
  fs.mkdirSync(f.runtime, { recursive: true });
  const previous = path.join(f.runtime, f.lock.binary);
  fs.writeFileSync(previous, "old binary");
  fs.writeFileSync(f.archive, "corrupt");
  assert.throws(() => prepareBrush("windows", f.options), /unverified archive/);
  assert.equal(fs.readFileSync(previous, "utf8"), "old binary");
  fs.writeFileSync(f.archive, "locked fake archive");
  assert.throws(() => prepareBrush("windows", { ...f.options, run: () => { throw new Error("native CLI timed out"); } }), /timed out/);
  assert.equal(fs.readFileSync(previous, "utf8"), "old binary");
  assert.equal(fs.existsSync(path.join(f.cache, "windows.install.lock")), false);
});

test("archive pin and safe paths are checked before any extraction", t => {
  const f = fixture(t);
  fs.writeFileSync(f.archive, "corrupt");
  assert.throws(() => extractBrushArchive(f.archive, f.source, f.lock, () => { throw new Error("must not call tar"); }), /archive SHA-256 mismatch/);
  fs.writeFileSync(f.archive, "locked fake archive");
  assert.throws(() => extractBrushArchive(f.archive, f.source, f.lock, () => "../escape\n"), /Unsafe or unexpected/);
  assert.throws(() => prepareBrush("windows", { ...f.options, destination: ".." }), /escapes workspace/);
  assert.throws(() => prepareBrush("windows", { ...f.options, destination: "." }), /escapes workspace/);
  assert.throws(() => prepareBrush("windows", { ...f.options, destination: "src" }), /only its managed runtime/);
});

test("Unix tar's root directory entry is accepted without allowing nested/traversal paths", t => {
  const f = fixture(t, "linux"), destination = path.join(f.base, "extracted");
  fs.mkdirSync(destination);
  const run = (_command, args) => {
    if (args[0] === "-tf") return "./\n" + fs.readdirSync(f.source).map(name => `./${name}`).join("\n");
    for (const name of fs.readdirSync(f.source)) fs.copyFileSync(path.join(f.source, name), path.join(destination, name));
    return "";
  };
  extractBrushArchive(f.archive, destination, f.lock, run);
  verifyExtractedBrush(destination, f.lock);
});

test("internal checksums, source build metadata and missing inventories fail closed", t => {
  const f = fixture(t);
  verifyExtractedBrush(f.source, f.lock);
  fs.appendFileSync(path.join(f.source, "BUILDINFO.json"), "tampered");
  assert.throws(() => verifyExtractedBrush(f.source, f.lock), /SHA-256 mismatch/);
  fs.writeFileSync(path.join(f.source, "BUILDINFO.json"), f.contents["BUILDINFO.json"]);
  fs.appendFileSync(path.join(f.source, "SHA256SUMS"), `\n${sha256("bad")}  ../bad\n`);
  assert.throws(() => verifyExtractedBrush(f.source, f.lock), /Invalid OOOBrush/);
});

test("version, missing flags and unresolved dynamic dependencies reject the CLI", t => {
  const f = fixture(t, "linux");
  assert.throws(() => verifyBrushCli("fake", f.lock, () => "brush_app 0.3.0"), /Expected brush-cli/);
  assert.throws(() => verifyBrushCli("fake", f.lock, (_cmd, args) => args[0] === "--version" ? brushVersion : "--total-steps"), /missing --total-train-iters/);
  assert.throws(() => verifyBrushCli("fake", f.lock, (cmd, args) => cmd === "ldd" ? "libbad => not found" : f.run(cmd, args)), /dependency missing/);
  const m = fixture(t, "macos");
  assert.throws(() => verifyBrushCli("fake", m.lock, (cmd, args) => cmd === "otool" ? "fake:\n /opt/homebrew/lib/private.dylib" : m.run(cmd, args)), /unbundled private/);
  assert.throws(() => verifyBrushCli("fake", m.lock, (cmd, args) => cmd === "vtool" ? "minos 15.1" : m.run(cmd, args)), /deployment target/);
});

test("tampered installed binary and metadata are rejected and recovered from the verified cache", t => {
  const f = fixture(t);
  prepareBrush("windows", f.options);
  fs.writeFileSync(path.join(f.runtime, f.lock.binary), "tampered");
  assert.throws(() => verifyBrushRuntime(f.runtime, f.lock, { run: f.run }), /CLI SHA-256 mismatch/);
  prepareBrush("windows", f.options);
  fs.writeFileSync(path.join(f.runtime, "BUILDINFO.json"), "{}");
  assert.throws(() => verifyBrushRuntime(f.runtime, f.lock, { run: f.run }), /BUILDINFO SHA-256 mismatch/);
  prepareBrush("windows", f.options);
  verifyBrushRuntime(f.runtime, f.lock, { run: f.run });
  fs.writeFileSync(path.join(f.runtime, f.lock.viewer), "viewer must not be shipped");
  assert.throws(() => verifyBrushRuntime(f.runtime, f.lock, { run: f.run }), /Unexpected Brush runtime/);
  prepareBrush("windows", f.options);
  assert.equal(fs.existsSync(path.join(f.runtime, f.lock.viewer)), false);
});

test("concurrent setup is rejected without removing another invocation's lock", t => {
  const f = fixture(t);
  const guard = path.join(f.cache, "windows.install.lock");
  fs.writeFileSync(guard, "another process owns this lock");
  assert.throws(() => prepareBrush("windows", f.options), /Another setup may be running/);
  assert.equal(fs.readFileSync(guard, "utf8"), "another process owns this lock");
});

test("failed directory replacement rolls back the old runtime", t => {
  const f = fixture(t);
  fs.mkdirSync(f.runtime, { recursive: true });
  fs.writeFileSync(path.join(f.runtime, f.lock.binary), "previous");
  let n = 0;
  assert.throws(() => prepareBrush("windows", { ...f.options, rename: (from, to) => {
    if (++n === 2) throw new Error("injected rename failure");
    fs.renameSync(from, to);
  } }), /injected rename failure/);
  assert.equal(fs.readFileSync(path.join(f.runtime, f.lock.binary), "utf8"), "previous");
});

test("macOS partial install rollback restores both CLI and metadata without touching shared files", t => {
  const f = fixture(t, "macos");
  const staged = path.join(f.base, "staged");
  for (const root of [staged, f.runtime]) {
    fs.mkdirSync(path.join(root, f.lock.metadata), { recursive: true });
    fs.mkdirSync(path.join(root, "bin"), { recursive: true });
    fs.writeFileSync(path.join(root, f.lock.metadata, "LICENSE"), root);
    fs.writeFileSync(path.join(root, f.lock.binary), root);
  }
  fs.writeFileSync(path.join(f.runtime, "bin", "ffprobe"), "keep");
  let n = 0;
  assert.throws(() => commitBrushRuntime(staged, f.runtime, f.lock, (from, to) => {
    if (++n === 4) throw new Error("injected macOS failure");
    fs.renameSync(from, to);
  }), /injected macOS failure/);
  assert.equal(fs.readFileSync(path.join(f.runtime, f.lock.binary), "utf8"), f.runtime);
  assert.equal(fs.readFileSync(path.join(f.runtime, f.lock.metadata, "LICENSE"), "utf8"), f.runtime);
  assert.equal(fs.readFileSync(path.join(f.runtime, "bin", "ffprobe"), "utf8"), "keep");
});

test("installer integration, CLI flags and bundle resources exclude the viewer", () => {
  const read = file => fs.readFileSync(path.join(workspace, file), "utf8");
  for (const file of ["scripts/setup-engines.ps1", "scripts/setup-engines-linux.sh", "scripts/build-engines-macos.sh"]) {
    assert.match(read(file), /brush-runtime\.mjs/);
    assert.doesNotMatch(read(file), /ArthurBrussee\/brush|brush-app-.*tar\.xz/);
  }
  const linux = JSON.parse(read("src-tauri/tauri.linux.conf.json")).bundle.resources;
  for (const file of ["brush_app", "LICENSE", "BUILDINFO.json", "INSTALL-INFO.json", "UPSTREAM-SHA256SUMS", "UPSTREAM-README.md"]) {
    assert.ok(linux[`../engines/linux/brush/${file}`]);
  }
  assert.equal(linux["../engines/linux/brush/brush"], undefined);
  const rust = read("src-tauri/src/engines/brush.rs");
  assert.doesNotMatch(rust, /OsString::from\("--total-steps"\)/);
  for (const flag of requiredBrushFlags) assert.ok(rust.includes(`"${flag}"`));
  assert.match(rust, /brush_cli=info,brush_process=info/);
  for (const file of ["scripts/verify-engines.ps1", "scripts/verify-engines-linux.sh", "scripts/verify-engines-macos.sh"]) {
    assert.doesNotMatch(read(file), /--total-steps/);
  }
});
