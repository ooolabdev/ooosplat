import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { buildLock, workspace, fileHash, filesUnder, verifyColmap } from "./colmap-runtime.mjs";
const [platform, stageArg, sourceArg, buildArg, installedArg] = process.argv.slice(2);
if (!["windows", "linux"].includes(platform)) throw new Error("Expected windows or linux");
const [stage, source, build, installed] = [stageArg, sourceArg, buildArg, installedArg].map(p => path.resolve(p));
// Deletions are limited to this freshly created build's staging directory.
if (!stage.startsWith(`${path.join(workspace, ".cache", `colmap-${platform}-build`)}${path.sep}`)) throw new Error("Unsafe staging directory");
const cacheText = fs.readFileSync(path.join(build, "CMakeCache.txt"), "utf8");
const preTrimBytes = filesUnder(stage).reduce((sum, file) => sum + fs.statSync(file).size, 0);
for (const [feature, value] of [...buildLock.disabledFeatures.map(f => [f, "OFF"]), ["CUDA", "ON"], ["CASPAR", "ON"]]) {
  if (!cacheText.includes(`${feature}_ENABLED:BOOL=${value}`)) throw new Error(`Actual build did not apply ${feature}=${value}`);
}
if (!cacheText.includes('CASPAR_USE_DOUBLE:BOOL=OFF')) throw new Error('Actual build must use Caspar f32');
if (!cacheText.includes(`CMAKE_CUDA_ARCHITECTURES:STRING=${buildLock.cudaArchitectures.join(";")}`)) throw new Error("Actual CUDA architecture set differs from the lock");
for (const directory of ["include", "share", "lib/cmake", "lib/pkgconfig"]) fs.rmSync(path.join(stage, directory), { recursive: true, force: true });
for (const file of filesUnder(stage)) if (/(?:_test\.exe|\.pdb|\.a|\.lib)$/i.test(file)) fs.unlinkSync(file);
const licenseRoot = path.join(stage, "licenses");
fs.mkdirSync(licenseRoot, { recursive: true });
const copyLicense = (file, relative) => {
  fs.mkdirSync(path.dirname(path.join(licenseRoot, relative)), { recursive: true });
  fs.copyFileSync(file, path.join(licenseRoot, relative));
};
copyLicense(path.join(source, "COPYING.txt"), "COLMAP-LICENSE.txt");
copyLicense(path.join(workspace, "licenses/NVIDIA-CUDA-Runtime.txt"), "NVIDIA-CUDA-Runtime.txt");
copyLicense(path.join(stage, "CUDA-EULA.html"), "NVIDIA-CUDA-EULA.html");
fs.unlinkSync(path.join(stage, "CUDA-EULA.html"));
for (const component of ["PoissonRecon", "SiftGPU", "VLFeat", "Symforce-Caspar"]) copyLicense(path.join(source, "src/thirdparty", component, "LICENSE"), `colmap-thirdparty/${component}-LICENSE.txt`);
for (const fetched of ["poselib", "faiss"]) {
  const candidates = filesUnder(path.join(build, "_deps", `${fetched}-src`)).filter(p => /^(?:LICENSE|COPYING)/i.test(path.basename(p)));
  if (!candidates.length) throw new Error(`Missing ${fetched} license`);
  candidates.forEach((file, i) => copyLicense(file, `colmap-thirdparty/${fetched}-${i}-${path.basename(file)}`));
}
const components = [];
const owners = new Map();
const triplet = path.basename(installed);
const infoRoot = path.join(path.dirname(installed), "vcpkg/info");
for (const list of fs.readdirSync(infoRoot).filter(f => f.endsWith(`_${triplet}.list`))) {
  const [name, version] = list.split("_");
  const copyright = path.join(installed, "share", name, "copyright");
  const component = { name, version, licenseFiles: [], files: [] };
  if (fs.existsSync(copyright)) {
    copyLicense(copyright, `vcpkg/${name}/copyright`);
    component.licenseFiles.push(`vcpkg/${name}/copyright`);
  }
  for (const relative of fs.readFileSync(path.join(infoRoot, list), "utf8").split(/\r?\n/).filter(Boolean)) {
    const base = path.basename(relative);
    if (/\.(dll|so(?:\..*)?)$/i.test(base)) owners.set(base.toLowerCase(), component);
  }
  components.push(component);
}
const originsFile = path.join(stage, "DEPENDENCY-ORIGINS.json");
const origins = fs.existsSync(originsFile) ? JSON.parse(fs.readFileSync(originsFile, "utf8")) : [];
if (fs.existsSync(originsFile)) fs.unlinkSync(originsFile);
const cuda = { name: "NVIDIA CUDA runtime", version: buildLock.cudaVersion, licenseFiles: ["NVIDIA-CUDA-Runtime.txt", "NVIDIA-CUDA-EULA.html"], files: [] };
const systemComponents = new Map();
for (const file of filesUnder(stage).filter(f => /\.(dll|so(?:\..*)?)$/i.test(f))) {
  const relative = path.relative(stage, file).split(path.sep).join("/");
  const basename = path.basename(file);
  let owner = owners.get(basename.toLowerCase());
  if (/^(?:cudart64_|curand64_|libcudart\.|libcurand\.)/i.test(basename)) owner = cuda;
  if (!owner && platform === "windows" && /^(?:msvcp|vcruntime|concrt)/i.test(basename)) {
    const key = "Microsoft Visual C++ Runtime";
    if (!systemComponents.has(key)) {
      const licenseFile = path.join(stage, "MSVC-LICENSE.html");
      if (!fs.existsSync(licenseFile)) throw new Error("Missing Microsoft runtime terms captured by the build");
      copyLicense(licenseFile, "MSVC-Redistributable-LICENSE.html");
      systemComponents.set(key, { name: key, licenseFiles: ["MSVC-Redistributable-LICENSE.html"], files: [] });
    }
    owner = systemComponents.get(key);
  }
  if (!owner && platform === "linux") {
    const origin = origins.find(o => o.file === relative)?.source;
    if (origin) {
      const query = spawnSync("dpkg-query", ["-S", origin], { encoding: "utf8" });
      const packageName = query.stdout?.split(": ")[0];
      const notices = packageName && path.join("/usr/share/doc", packageName.split(":")[0], "copyright");
      if (query.status === 0 && fs.existsSync(notices)) {
        if (!systemComponents.has(packageName)) {
          const version = spawnSync("dpkg-query", ["-W", "-f=${Version}", packageName], { encoding: "utf8" }).stdout;
          copyLicense(notices, `ubuntu/${packageName.replaceAll(":", "-")}/copyright`);
          systemComponents.set(packageName, { name: packageName, version, licenseFiles: [`ubuntu/${packageName.replaceAll(":", "-")}/copyright`], files: [] });
        }
        owner = systemComponents.get(packageName);
      }
    }
  }
  if (!owner?.licenseFiles.length) throw new Error(`Cannot safely map runtime dependency ${relative} to its license; preserve it and fix the inventory`);
  owner.files.push(relative);
}
fs.rmSync(path.join(stage, "MSVC-LICENSE.html"), { force: true });
const inventory = { schemaVersion: 1, note: "Pinned vcpkg ports plus transitive runtime libraries; unclassified dependencies fail packaging instead of being deleted.",
  components: [...components, cuda, ...systemComponents.values()].filter(c => c.files.length),
  sourceLicenseFiles: filesUnder(licenseRoot).map(f => path.relative(licenseRoot, f).split(path.sep).join("/")) };
