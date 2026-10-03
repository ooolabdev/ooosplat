import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { filesUnder } from "./colmap-runtime.mjs";
const [stage, installed, cuda] = process.argv.slice(2).map(p => path.resolve(p));
const origins = [];
// Only ABI-stable Ubuntu base libraries and host-provided NVIDIA driver files
// are external. This is not a generic /usr/lib exclusion.
const hostLibrary = /^(?:ld-linux-x86-64|lib(?:c|m|pthread|dl|rt|resolv|util|gcc_s|stdc\+\+))\.so(?:\.|$)|^(?:libcuda\.so|libnvidia-)/;
const queue = [path.join(stage, "bin/colmap"), ...filesUnder(path.join(stage, "lib"))];
const seen = new Set();
const environment = { ...process.env, LD_LIBRARY_PATH: [path.join(stage, "lib"), path.join(installed, "lib"), path.join(cuda, "lib64"), path.join(cuda, "targets/x86_64-linux/lib")].join(":") };
while (queue.length) {
  const file = queue.shift();
  if (seen.has(file)) continue;
  seen.add(file);
  const result = spawnSync("ldd", [file], { encoding: "utf8", env: environment });
  if (result.status !== 0 || /not found/.test(result.stdout)) throw new Error(`Unresolved dependency in ${file}: ${result.stdout} ${result.stderr}`);
  for (const line of result.stdout.split("\n")) {
    const match = /^\s*(\S+) => (\/\S+) \(/.exec(line);
    if (!match || hostLibrary.test(match[1])) continue;
    const source = match[2], destination = path.join(stage, "lib", match[1]);
    if (fs.existsSync(destination)) continue;
    fs.copyFileSync(source, destination);
    origins.push({ file: `lib/${match[1]}`, source: fs.realpathSync(source) });
    queue.push(destination);
  }
}
fs.writeFileSync(path.join(stage, "DEPENDENCY-ORIGINS.json"), JSON.stringify(origins, null, 2));
