import { createHash } from "node:crypto";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const workspace = join(dirname(fileURLToPath(import.meta.url)), "..");

function workspacePath(relativePath) {
  return join(workspace, ...relativePath.split("/"));
}

function readText(relativePath) {
  return readFileSync(workspacePath(relativePath), "utf8");
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function assertContains(text, expected, context) {
  assert(text.includes(expected), `${context} is missing '${expected}'.`);
}

function normalizedSha256(text) {
  const normalized = text.replace(/\r\n?/g, "\n").replace(/\n+$/g, "");
  return createHash("sha256").update(normalized, "utf8").digest("hex").toUpperCase();
}

const requiredFiles = [
  "LICENSE",
  "NOTICE",
  "TRADEMARK_POLICY.md",
  "GENERATED_OUTPUTS.md",
  "licenses/THIRD_PARTY_NOTICES.txt",
  "licenses/FFmpeg-LGPL-2.1.txt",
  "licenses/COLMAP-LICENSE.txt",
  "licenses/NVIDIA-CUDA-Runtime.txt",
  "licenses/Brush-LICENSE.txt",
  "engines/manifest.json",
  "engines/manifest.linux.json",
];

for (const relativePath of requiredFiles) {
  const path = workspacePath(relativePath);
  assert(existsSync(path) && statSync(path).isFile(), `Required license file is missing: ${relativePath}`);
}

const expectedApacheSha256 = "58D1E17FFE5109A7AE296CAAFCADFDBE6A7D176F0BC4AB01E12A689B0499D8BD";
assert(
  normalizedSha256(readText("LICENSE")) === expectedApacheSha256,
  "LICENSE is not the unmodified Apache License 2.0 text.",
);

const packageJson = JSON.parse(readText("package.json"));
assert(packageJson.license === "Apache-2.0", "package.json must declare Apache-2.0.");
assert(packageJson.author === "ooolabdev", "package.json must identify ooolabdev as author.");
assert(
  packageJson.repository?.url === "git+https://github.com/ooolabdev/ooosplat.git",
  "package.json repository URL is incorrect.",
);

const cargo = readText("src-tauri/Cargo.toml");
assert(/^license\s*=\s*"Apache-2\.0"\s*$/m.test(cargo), "Cargo.toml must declare Apache-2.0.");
assert(/^authors\s*=\s*\["ooolabdev"\]\s*$/m.test(cargo), "Cargo.toml must identify ooolabdev as author.");
assert(
  /^repository\s*=\s*"https:\/\/github\.com\/ooolabdev\/ooosplat"\s*$/m.test(cargo),
  "Cargo.toml repository URL is incorrect.",
);

const tauri = JSON.parse(readText("src-tauri/tauri.conf.json"));
assert(tauri.bundle?.license === "Apache-2.0", "Tauri bundle license must be Apache-2.0.");
assert(tauri.bundle?.licenseFile === "../LICENSE", "Tauri bundle licenseFile must point to ../LICENSE.");
for (const resource of [
  "../LICENSE",
  "../NOTICE",
  "../TRADEMARK_POLICY.md",
  "../GENERATED_OUTPUTS.md",
  "../licenses/THIRD_PARTY_NOTICES.txt",
  "../licenses/FFmpeg-LGPL-2.1.txt",
  "../licenses/COLMAP-LICENSE.txt",
  "../licenses/NVIDIA-CUDA-Runtime.txt",
  "../licenses/Brush-LICENSE.txt",
]) {
  assert(Object.hasOwn(tauri.bundle?.resources ?? {}, resource), `Tauri resources are missing ${resource}.`);
}

const expectedEngines = new Map([
  ["FFmpeg / FFprobe", { license: "LGPL-2.1-or-later", file: "licenses/FFmpeg-LGPL-2.1.txt" }],
  ["COLMAP", { license: "BSD-3-Clause", file: "licenses/COLMAP-LICENSE.txt" }],
  ["Brush", { license: "Apache-2.0", file: "licenses/Brush-LICENSE.txt" }],
]);
const manifest = JSON.parse(readText("engines/manifest.json"));
assert(manifest.schemaVersion >= 2, "Engine manifest schemaVersion must include license mappings.");
assert(manifest.engines?.length === 3, "License verification expects exactly the three direct native engines.");

const thirdParty = readText("licenses/THIRD_PARTY_NOTICES.txt");
assert(!thirdParty.includes("OOOSplat 0.2.0"), "Third-party notices still contain the obsolete 0.2.0 heading.");
assert(!existsSync(workspacePath("licenses/FFmpeg-LICENSE.txt")), "The obsolete LGPLv3 FFmpeg-LICENSE.txt file must not exist.");

for (const engine of manifest.engines) {
  const expected = expectedEngines.get(engine.name);
  assert(expected, `Unexpected direct engine in manifest: ${engine.name}`);
  assert(engine.license?.startsWith(expected.license), `${engine.name} license identifier does not match ${expected.license}.`);
  assert(engine.licenseFiles?.length === 1, `${engine.name} must map to one direct license file.`);
  assert(engine.licenseFiles[0] === expected.file, `${engine.name} license file mapping is incorrect.`);
  assert(existsSync(workspacePath(expected.file)), `${engine.name} mapped license file is missing.`);
  assertContains(thirdParty, engine.name, "THIRD_PARTY_NOTICES.txt");
  assertContains(thirdParty, expected.license, "THIRD_PARTY_NOTICES.txt");
  assertContains(thirdParty, expected.file, "THIRD_PARTY_NOTICES.txt");
}

const windowsFfmpeg = manifest.engines.find((engine) => engine.name === "FFmpeg / FFprobe");
assert(windowsFfmpeg, "Windows FFmpeg manifest entry is missing.");
assert(
  !windowsFfmpeg.sourceUrl?.includes("/latest/"),
  "Windows FFmpeg source URL must reference an immutable release asset, not /latest/.",
);
assertContains(thirdParty, windowsFfmpeg.actualVersion, "THIRD_PARTY_NOTICES.txt");
assertContains(thirdParty, windowsFfmpeg.sourceUrl, "THIRD_PARTY_NOTICES.txt");

const linuxManifest = JSON.parse(readText("engines/manifest.linux.json"));
assert(linuxManifest.schemaVersion >= 2, "Linux engine manifest schemaVersion must include license mappings.");
assert(linuxManifest.brush?.version === "0.3.0", "Linux Brush version is incorrect.");
assert(
  linuxManifest.brush?.sourceUrl ===
    "https://github.com/ArthurBrussee/brush/releases/download/v0.3.0/brush-app-x86_64-unknown-linux-gnu.tar.xz",
  "Linux Brush release archive is incorrect.",
);
assert(linuxManifest.brush?.license === "Apache-2.0", "Linux Brush license identifier is incorrect.");
assert(
  linuxManifest.brush?.licenseFiles?.length === 1 && linuxManifest.brush.licenseFiles[0] === "licenses/Brush-LICENSE.txt",
  "Linux Brush license file mapping is incorrect.",
);
for (const marker of [
  "Ubuntu 24.04 Alpha, Linux x86_64 release archive",
  "brush-app-x86_64-unknown-linux-gnu.tar.xz",
  "engines/manifest.linux.json",
]) {
  assertContains(thirdParty, marker, "THIRD_PARTY_NOTICES.txt");
}

const ffmpegLicense = readText("licenses/FFmpeg-LGPL-2.1.txt");
assertContains(ffmpegLicense, "GNU LESSER GENERAL PUBLIC LICENSE", "FFmpeg license");
assertContains(ffmpegLicense, "Version 2.1, February 1999", "FFmpeg license");
assert(!ffmpegLicense.includes("Version 3, 29 June 2007"), "FFmpeg license still contains the LGPLv3 text.");

const colmapLicense = readText("licenses/COLMAP-LICENSE.txt");
for (const marker of [
  "ETH Zurich and UNC Chapel Hill",
  "Redistributions of source code",
  "Redistributions in binary form",
  "Neither the name",
]) {
  assertContains(colmapLicense, marker, "COLMAP license");
}

const notice = readText("NOTICE");
assertContains(notice, "Copyright 2026 ooolabdev", "NOTICE");
assertContains(notice, "licenses/THIRD_PARTY_NOTICES.txt", "NOTICE");
assertContains(notice, "TRADEMARK_POLICY.md", "NOTICE");

const trademark = readText("TRADEMARK_POLICY.md");
assertContains(trademark, "does not grant a license to use the OOOSplat Marks", "Trademark policy");
assertContains(trademark, "https://github.com/ooolabdev/ooosplat/issues", "Trademark policy");

const outputs = readText("GENERATED_OUTPUTS.md");
for (const term of [
  "final.ply",
  "Apache License 2.0",
  "General Public License (GPL)",
  "Lesser General Public License (LGPL)",
  "does not assign copyright ownership",
]) {
  assertContains(outputs, term, "Generated outputs policy");
}

console.log("Verified OOOSplat license metadata and Windows/Linux notices for 3 direct engines.");
