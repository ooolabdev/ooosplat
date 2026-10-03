import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const probeSource = fileURLToPath(new URL("./fixtures/cuda/windows-cccl-smoke.cu", import.meta.url));
export const windowsCudaFlags = ["-allow-unsupported-compiler", "-Xcompiler=/Zc:preprocessor"];

function verifyFlags(flags, label) {
  for (const flag of windowsCudaFlags) {
    const escaped = flag.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    if (!new RegExp(`(?:^|[\\s"])${escaped}(?=$|[\\s"])`).test(flags)) {
      throw new Error(`${label} is missing required CUDA compiler flag: ${flag}`);
    }
  }
  if (/[/-]Zc:preprocessor-|CCCL_IGNORE_MSVC_TRADITIONAL_PREPROCESSOR_WARNING/.test(flags)) {
    throw new Error(`${label} disables or bypasses the required conforming MSVC preprocessor`);
  }
}

export function compileWindowsCudaProbe(compiler, outputDirectory, { flags = process.env.CUDAFLAGS ?? windowsCudaFlags.join(" "), execute = spawnSync, log = console.log } = {}) {
  verifyFlags(flags, "Windows CUDA preflight");
  const output = path.join(path.resolve(outputDirectory), "windows-cccl-smoke.obj");
  fs.mkdirSync(path.dirname(output), { recursive: true });
  // A compile-only sm_75 probe exercises CCCL and cooperative_groups without
  // requiring an NVIDIA GPU or modifying the production architecture list.
  const args = [...flags.trim().split(/\s+/), "--std=c++17", "--gpu-architecture=compute_75", "--compile", probeSource, "--output-file", output];
  log(`Windows CUDA/CCCL compile-only preflight: ${compiler} (${flags})`);
  const result = execute(path.resolve(compiler), args, { encoding: "utf8", timeout: 120000, windowsHide: true });
  const diagnostic = `${result.stdout ?? ""}\n${result.stderr ?? ""}`.trim();
  if (diagnostic) log(diagnostic);
  if (result.error || result.status !== 0) {
    throw new Error(`Windows CUDA/CCCL compile-only preflight failed: ${result.error?.message ?? result.signal ?? `exit ${result.status}`}\n${diagnostic}`);
  }
  if (!fs.existsSync(output) || !fs.statSync(output).isFile() || fs.statSync(output).size === 0) {
    throw new Error("Windows CUDA/CCCL preflight succeeded without producing an object file");
  }
  log("Windows CUDA/CCCL preflight passed; no GPU program was executed.");
  return { compiler: path.resolve(compiler), flags, object: output };
}

export function verifyWindowsCudaBuildFlags(build, { log = console.log } = {}) {
  const root = path.resolve(build);
  const cache = fs.readFileSync(path.join(root, "CMakeCache.txt"), "utf8");
  const flags = /^CMAKE_CUDA_FLAGS:STRING=(.*)$/m.exec(cache.replace(/\r\n/g, "\n"))?.[1];
  if (flags === undefined) throw new Error("CMake cache is missing actual CMAKE_CUDA_FLAGS:STRING");
  verifyFlags(flags, "CMake cached CUDA flags");
  const commands = JSON.parse(fs.readFileSync(path.join(root, "compile_commands.json"), "utf8"));
  if (!Array.isArray(commands)) throw new Error("Invalid CMake compile-command inventory");
  const cuda = commands.filter(command => typeof command.file === "string" && /\.cu$/i.test(command.file));
  const caspar = cuda.filter(command => /(?:^|[\\/])Symforce-Caspar[\\/]/i.test(command.file));
  if (!cuda.length || !caspar.length) throw new Error("Missing actual CUDA/Caspar compile commands");
  for (const command of cuda) {
    const invocation = command.command ?? command.arguments?.join(" ");
    if (typeof invocation !== "string") throw new Error(`Missing actual CUDA compile command: ${command.file}`);
    verifyFlags(invocation, `CUDA compile command ${command.file}`);
  }
  log(`Verified conforming MSVC preprocessor flags in ${cuda.length} actual CUDA compile commands (${caspar.length} Caspar).`);
  return { flags, cudaCommands: cuda.length, casparCommands: caspar.length };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [action, input, output, ...extra] = process.argv.slice(2);
  try {
    if (action === "probe" && input && output && !extra.length) compileWindowsCudaProbe(input, output);
    else if (action === "verify-build" && input && !output) verifyWindowsCudaBuildFlags(input);
    else throw new Error("Usage: node scripts/windows-cuda-preflight.mjs probe <nvcc> <output-directory> | verify-build <build-directory>");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
