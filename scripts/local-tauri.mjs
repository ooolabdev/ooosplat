import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { hostPlatform, prepareBrush } from "./brush-runtime.mjs";

const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

// Only these explicit local commands bypass COLMAP archive/manifest checks.
// Normal setup, build:bundle, package:windows and application CI are unchanged.
export function localConfiguration(platform) {
  if (!["win32", "linux", "darwin"].includes(platform)) throw new Error(`Unsupported local platform: ${platform}`);
  const config = { build: { beforeBuildCommand: "npm run build" }, bundle: { resources: {} } };
  const roots = {
    win32: "../engines/colmap",
    linux: "../engines/linux/colmap",
    darwin: "../engines/macos/arm64/colmap",
  };
  const destinations = {
    win32: "engines/colmap",
    linux: "engines/linux/colmap",
    darwin: "engines/macos/arm64/colmap",
  };
  const formalColmapResources = {
    win32: ["../engines/colmap/bin/", "../engines/colmap/lib/", "../engines/colmap/licenses/", "../engines/colmap/BUILD-INFO.json", "../engines/colmap/BUNDLED-COMPONENTS.json", "../engines/colmap/SHA256SUMS"],
    linux: ["../engines/linux/colmap/bin/", "../engines/linux/colmap/lib/", "../engines/linux/colmap/licenses/", "../engines/linux/colmap/BUILD-INFO.json", "../engines/linux/colmap/BUNDLED-COMPONENTS.json", "../engines/linux/colmap/SHA256SUMS"],
    darwin: ["../engines/macos/arm64/colmap/"],
  };
  for (const resource of formalColmapResources[platform]) config.bundle.resources[resource] = null;
  config.bundle.resources[`${roots[platform]}/`] = `${destinations[platform]}/`;
  if (platform === "darwin") {
    const root = "../engines/macos/arm64";
    Object.assign(config.bundle, {
      macOS: { signingIdentity: "-" },
      resources: {
        ...config.bundle.resources,
        // RFC 7396 nulls remove mixed-runtime resources from the local merge.
        [`${root}/bin/`]: null,
        [`${root}/SHA256SUMS`]: null,
        [`${root}/BUILD-INFO.json`]: null,
        [`${root}/BUNDLED-COMPONENTS.json`]: null,
        [`${root}/bin/ffmpeg`]: "engines/macos/arm64/bin/ffmpeg",
        [`${root}/bin/ffprobe`]: "engines/macos/arm64/bin/ffprobe",
        [`${root}/bin/brush_app`]: "engines/macos/arm64/bin/brush_app",
      },
    });
  }
  return config;
}

export function localInvocation(action, platform, extra = []) {
  if (!["dev", "build"].includes(action)) throw new Error("Usage: node scripts/local-tauri.mjs dev|build [Tauri arguments]");
  // Invocation construction is pure. Only Brush is prepared before execution;
  // manually placed COLMAP and existing FFmpeg are never verified/downloaded.
  return {
    command: process.execPath,
    args: [path.join(workspace, "node_modules", "@tauri-apps", "cli", "tauri.js"), action,
      "--features", "local-colmap", "--config", JSON.stringify(localConfiguration(platform)), ...extra],
  };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const action = process.argv[2];
  const invocation = localInvocation(action, process.platform, process.argv.slice(3));
  if (!process.argv.slice(3).some(arg => ["--help", "-h", "--version", "-V"].includes(arg))) {
    prepareBrush(hostPlatform());
  }
  console.log("Using pinned OOOBrush CLI; manually placed COLMAP skips package verification and downloads. FFmpeg is unchanged.");
  const result = spawnSync(invocation.command, invocation.args, { cwd: workspace, stdio: "inherit", windowsHide: true });
  if (result.error) throw result.error;
  process.exit(result.status ?? 1);
}