fs.writeFileSync(path.join(stage, "BUNDLED-COMPONENTS.json"), JSON.stringify(inventory, null, 2) + "\n");
const colmapFeatures = Object.fromEntries(buildLock.disabledFeatures.map(f => [f, false]));
Object.assign(colmapFeatures, { CUDA: true, CASPAR: true, CERES: true, CASPAR_USE_DOUBLE: false });
const compiler = spawnSync(platform === "windows" ? "cl" : "c++", platform === "windows" ? [] : ["--version"], { encoding: "utf8" });
const info = { schemaVersion: 1, platform, architecture: "x64", generatedAt: new Date().toISOString(),
  colmap: { version: buildLock.version, commit: buildLock.commit, sourceUrl: buildLock.sourceUrl, sourceSha256: buildLock.sourceSha256 },
  colmapFeatures, cudaVersion: buildLock.cudaVersion, cudaArchitectures: buildLock.cudaArchitectures,
  dependencies: { vcpkgCommit: buildLock.vcpkgCommit }, compiler: `${compiler.stdout ?? ""}${compiler.stderr ?? ""}`.trim(),
  cmakeVersion: buildLock.cmakeVersion, ninjaVersion: buildLock.ninjaVersion, trimPolicy: buildLock.trimPolicy };
fs.writeFileSync(path.join(stage, "BUILD-INFO.json"), JSON.stringify(info, null, 2) + "\n");
const entries = filesUnder(stage).filter(f => path.basename(f) !== "SHA256SUMS").map(file => `${fileHash(file)}  ${path.relative(stage, file).split(path.sep).join("/")}`);
fs.writeFileSync(path.join(stage, "SHA256SUMS"), entries.join("\n") + "\n");
verifyColmap(stage, platform, fileHash(path.join(stage, "SHA256SUMS")));
const imageSmoke = spawnSync(process.execPath, [path.join(workspace, "scripts/smoke-colmap-image-io.mjs"), path.join(stage, "bin", platform === "windows" ? "colmap.exe" : "colmap")], { stdio: "inherit", windowsHide: true });
if (imageSmoke.error || imageSmoke.status !== 0) throw new Error("Cannot publish a trimmed runtime without JPEG/PNG/CPU SIFT smoke verification");
const output = path.join(workspace, "dist-engines");
fs.mkdirSync(output, { recursive: true });
const archive = path.join(output, `ooosplat-colmap-${platform}-x64-v0.1.0.${platform === "windows" ? "zip" : "tar.xz"}`);
const result = platform === "windows"
  ? spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", "Compress-Archive -LiteralPath $env:OOOSPLAT_STAGE -DestinationPath $env:OOOSPLAT_ARCHIVE -Force"], { stdio: "inherit", env: { ...process.env, OOOSPLAT_STAGE: stage, OOOSPLAT_ARCHIVE: archive }, windowsHide: true })
  : spawnSync("tar", ["-cJf", archive, "-C", path.dirname(stage), path.basename(stage)], { stdio: "inherit" });
if (result.error || result.status !== 0) throw new Error("Engine archive creation failed");
fs.writeFileSync(`${archive}.sha256`, `${fileHash(archive)}  ${path.basename(archive)}\n`);
const report = { archive: path.basename(archive), archiveSha256: fileHash(archive), integritySha256: fileHash(path.join(stage, "SHA256SUMS")),
  preTrimBytes,
  compressedBytes: fs.statSync(archive).size, runtimeBytes: filesUnder(stage).reduce((sum, file) => sum + fs.statSync(file).size, 0),
  features: colmapFeatures, gpuExecutionValidated: false, note: "Real NVIDIA mapper local/global BA acceptance remains separate from CLI verification." };
fs.writeFileSync(`${archive}.build-report.json`, JSON.stringify(report, null, 2) + "\n");
console.log(JSON.stringify(report, null, 2));
