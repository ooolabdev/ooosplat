import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const targetRoot = path.join(workspace, "src-tauri", "target");
const generatedEngines = path.join(targetRoot, "release", "engines");
const relative = path.relative(targetRoot, generatedEngines);

if (!relative || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
  throw new Error(`Refusing to clean outside the Tauri target directory: ${generatedEngines}`);
}

fs.rmSync(generatedEngines, { recursive: true, force: true });
console.log("Removed stale generated Tauri engine resources.");
