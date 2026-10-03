import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { assertHashPin, buildLock, fileHash, filesUnder, verifyIntegrity, verifyColmap } from "./colmap-runtime.mjs";

function fixture(t, platform = "linux") {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-engine-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const write = (name, contents) => { const target = path.join(root, name); fs.mkdirSync(path.dirname(target), { recursive: true }); fs.writeFileSync(target, contents); };
  write(`bin/${platform === "windows" ? "colmap.exe" : "colmap"}`, "test executable");
  write("licenses/COLMAP-LICENSE.txt", "test BSD notice");
  const info = { platform, colmap: { ...buildLock }, colmapFeatures: {
    ...Object.fromEntries(buildLock.disabledFeatures.map(f => [f, false])),
    CUDA: platform !== "macos", CASPAR: platform !== "macos", CERES: true, CASPAR_USE_DOUBLE: false,
  }, cudaVersion: buildLock.cudaVersion, cudaArchitectures: buildLock.cudaArchitectures };
  write("BUILD-INFO.json", JSON.stringify(info));
  write("BUNDLED-COMPONENTS.json", JSON.stringify({ components: [{ name: "test", files: [], licenseFiles: ["COLMAP-LICENSE.txt"] }], sourceLicenseFiles: ["COLMAP-LICENSE.txt"] }));
  const reseal = () => {
    write("SHA256SUMS", filesUnder(root).filter(f => path.basename(f) !== "SHA256SUMS").map(f => `${fileHash(f)}  ${path.relative(root, f).split(path.sep).join("/")}`).join("\n") + "\n");
    return fileHash(path.join(root, "SHA256SUMS"));
  };
  return { root, info, write, reseal };
}
test("pending/null/sidecar placeholders are never accepted as trust pins", () => {
  for (const pin of [null, undefined, "pending", "", "00"]) assert.throws(() => assertHashPin(pin, "archive"), /not hash-locked/);
  assert.doesNotThrow(() => assertHashPin("A".repeat(64), "archive"));
});
test("same commit and policy validates for all three platform packages", t => {
  for (const platform of ["windows", "linux", "macos"]) {
    const f = fixture(t, platform);
    verifyColmap(f.root, platform, f.reseal(), { run: false });
  }
});
test("changed executable and changed checksum inventory are rejected", t => {
  const f = fixture(t), pin = f.reseal();
  f.write("bin/colmap", "tampered");
  assert.throws(() => verifyIntegrity(f.root, pin), /hash mismatch/);
  f.reseal();
  assert.throws(() => verifyIntegrity(f.root, pin), /reviewed manifest/);
});
test("unlisted files cannot piggyback on a valid checksum inventory", t => {
  const f = fixture(t), pin = f.reseal();
  f.write("bin/surprise.dll", "unused");
  assert.throws(() => verifyIntegrity(f.root, pin), /Unverified runtime file/);
});
test("wrong source commit, feature flags or architecture set fail validation", t => {
  const f = fixture(t);
  f.info.colmap.commit = "0".repeat(40);
  f.write("BUILD-INFO.json", JSON.stringify(f.info));
  assert.throws(() => verifyColmap(f.root, "linux", f.reseal(), { run: false }), /source identity/);
  f.info.colmap.commit = buildLock.commit;
  f.info.colmapFeatures.ONNX = true;
  f.write("BUILD-INFO.json", JSON.stringify(f.info));
  assert.throws(() => verifyColmap(f.root, "linux", f.reseal(), { run: false }), /feature policy/);
  f.info.colmapFeatures.ONNX = false;
  f.info.cudaArchitectures = [86];
  f.write("BUILD-INFO.json", JSON.stringify(f.info));
  assert.throws(() => verifyColmap(f.root, "linux", f.reseal(), { run: false }), /CUDA build lock/);
});
test("unlicensed shared library is retained but blocks packaging", t => {
  const f = fixture(t);
  f.write("lib/libunknown.so.1", "library");
  assert.throws(() => verifyColmap(f.root, "linux", f.reseal(), { run: false }), /Missing dependency license inventory/);
  assert.ok(fs.existsSync(path.join(f.root, "lib/libunknown.so.1")));
});
test("test binaries and vocabulary resources are forbidden even when hashed", t => {
  for (const name of ["bin/sift_test.exe", "bin/colmap.pdb", "bin/vocab_tree_faiss_flickr100K_words256K.bin"]) {
    const f = fixture(t);
    f.write(name, "development artifact");
    assert.throws(() => verifyColmap(f.root, "linux", f.reseal(), { run: false }), /Development\/unused/);
  }
});
test("traversal entries cannot escape the runtime", t => {
  const f = fixture(t);
  f.write("SHA256SUMS", `${"0".repeat(64)}  ../secret\n`);
  assert.throws(() => verifyIntegrity(f.root, fileHash(path.join(f.root, "SHA256SUMS"))), /Unsafe integrity path/);
});
