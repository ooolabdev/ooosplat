import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { DatabaseSync } from "node:sqlite";
import { workspace } from "./colmap-runtime.mjs";

const binary = path.resolve(process.argv[2]);
const fixtures = JSON.parse(fs.readFileSync(path.join(workspace, "scripts/fixtures/colmap-image-io.json"), "utf8"));
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-colmap-io-"));
const root = path.join(temporary, "中文 工程");
fs.mkdirSync(path.join(root, "images"), { recursive: true });
fs.mkdirSync(path.join(root, "masks"));
try {
  fs.writeFileSync(path.join(root, "images/jpeg_frame.jpg"), Buffer.from(fixtures.jpeg, "base64"));
  fs.copyFileSync(path.join(workspace, "src-tauri/icons/128x128.png"), path.join(root, "images/png_frame.png"));
  for (const name of ["jpeg_frame.jpg", "png_frame.png"]) fs.writeFileSync(path.join(root, "masks", `${name}.png`), Buffer.from(fixtures.mask, "base64"));
  // Relative ASCII image paths preserve the production Windows Unicode-path
  // workaround, while the actual working directory contains Unicode/spaces.
  const environment = { ...process.env, PATH: process.platform === "win32" ? path.join(process.env.SystemRoot ?? "C:\\Windows", "System32") : "/usr/bin:/bin:/usr/sbin:/sbin" };
  delete environment.LD_LIBRARY_PATH;
  delete environment.DYLD_LIBRARY_PATH;
  delete environment.DYLD_FALLBACK_LIBRARY_PATH;
  const result = spawnSync(binary, ["feature_extractor", "--database_path", "database.db",
    "--image_path", "images", "--ImageReader.mask_path", "masks", "--ImageReader.single_camera", "1",
    "--ImageReader.camera_model", "SIMPLE_RADIAL", "--FeatureExtraction.use_gpu", "0",
    "--FeatureExtraction.num_threads", "2"], { cwd: root, encoding: "utf8", timeout: 60000, windowsHide: true, env: environment });
  if (result.error || result.status !== 0) throw new Error(`JPEG/PNG CPU SIFT smoke failed: ${result.error ?? result.stderr}`);
  const database = new DatabaseSync(path.join(root, "database.db"), { readOnly: true });
  try {
    const images = database.prepare("SELECT COUNT(*) AS n FROM images").get().n;
    const features = database.prepare("SELECT COUNT(*) AS n, SUM(rows) AS features FROM keypoints").get();
    const camera = database.prepare("SELECT width, height FROM cameras").get();
    if (images !== 2 || features.n !== 2 || features.features < 1 || camera.width !== 128 || camera.height !== 128) throw new Error("Image loader/SIFT/mask smoke produced incomplete database data");
  } finally { database.close(); }
  console.log("Verified JPEG + RGBA PNG decoding, alpha masks, CPU SIFT and Unicode working directory.");
} finally { fs.rmSync(temporary, { recursive: true, force: true }); }
