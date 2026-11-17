import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import path from 'node:path';
import net from 'node:net';

const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)));
const mode = process.argv[2] ?? 'run';
const env = { ...process.env, PLAYWRIGHT_BROWSERS_PATH: path.join(root, '.cache/e2e/browsers') };
const node = process.execPath;
async function run(command, args, extraEnv = {}) {
  const code = await new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, env: { ...env, ...extraEnv }, stdio: 'inherit', windowsHide: true });
    child.once('error', reject);
    child.once('exit', (value, signal) => resolve(value ?? (signal ? 1 : 0)));
  });
  if (code !== 0) throw new Error(`${command} exited with ${code}`);
}
const cli = path.join(root, 'node_modules/@playwright/test/cli.js');
if (mode === 'report') {
  const { runId } = JSON.parse(await readFile(path.join(root, '.cache/e2e/latest.json'), 'utf8'));
  if (!/^[a-zA-Z0-9-]+$/.test(runId)) throw new Error('Invalid E2E report identity');
  await run(node, [cli, 'show-report', `.cache/e2e/artifacts/${runId}/report`, '--host=127.0.0.1', ...process.argv.slice(3)]);
} else {
  if (!['run', 'debug'].includes(mode)) throw new Error(`Unknown E2E command: ${mode}`);
  if (process.platform !== 'win32') throw new Error('Native recipe E2E currently requires Windows/WebView2; this is not a browser-only substitute.');
  await mkdir(path.join(root, '.cache/e2e/runs'), { recursive: true });
  const runId = `${new Date().toISOString().replace(/[^0-9]/g, '')}-${randomUUID()}`;
  env.OOOSPLAT_E2E_REPORT_RUN = runId;
  const config = await readFile(path.join(root, 'src-tauri/tauri.e2e.conf.json'), 'utf8');
  await run('cargo', ['build', '--manifest-path', 'src-tauri/Cargo.toml', '--features', 'e2e', '--bin', 'ooo-splat-e2e', '--bin', 'splatstudio-e2e'], { TAURI_CONFIG: config });
  // Refuse another frontend, including another E2E run. Never connect to it or
  // terminate it. The real application uses a different port and identifier.
  const portCheck = net.createServer();
  await new Promise((resolve, reject) => {
    portCheck.once('error', () => reject(new Error('Port 1429 is occupied. Stop that service before running E2E.')));
    portCheck.listen(1429, '127.0.0.1', resolve);
  });
  await new Promise(resolve => portCheck.close(resolve));
  // Spawn Node directly: Playwright's shell-based webServer can leave a Vite
  // grandchild alive on Windows after test discovery or setup fails.
  const vite = spawn(node, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1429', '--strictPort'], { cwd: root, env: { ...env, OOOSPLAT_E2E_FRONTEND: '1' }, stdio: 'inherit', windowsHide: true });
  try {
    let ready = false;
    for (let n = 0; n < 200; n++) {
      if (vite.exitCode != null) throw new Error('The isolated E2E frontend could not bind port 1429. Do not reuse another service.');
      try { ready = (await fetch('http://127.0.0.1:1429', { signal: AbortSignal.timeout(1000) })).ok; } catch {}
      if (ready) break;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    if (!ready) throw new Error('E2E frontend startup timed out');
    await writeFile(path.join(root, '.cache/e2e/latest.json'), JSON.stringify({ runId }));
    await run(node, [cli, 'test', '--config=playwright.config.ts', ...(mode === 'debug' ? ['--debug'] : []), ...process.argv.slice(3)]);
  } finally {
    if (vite.exitCode == null) {
      const exited = new Promise(resolve => vite.once('exit', resolve));
      vite.kill();
      await Promise.race([exited, new Promise((_, reject) => {
        const timer = setTimeout(() => reject(new Error(`E2E-owned frontend ${vite.pid} did not stop`)), 5000); timer.unref();
      })]);
    }
  }
}
