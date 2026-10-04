import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { archiveEntriesSafe, commitColmapRuntime } from "./setup-colmap-runtime.mjs";
import { assertHashPin, fileHash, filesUnder, isLockedColmapHelp, runtimeLock, verifyIntegrity, verifyColmap } from "./colmap-runtime.mjs";

function releaseFixture(t, platform = "linux") {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-colmap-release-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const write = (name, contents) => {
    const target = path.join(root, ...name.split("/"));
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, contents);
  };
  const binary = `bin/${platform === "windows" ? "colmap.exe" : "colmap"}`;
  const shared = platform === "windows" ? "bin/runtime.dll" : platform === "macos" ? "lib/runtime.dylib" : "lib/runtime.so.1";
  const vocabulary = "lib/colmap/vocab_tree_faiss_flickr100K_words256K.bin";
  const models = ["lib/validation/model/cameras.txt", "lib/validation/model/images.txt", "lib/validation/model/points3D.txt"];
  write(binary, "colmap");
  write(shared, "runtime");
  write(vocabulary, "vocabulary");
  for (const model of models) write(model, "model");
  write("licenses/COLMAP/0-COPYING.txt", "BSD-3-Clause");

  const lock = structuredClone(runtimeLock);
  lock.offlineVocabulary = { path: vocabulary, sha256: fileHash(path.join(root, ...vocabulary.split("/"))), bytes: 10 };
  lock.platforms[platform] = { ...lock.platforms[platform], buildInfoSha256: "0".repeat(64) };
  const expected = lock.platforms[platform];
  const info = {
    schemaVersion: 1,
    platform,
    architecture: expected.architecture,
    sourceCommit: lock.sourceCommit,
    scriptCommit: lock.scriptCommit,
    repository: lock.repository,
    runtimeRevision: lock.runtimeRevision,
    colmapVersion: lock.colmapVersion,
    features: {
      ...Object.fromEntries(lock.disabledFeatures.map(feature => [feature, false])),
      cpuSift: true,
      ceresCpuBA: true,
      offlineLoopDetection: true,
      vocabTreeMatching: true,
      CUDA_ENABLED: expected.cuda,
      CASPAR_ENABLED: expected.caspar,
      CASPAR_USE_DOUBLE: false,
    },
    cudaRelease: expected.cuda ? lock.cudaRelease : null,
    gpuArchitectures: expected.cuda ? [...lock.gpuArchitectures] : [],
    offlineVocabulary: { ...lock.offlineVocabulary },
  };
  const runtimeFiles = [binary, shared, vocabulary, ...models];
  const inventory = { schemaVersion: 1, components: [{ name: "COLMAP", licenseFiles: ["licenses/COLMAP/0-COPYING.txt"], runtimeFiles }] };
  const writeMetadata = () => {
    write("BUILD-INFO.json", `${JSON.stringify(info)}\n`);
    lock.platforms[platform].buildInfoSha256 = fileHash(path.join(root, "BUILD-INFO.json"));
    write("BUNDLED-COMPONENTS.json", `${JSON.stringify(inventory)}\n`);
  };
  const reseal = () => {
    writeMetadata();
    write("SHA256SUMS", `${filesUnder(root).filter(file => path.basename(file) !== "SHA256SUMS").map(file => `${fileHash(file)}  ${path.relative(root, file).split(path.sep).join("/")}`).join("\n")}\n`);
    return fileHash(path.join(root, "SHA256SUMS"));
  };
  return { root, lock, info, inventory, write, reseal };
}

