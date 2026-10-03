import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileHash } from "./colmap-runtime.mjs";

export const isLicenseNotice = name => /^(?:licen[cs]e|copying|copyright|notice)(?:[_.-].*)?$/i.test(path.basename(name));

// Homebrew bottles do not consistently install upstream notices. First retain
// all available notice files; otherwise retrieve the pinned formula's verified
// source archive and read its notices without extracting files into the stage.
export function collectComponentNotices(formula, prefix, destination, cache, execute = spawnSync) {
  fs.mkdirSync(destination, { recursive: true });
  const walk = directory => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const file = path.join(directory, entry.name);
      if (entry.isDirectory()) walk(file);
      else if (entry.isFile() && isLicenseNotice(entry.name)) {
        const relative = path.relative(prefix, file);
        fs.mkdirSync(path.dirname(path.join(destination, relative)), { recursive: true });
        fs.copyFileSync(file, path.join(destination, relative));
      }
    }
  };
  walk(prefix);
  if (!fs.readdirSync(destination).length) {
    const stable = formula.urls?.stable;
    if (!stable?.url || !/^[a-f0-9]{64}$/i.test(stable.checksum ?? "")) throw new Error(`No installed notices or pinned source archive for ${formula.name}; preserve dependency and review licensing`);
    fs.mkdirSync(cache, { recursive: true });
    const archive = path.join(cache, `${formula.name}-${stable.checksum}.archive`);
    const run = (command, args, options = {}) => {
      const result = execute(command, args, { encoding: "utf8", maxBuffer: 32 * 1024 * 1024, ...options });
      if (result.error || result.status !== 0) throw new Error(`${formula.name} notices: ${command} failed: ${result.error ?? result.stderr}`);
      return result.stdout;
    };
    if (!fs.existsSync(archive) || fileHash(archive).toLowerCase() !== stable.checksum.toLowerCase()) run("curl", ["--fail", "--location", "--retry", "3", stable.url, "--output", archive]);
    if (fileHash(archive).toLowerCase() !== stable.checksum.toLowerCase()) throw new Error(`${formula.name} license-source SHA-256 mismatch`);
    const entries = run("tar", ["-tf", archive]).split(/\r?\n/).filter(entry => entry && !entry.endsWith("/") && isLicenseNotice(entry));
    let retained = 0;
    for (const [index, entry] of entries.entries()) {
      const notice = run("tar", ["-xOf", archive, entry]);
      if (!notice.trim()) continue; // A source symlink is not a notice body.
      if (notice.includes("\0")) throw new Error(`Non-text license notice in ${formula.name}: ${entry}`);
      fs.writeFileSync(path.join(destination, `${index}-${path.basename(entry)}`), notice);
      retained++;
    }
    if (!retained) throw new Error(`No upstream notices found for ${formula.name}; preserve dependency and review licensing`);
  }
  fs.writeFileSync(path.join(destination, "FORMULA-INFO.json"), JSON.stringify(formula, null, 2) + "\n");
}

if (process.argv[1] && path.basename(process.argv[1]) === "collect-macos-component-notices.mjs") {
  const [infoFile, prefix, destination, cache] = process.argv.slice(2);
  const formula = JSON.parse(fs.readFileSync(infoFile, "utf8")).formulae[0];
  collectComponentNotices(formula, prefix, destination, cache);
}
