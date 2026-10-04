import fs from "node:fs";
import path from "node:path";
import { fileHash, filesUnder, verifyIntegrity } from "./colmap-runtime.mjs";
const [platform, archive, root] = process.argv.slice(2);
const integritySha256 = fileHash(path.join(root, "SHA256SUMS"));
verifyIntegrity(root, integritySha256);
const report = { platform, archive: path.basename(archive), archiveSha256: fileHash(archive), integritySha256,
  compressedBytes: fs.statSync(archive).size, runtimeBytes: filesUnder(root).reduce((sum, file) => sum + fs.statSync(file).size, 0),
  gpuExecutionValidated: false };
fs.writeFileSync(`${archive}.build-report.json`, JSON.stringify(report, null, 2) + "\n");
console.log(JSON.stringify(report, null, 2));
