import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const configDirectory = path.join(workspace, "src-tauri");
const vocabularyName = "vocab_tree_faiss_flickr100K_words256K.bin";

const forbiddenFiles = [
  path.join(workspace, "engines", "colmap", "share", vocabularyName),
  path.join(workspace, "engines", "linux", "colmap", vocabularyName),
  path.join(workspace, "engines", "linux", "colmap", "share", vocabularyName),
  path.join(workspace, "engines", "macos", "arm64", "share", vocabularyName),
];

const configs = [
  "tauri.windows.conf.json",
  "tauri.linux.conf.json",
  "tauri.macos.conf.json",
];

function normalized(value) {
  return path.resolve(configDirectory, value.replace(/[\\/]$/, ""));
}

function canInclude(resource, forbidden) {
  const resourcePath = normalized(resource);
  const relative = path.relative(resourcePath, forbidden);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

for (const configName of configs) {
  const configPath = path.join(configDirectory, configName);
  const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
  const resources = config.bundle?.resources;
  const sources = Array.isArray(resources) ? resources : Object.keys(resources ?? {});

  for (const source of sources) {
    for (const forbidden of forbiddenFiles) {
      if (canInclude(source, forbidden)) {
        throw new Error(
          `${configName} resource '${source}' can bundle the unused ${vocabularyName}`,
        );
      }
    }
  }
}

console.log(`Verified bundle resource allowlists exclude ${vocabularyName} on Windows, Linux, and macOS.`);
