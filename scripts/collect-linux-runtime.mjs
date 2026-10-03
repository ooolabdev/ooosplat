import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { filesUnder } from "./colmap-runtime.mjs";

export function isLinuxRuntimeElf(file) {
  const header = Buffer.alloc(18);
  const descriptor = fs.openSync(file, "r");
  let bytes;
  try { bytes = fs.readSync(descriptor, header, 0, header.length, 0); }
  finally { fs.closeSync(descriptor); }
  const namedSharedLibrary = /\.so(?:\.|$)/i.test(path.basename(file));
  const hasElfMagic = bytes >= 4 && header.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]));
  if (!hasElfMagic) {
    if (namedSharedLibrary) throw new Error(`Invalid ELF shared library: ${file}`);
    return false;
  }
  if (bytes !== header.length || ![1, 2].includes(header[4]) || ![1, 2].includes(header[5])) {
    throw new Error(`Cannot parse ELF runtime header: ${file}`);
  }
  const type = header[5] === 1 ? header.readUInt16LE(16) : header.readUInt16BE(16);
  // ET_EXEC and ET_DYN require a runtime dependency closure. Static archives,
  // relocatable objects and development files stay untouched until packaging.
  if ([2, 3].includes(type)) return true;
  if (namedSharedLibrary) throw new Error(`Invalid ELF shared library type ${type}: ${file}`);
  return false;
}

export function linuxLddDependency(line) {
  // The SONAME contains no whitespace, but its resolved absolute source path
  // can contain spaces/Unicode. Stop at ldd's address delimiter, not a space.
  const match = /^\s*(\S+) => (\/.*?)\s+\(0x[0-9a-f]+\)\s*$/i.exec(line);
  return match ? { name: match[1], source: match[2] } : null;
}

// Only ABI-stable Ubuntu base libraries and host-provided NVIDIA driver files
// are external. This is not a generic /usr/lib exclusion.
const hostLibrary = /^(?:ld-linux-x86-64|lib(?:c|m|pthread|dl|rt|resolv|util|gcc_s|stdc\+\+))\.so(?:\.|$)|^(?:libcuda\.so|libnvidia-)/;

export function collectLinuxRuntime(stage, installed, cuda, { execute = spawnSync } = {}) {
  [stage, installed, cuda] = [stage, installed, cuda].map(p => path.resolve(p));
  const origins = [];
  const queue = [path.join(stage, "bin/colmap"), ...filesUnder(path.join(stage, "lib")).filter(isLinuxRuntimeElf)];
  const seen = new Set();
  const environment = { ...process.env, LD_LIBRARY_PATH: [path.join(stage, "lib"), path.join(installed, "lib"), path.join(cuda, "lib64"), path.join(cuda, "targets/x86_64-linux/lib")].join(":") };
  while (queue.length) {
    const file = queue.shift();
    if (seen.has(file)) continue;
    seen.add(file);
    const result = execute("ldd", [file], { encoding: "utf8", env: environment });
    if (result.error || result.status !== 0 || /not found/.test(result.stdout ?? "")) throw new Error(`Unresolved dependency in ${file}: ${result.stdout} ${result.stderr} ${result.error?.message ?? ""}`);
    for (const line of (result.stdout ?? "").split("\n")) {
      const dependency = linuxLddDependency(line);
      if (!dependency || hostLibrary.test(dependency.name)) continue;
      const { name, source } = dependency, destination = path.join(stage, "lib", name);
      if (fs.existsSync(destination)) continue;
      fs.copyFileSync(source, destination);
      origins.push({ file: `lib/${name}`, source: fs.realpathSync(source) });
      queue.push(destination);
    }
  }
  fs.writeFileSync(path.join(stage, "DEPENDENCY-ORIGINS.json"), JSON.stringify(origins, null, 2));
  return origins;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 5) throw new Error("Usage: node scripts/collect-linux-runtime.mjs <stage> <installed> <cuda-root>");
  collectLinuxRuntime(...process.argv.slice(2));
}
