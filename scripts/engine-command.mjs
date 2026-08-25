import { spawnSync } from "node:child_process";

const action = process.argv[2];
if (!new Set(["setup", "verify"]).has(action)) {
  console.error("Usage: node scripts/engine-command.mjs <setup|verify>");
  process.exit(2);
}

let command;
let args;
if (process.platform === "win32") {
  command = "powershell";
  args = ["-ExecutionPolicy", "Bypass", "-File", `scripts/${action}-engines.ps1`];
} else if (process.platform === "linux") {
  command = "bash";
  args = [`scripts/${action}-engines-linux.sh`];
} else if (process.platform === "darwin") {
  command = "bash";
  args = [`scripts/${action}-engines-macos.sh`];
} else {
  console.error(`Unsupported platform: ${process.platform}`);
  process.exit(1);
}

const result = spawnSync(command, args, { stdio: "inherit" });
if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 1);
