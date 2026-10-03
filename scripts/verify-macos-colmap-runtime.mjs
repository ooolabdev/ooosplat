import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { filesUnder } from "./colmap-runtime.mjs";

export function deploymentVersion(text) {
  const value = /\bminos\s+(\d+(?:\.\d+)*)/.exec(text)?.[1]
    ?? /LC_VERSION_MIN_MACOSX[\s\S]*?\bversion\s+(\d+(?:\.\d+)*)/.exec(text)?.[1];
  if (!value) throw new Error("Cannot read Mach-O minimum system version");
  return value;
}

export function versionAtMost(actual, limit) {
  const left = actual.split(".").map(Number), right = limit.split(".").map(Number);
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const delta = (left[i] ?? 0) - (right[i] ?? 0);
    if (delta) return delta < 0;
  }
  return true;
}

export function dependencyPath(dependency, target, root) {
  if (dependency.startsWith("/System/Library/") || dependency.startsWith("/usr/lib/")) return null;
  const prefixes = {
    "@rpath/": path.join(root, "lib"),
    "@loader_path/": path.dirname(target),
    "@executable_path/": path.join(root, "bin"),
  };
  for (const [prefix, base] of Object.entries(prefixes)) {
    if (!dependency.startsWith(prefix)) continue;
    const resolved = path.resolve(base, dependency.slice(prefix.length));
    if (!resolved.startsWith(`${path.resolve(root)}${path.sep}`)) throw new Error(`Dependency escapes runtime: ${dependency}`);
    return resolved;
  }
  throw new Error(`Non-relocatable dependency: ${dependency}`);
}

export function verifyMacosRuntime(root, minimumSystemVersion = "15.0") {
  const command = (name, args) => {
    const result = spawnSync(name, args, { encoding: "utf8", timeout: 15000 });
    if (result.error || result.status !== 0) throw new Error(`${name} failed: ${result.error ?? result.stderr}`);
    return result.stdout;
  };
  for (const file of [...filesUnder(path.join(root, "bin")), ...filesUnder(path.join(root, "lib"))]) {
    const format = command("file", [file]);
    if (!/Mach-O/.test(format) || !/arm64/.test(format) || /x86_64/.test(format)) throw new Error(`Not arm64 Mach-O: ${file}`);
    const loadCommands = command("otool", ["-l", file]);
    const minos = deploymentVersion(loadCommands);
    if (!versionAtMost(minos, minimumSystemVersion)) throw new Error(`${file} requires macOS ${minos}, expected <= ${minimumSystemVersion}`);
    if (/\bpath\s+\/(?:opt\/homebrew|usr\/local|Users|private\/tmp|var\/folders)\//.test(loadCommands)) throw new Error(`Build-host RPATH in ${file}`);
    for (const line of command("otool", ["-L", file]).split(/\r?\n/).slice(1).filter(line => line.trim())) {
      const dependency = line.trim().split(/\s+\(compatibility version/)[0];
      const resolved = dependencyPath(dependency, file, root);
      if (resolved && !fs.existsSync(resolved)) throw new Error(`Missing dependency ${dependency} in ${file}`);
    }
    command("codesign", ["--verify", "--strict", file]);
  }
}
