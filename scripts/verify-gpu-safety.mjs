import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const workspace = resolve(fileURLToPath(new URL("..", import.meta.url)));
const files = [
  "src-tauri/Cargo.toml",
  "src-tauri/build.rs",
  "src-tauri/src/commands/mod.rs",
  "src-tauri/src/engines/health.rs",
  "src-tauri/src/lib.rs",
  "src-tauri/tauri.windows.conf.json",
  "src/app/App.tsx",
  "src/lib/backend.ts",
];
const forbidden = [
  /gpu-helper/i,
  /mitigate_gpu_conflicts/i,
  /restore_gpu_conflicts/i,
  /ShellExecuteExW/i,
  /SetNamedSecurityInfoW/i,
  /(?:^|[\\/])sc\.exe/i,
  /taskkill\.exe/i,
];

const violations = [];
for (const relativePath of files) {
  const path = resolve(workspace, relativePath);
  const source = readFileSync(path, "utf8");
  for (const pattern of forbidden) {
    if (pattern.test(source)) violations.push(`${relativePath}: ${pattern}`);
  }
}

if (violations.length > 0) {
  throw new Error(`GPU safety regression detected:\n${violations.join("\n")}`);
}

const scriptPath = resolve(workspace, "scripts/windows/OOOSplat-GpuConflict.ps1");
const script = readFileSync(scriptPath, "utf8");
for (const marker of ["#requires -RunAsAdministrator", "Type $Expected exactly", "if ($Action -eq 'Close')", "$AllowedPrefixes"]) {
  if (!script.includes(marker)) throw new Error(`GPU script safety marker missing: ${marker}`);
}

console.log("Verified GPU conflict handling uses a visible, explicitly confirmed PowerShell script; the app has no direct service-mutation path.");
