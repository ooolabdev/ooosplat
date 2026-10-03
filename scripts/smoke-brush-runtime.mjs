// Opt-in real GPU acceptance, never run by the COLMAP-only workflow.
// Writes only an isolated checkout cache; does not touch user datasets/engines.
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { brushLock, hostPlatform, fileSha256, workspace, verifyBrushRuntime } from "./brush-runtime.mjs";

const platform = hostPlatform(), lock = brushLock(platform);
const runtime = path.join(workspace, lock.root);
verifyBrushRuntime(runtime, lock);
const cache = path.join(workspace, ".cache", "brush-acceptance");
fs.mkdirSync(cache, { recursive: true });
const root = fs.mkdtempSync(path.join(cache, "中文 GPU 验证 "));
const dataset = path.join(root, "dataset"), sparse = path.join(dataset, "sparse", "0"), images = path.join(dataset, "images");
fs.mkdirSync(sparse, { recursive: true });
fs.mkdirSync(images, { recursive: true });
const fixtures = JSON.parse(fs.readFileSync(path.join(workspace, "scripts", "fixtures", "colmap-image-io.json"), "utf8"));
fs.writeFileSync(path.join(images, "jpeg_frame.jpg"), Buffer.from(fixtures.jpeg, "base64"));
fs.copyFileSync(path.join(workspace, "src-tauri", "icons", "128x128.png"), path.join(images, "rgba_frame.png"));
fs.writeFileSync(path.join(images, "mask_frame.png"), Buffer.from(fixtures.mask, "base64"));
fs.writeFileSync(path.join(sparse, "cameras.txt"), "# Camera list\n1 SIMPLE_RADIAL 128 128 100 64 64 0\n");
const names = ["jpeg_frame.jpg", "rgba_frame.png", "mask_frame.png"];
const points = Array.from({ length: 64 }, (_, i) => ({ id: i + 1, x: (i % 8 - 3.5) * 0.06, y: (Math.floor(i / 8) - 3.5) * 0.06, z: 2 + (i % 3) * 0.02 }));
fs.writeFileSync(path.join(sparse, "images.txt"), names.map((name, i) => `${i + 1} 1 0 0 0 ${i * 0.1} 0 0 1 ${name}\n${points.map(p => `${64 + p.x * 50} ${64 + p.y * 50} ${p.id}`).join(" ")}\n`).join(""));
fs.writeFileSync(path.join(sparse, "points3D.txt"), points.map((p, i) => `${p.id} ${p.x} ${p.y} ${p.z} 128 100 80 0.1 1 ${i} 2 ${i} 3 ${i}\n`).join(""));

const output = path.join(root, "output"), binary = path.join(runtime, lock.binary);
const args = ["--total-train-iters", "4", "--max-resolution", "64", "--refine-every", "200",
  "--max-splats", "10000", "--growth-grad-threshold", "0.00003", "--growth-select-fraction", "0.25",
  "--growth-stop-iter", "4", "--export-every", "4", "--export-path", output, "--export-name", "final.ply.tmp", dataset];
const env = { ...process.env, RUST_LOG: `${process.env.RUST_LOG ? process.env.RUST_LOG + "," : ""}brush_cli=info,brush_process=info,cubecl_wgpu=info,burn_wgpu=info`, RUST_BACKTRACE: process.env.RUST_BACKTRACE ?? "1" };
const result = spawnSync(binary, args, { cwd: root, env, encoding: "utf8", timeout: 180_000, maxBuffer: 16 * 1024 * 1024, windowsHide: true });
fs.writeFileSync(path.join(root, "stdout.log"), result.stdout ?? "");
fs.writeFileSync(path.join(root, "stderr.log"), result.stderr ?? "");
const candidate = ["final.ply.tmp", "final.ply.tmp.ply"].map(name => path.join(output, name)).find(file => fs.existsSync(file));
let plyVertices = 0;
if (candidate) {
  const file = fs.openSync(candidate, "r");
  const header = Buffer.alloc(16 * 1024);
  const bytes = fs.readSync(file, header);
  fs.closeSync(file);
  const contents = header.subarray(0, bytes).toString("utf8"), text = contents.split("end_header")[0];
  // Match the application's inspect_gaussian_ply header/property contract.
  const gaussianProperties = ["x", "y", "z", "f_dc_0", "opacity", "scale_0", "rot_0"];
  const hasProperties = gaussianProperties.every(name => new RegExp(`^property\\s+\\S+\\s+${name}\\s*$`, "m").test(text));
  if (contents.includes("end_header") && hasProperties && /^ply\r?\nformat binary_little_endian 1\.0/.test(text)) {
    plyVertices = Number(text.match(/element vertex (\d+)/)?.[1] ?? 0);
  }
}
const logs = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
const adapterDiagnostics = logs.split(/\r?\n/).filter(line => /adapter|device|backend|GPU|Vulkan|Metal/i.test(line));
const hardware = logs.match(/GPU initialization completed[^\n]*backend=(Vulkan|Metal) device=(.+?) type=(DiscreteGpu|IntegratedGpu)\b/);
const iterationsConfirmed = /Training config: train_iters=4 total_iters=4\b/.test(logs)
  && /Training progress: iteration=4 total=4\b/.test(logs);
const passed = !result.error && result.status === 0 && plyVertices > 0 && iterationsConfirmed;
const report = { platform, commit: lock.commit, tag: lock.releaseTag, binarySha256: fileSha256(binary),
  test: "4-iteration tiny COLMAP-text dataset, JPEG/RGBA PNG/alpha-mask PNG, Unicode/spaced paths and final.ply.tmp export",
  exitCode: result.status, error: result.error?.message ?? null, exportedPly: candidate ?? null, plyVertices,
  processAndExportPassed: passed, iterationsConfirmed,
  gpuExecutionValidated: passed && Boolean(hardware),
  hardware: hardware ? { backend: hardware[1], device: hardware[2], type: hardware[3] } : null,
  note: "Physical hardware is confirmed only from runtime adapter diagnostics; this is not full reconstruction/quality/reshoot acceptance.", adapterDiagnostics };
fs.writeFileSync(path.join(root, "report.json"), JSON.stringify(report, null, 2) + "\n");
console.log(JSON.stringify({ ...report, report: path.join(root, "report.json") }, null, 2));
if (!report.processAndExportPassed || !report.gpuExecutionValidated) process.exitCode = 1;