test("published runtime lock pins the requested immutable asset identities", () => {
  assert.equal(runtimeLock.repository, "ooolabdev/ooosplat-colmap");
  assert.equal(runtimeLock.releaseTag, "colmap-4.2.1-runtime.1");
  assert.equal(runtimeLock.sourceCommit, "bd1fcf654d2dd8fefa1466999c190a246f83f4b9");
  const expected = {
    windows: "6ea29a37f5ace05cf4003dd97143a5fe9e9d01b315f52405d0e38b8970b95d00",
    linux: "99d3e586b554069ff8fc524430924886608505ba8337751783682fae9eadf88d",
    macos: "fd7ac276a2b858e10f2bddd7febfac139c0d6f18b07d84c869c0363eb814530e",
  };
  for (const [platform, hash] of Object.entries(expected)) {
    assert.equal(runtimeLock.platforms[platform].archiveSha256, hash);
    assertHashPin(runtimeLock.platforms[platform].integritySha256, `${platform} inventory`);
    assert.match(runtimeLock.platforms[platform].sourceUrl, /github\.com\/ooolabdev\/ooosplat-colmap\/releases\/download\/colmap-4\.2\.1-runtime\.1\//);
  }
});

test("COLMAP identity accepts the locked full or standard short commit only", () => {
  assert.equal(isLockedColmapHelp(`COLMAP ${runtimeLock.colmapVersion} (Commit ${runtimeLock.sourceCommit})`), true);
  assert.equal(isLockedColmapHelp("COLMAP 4.2.1 (Commit bd1fcf6 on 2026-09-29 with CUDA)"), true);
  assert.equal(isLockedColmapHelp("COLMAP 4.2.1 (Commit bd1fcf60 on 2026-09-29 with CUDA)"), false);
  assert.equal(isLockedColmapHelp("COLMAP 4.1.0 (Commit bd1fcf6)"), false);
});

test("new Release metadata validates for all three platform policies", t => {
  for (const platform of ["windows", "linux", "macos"]) {
    const fixture = releaseFixture(t, platform);
    assert.doesNotThrow(() => verifyColmap(fixture.root, platform, fixture.reseal(), { run: false, requireRelease: true, releaseLock: fixture.lock }));
  }
});

test("identity, feature, platform and vocabulary drift are rejected", t => {
  for (const mutate of [
    fixture => { fixture.info.sourceCommit = "0".repeat(40); },
    fixture => { fixture.info.features.ONNX_ENABLED = true; },
    fixture => { fixture.info.architecture = "arm64"; },
    fixture => { fixture.info.offlineVocabulary.sha256 = "0".repeat(64); },
  ]) {
    const fixture = releaseFixture(t);
    mutate(fixture);
    assert.throws(() => verifyColmap(fixture.root, "linux", fixture.reseal(), { run: false, requireRelease: true, releaseLock: fixture.lock }), /identity|feature|architecture|vocabulary/i);
  }
});

test("inventory requires every dependency, license, model and vocabulary", t => {
  const fixture = releaseFixture(t);
  fixture.inventory.components[0].runtimeFiles = fixture.inventory.components[0].runtimeFiles.filter(file => file !== "lib/runtime.so.1");
  assert.throws(() => verifyColmap(fixture.root, "linux", fixture.reseal(), { run: false, requireRelease: true, releaseLock: fixture.lock }), /license inventory/);
});

test("changed, unlisted and traversal files are rejected", t => {
  const fixture = releaseFixture(t), pin = fixture.reseal();
  fixture.write("bin/colmap", "tampered");
  assert.throws(() => verifyIntegrity(fixture.root, pin), /hash mismatch/);
  const extra = releaseFixture(t), extraPin = extra.reseal();
  extra.write("bin/surprise.dll", "extra");
  assert.throws(() => verifyIntegrity(extra.root, extraPin), /Unverified runtime file/);
  extra.write("SHA256SUMS", `${"0".repeat(64)}  ../secret\n`);
  assert.throws(() => verifyIntegrity(extra.root, fileHash(path.join(extra.root, "SHA256SUMS"))), /Unsafe integrity path/);
});

test("archive listing accepts direct roots and rejects traversal or absolute paths", () => {
  assert.equal(archiveEntriesSafe("bin/\nbin/colmap\nBUILD-INFO.json\n"), true);
  for (const listing of ["", "../escape\n", "/absolute\n", "C:/absolute\n", "bin/../../escape\n"]) assert.equal(archiveEntriesSafe(listing), false);
});

test("failed installation verification restores the previous runtime", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-colmap-commit-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const destination = path.join(root, "installed"), staged = path.join(root, "transaction", "runtime");
  fs.mkdirSync(destination, { recursive: true });
  fs.mkdirSync(staged, { recursive: true });
  fs.writeFileSync(path.join(destination, "identity"), "previous");
  fs.writeFileSync(path.join(staged, "identity"), "candidate");
  assert.throws(() => commitColmapRuntime(staged, destination, () => { throw new Error("candidate rejected"); }), /candidate rejected/);
  assert.equal(fs.readFileSync(path.join(destination, "identity"), "utf8"), "previous");
});
