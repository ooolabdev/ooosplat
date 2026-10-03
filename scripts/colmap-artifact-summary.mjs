import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { buildLock } from "./colmap-runtime.mjs";

export const placements = {
  windows: { architecture: "x64", destination: "engines/colmap", binary: "bin/colmap.exe" },
  linux: { architecture: "x64", destination: "engines/linux/colmap", binary: "bin/colmap" },
  macos: { architecture: "arm64", destination: "engines/macos/arm64/colmap", binary: "bin/colmap" },
};

export function artifactSummary(platform, report, artifactUrl) {
  const placement = placements[platform];
  if (!placement) throw new Error(`Unsupported platform: ${platform}`);
  const size = bytes => `${(bytes / 1024 / 1024).toFixed(2)} MiB (${bytes} bytes)`;
  const cudaIdentity = report.cudaToolkit
    ? `\n- Actual CUDA identity: metadata \`${report.cudaToolkit.metadataVersion}\`; nvcc \`${report.cudaToolkit.compiler.version}\`; components \`${JSON.stringify(report.cudaToolkit.components)}\``
    : "";
  return `## COLMAP-only: ${platform} ${placement.architecture}

[Download Actions Artifact](${artifactUrl}) — expires after 30 days.

- Archive: \`${report.archive}\`
- COLMAP source: \`${buildLock.version}\` / \`${buildLock.commit}\`
- Build backends: ${platform === "macos" ? "Ceres CPU; no CUDA/Caspar" : `CUDA ${buildLock.cudaVersion}, Caspar f32 and Ceres CPU; architectures ${buildLock.cudaArchitectures.join(";")}`}${cudaIdentity}
- Archive SHA-256 (optional reference, not required locally): \`${report.archiveSha256}\`
- Before trimming: ${size(report.preTrimBytes)}
- Runtime: ${size(report.runtimeBytes)}
- Compressed: ${size(report.compressedBytes)}
- GPU execution validated: **false** — real NVIDIA local/global Caspar BA acceptance is separate.

### Local placement (no installer or hash verification)

1. Extract the downloaded Artifact ZIP.
2. Extract the inner runtime archive, then copy **the contents** of its top-level directory into \`${placement.destination}/\` in your OOOSplat checkout.
3. The executable must be \`${placement.destination}/${placement.binary}\`, with its adjacent libraries, licenses and metadata. Do not add another wrapper directory.
4. Keep your existing FFmpeg. Run \`npm run dev:local\` or \`npm run build:local\` on the matching platform; these commands prepare pinned OOOBrush automatically.

The workflow builds no Brush, FFmpeg or OOOSplat installer. Local commands do not download or verify COLMAP packages; only OOOBrush is prepared and cached. Normal release checks and application runtime health checks are unchanged.
`;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const output = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../dist-engines");
  const reports = fs.readdirSync(output).filter(name => name.endsWith(".build-report.json"));
  if (reports.length !== 1) throw new Error("Expected exactly one COLMAP build report");
  const report = JSON.parse(fs.readFileSync(path.join(output, reports[0]), "utf8"));
  const summary = artifactSummary(process.env.COLMAP_PLATFORM, report, process.env.COLMAP_ARTIFACT_URL);
  if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
  else process.stdout.write(summary);
}
