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
  faults?: { operation: string; occurrence: number; outcome: 'success' | 'geometry' | 'oom' | 'export' | 'crash' | 'io' | 'zero'; gate?: string; registered?: number; task?: number; source?: string; registeredBySource?: Record<string, number> }[];
  barriers?: BarrierRule[];
  lateEvent?: { id: string; task: number; operation: string };
}
export interface BarrierRule { id: string; operation: string; boundary: 'partial' | 'written' | 'checkpoint' | 'entered'; task?: number; source?: string; occurrence?: number }
export interface Scope { taskId: string | null; runId: string | null; taskOrdinal: number; runOrdinal: number; sourceId: string | null; sourceName: string | null; operationId: number | null }
export interface Hit { id: string; ruleId: string; scope: Scope; operation: string; boundary: string; occurrence: number; evidence: any; disposition: string }
export interface TaskRef { taskId: string; runId: string }
export interface Call {
  sequence: number; engine: string; operation: string; args: string[]; environment: [string, string][];
  outcome: string; datasetInitFiles: string[];
  datasetInitSha256?: [string, string][];
  runId?: string; pipelineOperation?: { id: number; attempt: number; sourceId?: string };
  scope?: Scope;
  inputLists?: Record<string,string[]>;
}
export interface Snapshot {
  inputs: string[]; calls: Call[]; pending: string[];
  tasks: { task_id: string; run_id: string; status: string; error: unknown; project_id: string; automatically_retried: boolean; [key: string]: any }[];
  projects: { taskId: string; path: string; state: Record<string, any>; metadata: Record<string, any> }[];
  lifecycle: { hits: Hit[]; consumed: string[]; counts: Record<string, number> };
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
  taskId = '';
  readonly ancestors = new Map<string,Record<string,string>>();
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
    const task=this.task(await this.snapshot());
    await this.page.evaluate(args => (window as any).__TAURI_INTERNALS__.invoke('e2e_release', args), {gate,taskId:task.task_id,runId:task.run_id});
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
      await this.page.getByRole('button', { name: batch.folders ? '添加文件夹' : '添加文件', exact: true }).click();
      count += batch.paths.length;
      await expect(this.page.locator('.material-item')).toHaveCount(count);
      await expect(this.page.locator('.material-row.checking')).toHaveCount(0);
    }
  }
  async quality(value: 'fast' | 'balanced' | 'high') {
    await this.page.getByRole('radio',{name:new RegExp(`^${{fast:'快速',balanced:'均衡',high:'精细'}[value]}`)}).click();
  }
  async generate() {
    const draft = this.page.locator('.project-row[data-draft-id][aria-pressed="true"]');
    await expect(draft).toHaveCount(1); this.taskId = (await draft.getAttribute('data-draft-id'))!;
    await this.page.getByRole('button', { name: '开始生成', exact: true }).click();
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
  async waitTerminal(status = 'completed', taskId?: string) {
    await expect.poll(async () => {
      const snapshot = await this.snapshot();
      return snapshot.tasks.find(t => t.task_id === (taskId ?? this.taskId))?.status;
    }, { timeout: 60_000 }).toMatch(/^(completed|failed|cancelled)$/);
    const snapshot = await this.snapshot();
    expect(this.task(snapshot, taskId ?? this.taskId), 'Actual terminal task, including its durable error').toMatchObject({status});
    return snapshot;
  }
  task(snapshot: Snapshot, id = this.taskId) { const task = snapshot.tasks.find(t => t.task_id === id); if (!task) throw new Error(`Missing task ${id}`); return task; }
  project(snapshot: Snapshot, id = this.taskId) { const project = snapshot.projects.find(p => p.taskId === id); if (!project) throw new Error(`Missing project for ${id}`); return project; }
  async reference(previousTaskId?: string): Promise<TaskRef> {
    await expect.poll(async () => { const s = await this.snapshot(); return !!this.taskId && this.taskId !== previousTaskId && !!s.tasks.find(t => t.task_id === this.taskId)?.run_id; }).toBe(true);
    const task = this.task(await this.snapshot()); return { taskId: task.task_id, runId: task.run_id };
  }
  async selectTask(id = this.taskId) {
    const row = this.page.locator(`.project-row[data-task-id="${id}"], .project-row[data-draft-id="${id}"]`);
    await expect(row).toHaveCount(1); await row.click(); await expect(row).toHaveAttribute('aria-pressed', 'true');
  }
  async reshoot(id = this.taskId) {
    const {criticalHashes,json}=await import('./lifecycle');
    const parent=this.project(await this.snapshot(),id);
    await expect.poll(async()=>(await json(path.join(parent.path,'work/publish-transaction.json'))).phase).toBe('reconciled');
    const hashes=await criticalHashes(parent.path);
    if(this.ancestors.has(parent.path))expect(hashes).toEqual(this.ancestors.get(parent.path));else this.ancestors.set(parent.path,hashes);
    await this.selectTask(id);await this.page.locator(`.project-row[data-task-id="${id}"], .project-row[data-draft-id="${id}"]`).getByRole('button', { name: '补拍', exact: true }).click();
  }
  async continueTask(id = this.taskId):Promise<TaskRef> {
    const previous=this.task(await this.snapshot(),id);
    await this.selectTask(id);await this.page.getByRole('region', { name: '任务详情', exact: true }).getByRole('button', { name: '继续生成', exact: true }).click();
    await expect.poll(async()=>{const current=this.task(await this.snapshot(),id);return current.run_id!==previous.run_id||current.status==='completed';}, {message:'Continuation must be admitted before waiting for its terminal result'}).toBe(true);
    const current=this.task(await this.snapshot(),id);return {taskId:id,runId:current.run_id};
  }
  async cancelTask(id = this.taskId) { await this.selectTask(id); await this.page.getByRole('button', { name: '取消任务并终止所有进程', exact: true }).click(); }
  async waitHit(ruleId: string): Promise<Hit> {
    await expect.poll(async () => (await this.snapshot()).lifecycle.hits.some(h => h.ruleId === ruleId && h.disposition === 'pending')).toBe(true);
    const snapshot=await this.snapshot(),hit=snapshot.lifecycle.hits.find(h => h.ruleId === ruleId && h.disposition === 'pending')!;
    await writeFile(path.join(this.root,`boundary-${hit.id.replace(/[^a-zA-Z0-9-]/g,'_')}.json`),JSON.stringify(snapshot,null,2));
    return hit;
  }
  async releaseHit(hit: Hit) { await this.page.evaluate(h => (window as any).__TAURI_INTERNALS__.invoke('e2e_release_hit', {hitId:h.id,taskId:h.scope.taskId,runId:h.scope.runId}),hit); }
  async crash() {
    const child = this.process;
    if (!child || child.exitCode != null) throw new Error('No owned live application to crash');
    await writeFile(path.join(this.root, `app-output-${child.pid}.log`), this.output);
    const exited = new Promise<void>(resolve => child.once('exit', () => resolve()));
    child.kill('SIGKILL'); await exited; this.process = null;
    await this.browser?.close().catch(() => {}); this.browser = null;
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
      if(app.ancestors.size){
        const {criticalHashes}=await import('./lifecycle');
        const evidence=[];
        for(const [directory,before] of app.ancestors){
          try{const after=await criticalHashes(directory);evidence.push({directory,before,after,unchanged:JSON.stringify(before)===JSON.stringify(after)});}
          catch(error){evidence.push({directory,before,error:String(error),unchanged:false});}
        }
        await writeFile(path.join(root,'ancestor-checks.json'),JSON.stringify(evidence,null,2));
        await info.attach('ancestor-checks',{body:JSON.stringify(evidence,null,2),contentType:'application/json'});
        expect(evidence.filter(item=>!item.unchanged),'Every protected ancestor must remain unchanged, including after a failed child').toEqual([]);
      }
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
