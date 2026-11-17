import { test as base, expect, type Browser, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import net from 'node:net';
import path from 'node:path';

export interface Media {
  name: string; kind: 'images' | 'video'; count?: number; width?: number; height?: number;
  portrait?: boolean; alpha?: boolean; transparent?: boolean; erp?: boolean; fps?: number;
  duration?: number; timestamps?: number[]; jpeg?: boolean; mismatchedDimensions?: boolean;
}
export interface Scenario {
  name: string; inputs: Media[]; allowedOperations?: string[]; multipleGpus?: boolean; cpuColmap?: boolean; sharedJsonReaders?: boolean;
  faults?: { operation: string; occurrence: number; outcome: 'success' | 'geometry' | 'oom' | 'export' | 'crash' | 'io' | 'zero'; gate?: string; registered?: number }[];
}
export interface Call {
  sequence: number; engine: string; operation: string; args: string[]; environment: [string, string][];
  outcome: string; datasetInitFiles: string[];
  datasetInitSha256?: [string, string][];
  runId?: string; pipelineOperation?: { id: number; attempt: number; sourceId?: string };
}
export interface Snapshot {
  inputs: string[]; calls: Call[]; pending: string[];
  tasks: { task_id: string; run_id: string; status: string; error: unknown; project_id: string; automatically_retried: boolean }[];
  projects: { taskId: string; path: string; state: Record<string, any>; metadata: Record<string, any> }[];
}
const operations = [
  'colmap:help',
  'ffprobe:probe', 'ffprobe:timestamps', 'ffmpeg:preview', 'ffmpeg:extract',
  'colmap:feature_extractor', 'colmap:sequential_matcher', 'colmap:exhaustive_matcher',
  'colmap:matches_importer', 'colmap:vocab_tree_matcher', 'colmap:vocab_tree_retriever', 'colmap:mapper', 'colmap:pose_prior_mapper', 'brush:train',
];
const workspace = path.resolve('.');
async function freePort() {
  const server = net.createServer();
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const port = (server.address() as net.AddressInfo).port;
  await new Promise<void>(resolve => server.close(() => resolve()));
  return port;
}
export class NativeApp {
  process: ChildProcess | null = null;
  browser: Browser | null = null;
  page!: Page;
  private output = '';
  constructor(public root: string, private token: string, private playwright: typeof import('@playwright/test').chromium) {}
  async start() {
    const port = await freePort();
    this.process = spawn(path.join(workspace, 'src-tauri/target/debug/ooo-splat-e2e.exe'), [], {
      cwd: workspace, windowsHide: true, env: {
        ...process.env, OOOSPLAT_E2E_ROOT: this.root, OOOSPLAT_E2E_TOKEN: this.token,
        OOOSPLAT_E2E_CDP_PORT: String(port),
        TEMP: path.join(this.root, 'temp'), TMP: path.join(this.root, 'temp'),
        WEBVIEW2_USER_DATA_FOLDER: path.join(this.root, 'webview'),
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1 --disable-background-networking`,
      }, stdio: ['ignore', 'pipe', 'pipe'],
    });
    await writeFile(path.join(this.root, 'connection.json'), JSON.stringify({ port, pid: this.process.pid }));
    this.process.stdout?.on('data', chunk => { this.output = (this.output + chunk).slice(-131072); });
    this.process.stderr?.on('data', chunk => { this.output = (this.output + chunk).slice(-131072); });
    this.process.on('error', error => { this.output += error.stack; });
    await expect.poll(async () => {
      if (this.process?.exitCode != null) throw new Error(`E2E application exited ${this.process.exitCode}: ${this.output}`);
      try { return (await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(1000) })).ok; } catch { return false; }
    }, { timeout: 60_000, message: 'Test WebView2 must expose its local CDP endpoint' }).toBe(true);
    this.browser = await this.playwright.connectOverCDP(`http://127.0.0.1:${port}`);
    await expect.poll(() => this.browser!.contexts()[0]?.pages().length ?? 0).toBeGreaterThan(0);
    this.page = this.browser.contexts()[0].pages()[0];
    await this.page.waitForURL('http://127.0.0.1:1429/**');
    await this.page.waitForLoadState('domcontentloaded');
    await expect(this.page.locator('.quality-option')).toHaveCount(3);
    const language = this.page.locator('.language-action');
    if ((await language.innerText()).includes('中文')) await language.click();
    await expect(this.page.getByRole('button', { name: '引擎就绪', exact: true })).toBeVisible();
    await this.installDialogBridge();
  }
  async installDialogBridge() {
    await this.page.waitForFunction(() => typeof (window as any).__TAURI_INTERNALS__?.invoke === 'function');
    await this.page.evaluate(() => {
      (window as any).__e2eDialogResults ??= [];
    });
  }
  async snapshot(): Promise<Snapshot> {
    return this.page.evaluate(() => (window as any).__TAURI_INTERNALS__.invoke('e2e_snapshot'));
  }
  async release(gate: string) {
    await this.page.evaluate(gate => (window as any).__TAURI_INTERNALS__.invoke('e2e_release', { gate }), gate);
  }
  async chooseInputs(indices = [0]) {
    const state = await this.snapshot();
    // Native folder pickers cannot return video files. Exercise the actual
    // matching button, adding mixed folders/files in their declared order.
    const batches: { folders: boolean; paths: string[] }[] = [];
    for (const n of indices) {
      const input = state.inputs[n];
      const folders = !/\.(mp4|mov|jpg|jpeg|png)$/i.test(input);
      if (batches.at(-1)?.folders === folders) batches.at(-1)!.paths.push(input);
      else batches.push({ folders, paths: [input] });
    }
    let count = await this.page.locator('.material-item').count();
    for (const batch of batches) {
      await this.page.evaluate(paths => (window as any).__e2eDialogResults.push(paths), batch.paths);
      await this.page.getByRole('button', { name: batch.folders ? '添加文件夹' : '添加文件', exact: true }).first().click();
      count += batch.paths.length;
      await expect(this.page.locator('.material-item')).toHaveCount(count);
      await expect(this.page.locator('.material-row.checking')).toHaveCount(0);
    }
  }
  async quality(value: 'fast' | 'balanced' | 'high') {
    await this.page.locator('.quality-option').nth({ fast: 0, balanced: 1, high: 2 }[value]).click();
  }
  async generate() {
    await this.page.getByRole('button', { name: '开始生成', exact: true }).first().click();
  }
  async answerReshoot(condition: 'good' | 'appearance' | 'geometry' | 'unsure', camera: 'same' | 'different' | 'unsure' = 'unsure') {
    let dialog = this.page.locator('.preflight-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('input[type=radio]:checked')).toHaveCount(0);
    await dialog.locator(`input[value="${condition}"]`).check();
    await dialog.locator('button.primary-action').click();
    await expect(dialog.locator('input[value="same"]')).toBeVisible();
    await expect(dialog.locator('input[type=radio]:checked')).toHaveCount(0);
    await dialog.locator(`input[value="${camera}"]`).check();
    await dialog.locator('button.primary-action').click();
  }
  async waitCompleted(count: number) {
    await expect.poll(async () => (await this.snapshot()).tasks.filter(t => t.status === 'completed').length, { timeout: 60_000 }).toBe(count);
    return this.snapshot();
  }
  async waitTerminal(status = 'completed', taskIndex = 0) {
    await expect.poll(async () => (await this.snapshot()).tasks[taskIndex]?.status, { timeout: 60_000 }).toBe(status);
    return this.snapshot();
  }
  async stop() {
    await writeFile(path.join(this.root, 'app-output.log'), this.output);
    if (this.browser) { await this.browser.close().catch(() => {}); this.browser = null; }
    const child = this.process;
    if (child && child.exitCode == null) {
      const exited = new Promise<void>(resolve => child.once('exit', () => resolve()));
      child.kill();
      await Promise.race([exited, new Promise<never>((_, reject) => {
        const timer = setTimeout(() => reject(new Error(`E2E-owned app ${child.pid} did not stop`)), 5000); timer.unref();
      })]);
    }
    this.process = null;
  }
}
export const test = base.extend<{ app: NativeApp; scenario: Scenario }>({
  scenario: [{ name: 'ordinary', inputs: [{ name: 'photos', kind: 'images' }] }, { option: true }],
  app: async ({ playwright, scenario }, use, info) => {
    test.skip(process.platform !== 'win32', 'Native E2E requires Windows WebView2');
    const token = randomUUID();
    const root = path.join(workspace, '.cache/e2e/runs', `${info.workerIndex}-${token}`);
    await mkdir(path.join(root, 'temp'), { recursive: true });
    await writeFile(path.join(root, '.owner'), token);
    await writeFile(path.join(root, 'scenario.json'), JSON.stringify({ ...scenario, allowedOperations: scenario.allowedOperations ?? operations }, null, 2));
    const app = new NativeApp(root, token, playwright.chromium);
    try {
      await app.start();
      await use(app);
    } finally {
      if (app.browser) {
        await info.attach('actual-execution', { body: JSON.stringify(await app.snapshot().catch(() => null), null, 2), contentType: 'application/json' });
        if (info.status !== info.expectedStatus) await app.page.screenshot({ path: info.outputPath('failure.png') }).catch(() => {});
      }
      await app.stop();
      if (info.status === info.expectedStatus && await readFile(path.join(root, '.owner'), 'utf8') === token) {
        // This exact owned, per-test WebView cache is disposable. Failed cases
        // and their source fixtures remain intact for diagnosis.
        await rm(path.join(root, 'webview'), { recursive: true, force: true, maxRetries: 3 }).catch(() => {});
      }
    }
  },
  context: async ({ app }, use) => { await use(app.browser!.contexts()[0]); },
  page: async ({ app }, use) => { await use(app.page); },
});
export { expect };
