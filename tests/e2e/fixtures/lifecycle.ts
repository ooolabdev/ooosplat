import { expect, type NativeApp, type Snapshot, type TaskRef, type Hit, type Scenario } from './native';
import { assertNewCacheArchive, assertPreparedBaseRebind } from '../expected/lifecycle';
import { access, lstat, readFile, readdir, stat } from 'node:fs/promises';
import { constants } from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';

export const mixedInputs: Scenario['inputs'] = [
  { name: 'first.mp4', kind: 'video', fps: 30, duration: 5 },
  { name: 'second.mp4', kind: 'video', fps: 60, duration: 3 },
  { name: 'alpha', kind: 'images', count: 30, alpha: true, transparent: true, portrait: true },
];
export const sha = async (file: string) => createHash('sha256').update(await readFile(file)).digest('hex');
export const json = async (file: string) => JSON.parse(await readFile(file, 'utf8'));
export async function assertFrozenMasks(app: NativeApp, snapshot: Snapshot, id: string) {
  const project = app.project(snapshot, id);
  const frozen = path.join(project.path, 'work/reshoot-snapshot-v3');
  const manifest = await json(path.join(frozen, 'snapshot.json'));
  const masks = manifest.assets.filter((asset: any) => asset.role === 'mask');
  expect(masks.length).toBeGreaterThan(0);
  for (const asset of masks) {
    expect(asset.relativePath).toMatch(/^masks\//);
    const source = path.join(frozen, asset.relativePath);
    const target = path.join(project.path, 'work', asset.relativePath);
    expect(await sha(source)).toBe(asset.sha256);
    expect(await readFile(target)).toEqual(await readFile(source));
    await access(target, constants.W_OK);
    expect((await stat(source)).mode & 0o200).toBe(0);
  }
}
export async function criticalHashes(root: string) {
  const result: Record<string, string> = {};
  async function walk(directory: string) {
    for (const entry of (await readdir(directory, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
      const file = path.join(directory, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`Unexpected redirected ancestor asset ${file}`);
      if (entry.isDirectory()) { if (entry.name !== 'logs') await walk(file); }
      else result[path.relative(root, file).replaceAll('\\', '/')] = await sha(file);
    }
  }
  for (const file of ['project.json', 'state.json', 'final.ply']) result[file] = await sha(path.join(root, file));
  await walk(path.join(root, 'work'));
  return result;
}
export async function completed(app: NativeApp, id = app.taskId) {
  await app.waitTerminal('completed', id || undefined);
  id ||= app.taskId;
  await expect.poll(async () => (await json(path.join(app.project(await app.snapshot(), id).path, 'work/publish-transaction.json'))).phase).toBe('reconciled');
  const s = await app.snapshot();
  const project = app.project(s, id);
  expect(await json(path.join(project.path, 'state.json'))).toEqual(project.state);
  expect(await json(path.join(project.path, 'project.json'))).toEqual(project.metadata);
  const registry=await json(path.join(app.root,'app-data/tasks.json'));
  expect(registry.tasks[id]).toEqual(app.task(s,id));
  expect(project.metadata.output.splatCount).toBe(64);
  expect(app.task(s, id).result.splatCount).toBe(64);
  return s;
}
export async function generate(app: NativeApp, indices = [0]) {
  const previous = app.taskId;
  await app.chooseInputs(indices); await app.quality('fast'); await app.generate();
  return app.reference(previous || undefined);
}
export async function derive(app: NativeApp, source: TaskRef, indices: number[], condition: 'good' | 'appearance' | 'geometry' | 'unsure' = 'good') {
  await app.reshoot(source.taskId); await app.chooseInputs(indices); await app.generate();
  const dialog = app.page.locator('.preflight-dialog');
  await expect(dialog.locator(`input[value="${condition}"]`)).toBeVisible();
  await expect(dialog.locator('input:checked')).toHaveCount(0);
  await dialog.locator(`input[value="${condition}"]`).check(); await dialog.locator('button.primary-action').click();
  for (const index of indices) {
    const media = (await app.snapshot()).inputs[index];
    // Every declared perspective source has an independent camera answer.
    // ERP sources have no camera-relationship question.
    if (/erp/i.test(media)) continue;
    await expect(dialog.locator('input[value="unsure"]')).toBeVisible();
    await expect(dialog.locator('input:checked')).toHaveCount(0);
    await dialog.locator('input[value="unsure"]').check(); await dialog.locator('button.primary-action').click();
  }
  return app.reference(source.taskId);
}
export type StopMode = 'cancel' | 'cancel-restart' | 'crash';
export async function interrupt(app: NativeApp, hit: Hit, mode: StopMode) {
  const id = hit.scope.taskId!;
  expect(hit.scope.runId).toBeTruthy();
  if (mode === 'crash') { await app.crash(); await app.start(); }
  else {
    if(app.task(await app.snapshot(),id).status==='completed') {
      const cancelled=app.page.evaluate(h=>(window as any).__TAURI_INTERNALS__.invoke('cancel_shared_task',{taskId:h.scope.taskId,runId:h.scope.runId}),hit);
      await app.releaseHit(hit);const result=await cancelled;expect(result.status).toBe('completed');
    } else await app.cancelTask(id);
    await expect.poll(async () => app.task(await app.snapshot(), id).status).toMatch(/^(cancelled|completed)$/);
    if (mode === 'cancel-restart') { await app.stop(); await app.start(); }
  }
  const task = app.task(await app.snapshot(), id);
  if (task.status !== 'completed') await app.continueTask(id);
  return id;
}
export async function cacheArchives(project: string): Promise<string[]> {
  try {
    const entries = await readdir(path.join(project, 'work/cache-archive'), { withFileTypes: true });
    return entries.filter(entry => entry.isDirectory()).map(entry => entry.name);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return [];
    throw error;
  }
}

async function verifyRestoredPreparedBase(
  original: Snapshot['projects'][number],
  current: Snapshot['projects'][number],
  archivesBefore: readonly string[],
) {
  expect(original.state.inputs.prepared).toBe(true);
  expect(original.state.featuresComplete).toBe(false);
  expect(current.state.featuresComplete).toBe(true);
  expect(current.state.reshoot.snapshotV3Identity).toBe(original.state.reshoot.snapshotV3Identity);

  const archive = assertNewCacheArchive(archivesBefore, await cacheArchives(current.path));
  const archivedColmap = path.join(current.path, 'work/cache-archive', archive, 'colmap');
  expect((await lstat(path.join(archivedColmap, 'database.db'))).isFile()).toBe(true);
  const saved = current.state.artifactManifests.entries;
  const baseFiles = assertPreparedBaseRebind(original.state.artifactManifests.entries.prepared, saved.prepared);
  for (const file of baseFiles) {
    const archived = path.join(archivedColmap, path.posix.relative('work/colmap', file.relativePath));
    const restored = path.join(current.path, file.relativePath);
    for (const copy of [archived, restored]) {
      expect((await lstat(copy)).isFile()).toBe(true);
      expect(await sha(copy), `Verified base file ${copy}`).toBe(file.sha256);
    }
  }
  expect(saved.features.upstreamIdentity).toBe(saved.prepared.artifactIdentity);
  expect(saved.matching.upstreamIdentity).toBe(saved.features.artifactIdentity);
}

export async function assertPreserved(
  app: NativeApp,
  before: Snapshot,
  after: Snapshot,
  id: string,
  archivesBefore?: readonly string[],
) {
  const original = app.project(before, id), current = app.project(after, id);
  expect(current.path).toBe(original.path);
  for (const key of ['generationPolicy', 'configuration', 'cameraGroupingPolicyVersion', 'ordinaryImagePreparationVersion', 'baPolicyVersion']) {
    if(original.state[key]!=null)expect(current.state[key]).toEqual(original.state[key]);
  }
  const saved = original.state.brushTraining;
  if (saved?.resolved) expect(current.state.brushTraining.resolved).toEqual(saved.resolved);
  if (saved?.oomRetryUsed) expect(current.state.brushTraining.oomRetryUsed).toBe(true);
  if (original.state.reshoot?.recipe) {
    expect(current.state.reshoot.recipe.actual).toEqual(original.state.reshoot.recipe.actual);
    expect(current.state.reshoot.recipe.trainingFallbacks).toBe(original.state.reshoot.recipe.trainingFallbacks);
  }
  const oldTask = app.task(before, id), newTask = app.task(after, id);
  expect(newTask.gpu_selection).toEqual(oldTask.gpu_selection);
  expect(newTask.runs.slice(0, oldTask.runs.length).map((r: any) => r.run_id)).toEqual(oldTask.runs.map((r: any) => r.run_id));
  for (const source of original.state.inputs?.sources ?? []) {
    const resumed = current.state.inputs.sources.find((s: any) => s.id === source.id);
    expect(resumed?.identity).toBe(source.identity);
    if (source.frames) for (const key of ['samplingFps', 'initialExtractedFrames', 'rescueMaxFrames', 'selectedFrames', 'candidateFrames']) expect(resumed.frames[key]).toEqual(source.frames[key]);
    if(source.frames){
      const item=current.state.inputs.request.items.find((item:any)=>item.id===source.id);
      for(const call of after.calls.filter(call=>call.scope?.taskId===id&&call.scope.sourceId===source.id&&call.operation==='extract'))
        expect(call.args[call.args.indexOf('-i')+1].replace(/^\\\\\?\\/, '')).toBe(item.source.path.replace(/^\\\\\?\\/, ''));
    }
    // Individual source records are recovery plans until the joint prepared
    // manifest is committed. normalize_collection deliberately invalidates
    // uncommitted assets and restore_frozen_video_source extracts that plan.
    if(source.frames&&original.state.inputs.prepared&&original.state.artifactManifests?.entries?.prepared){
      const extractions=(snapshot:Snapshot)=>snapshot.calls.filter(call=>call.scope?.taskId===id&&call.scope.sourceId===source.id&&call.operation==='extract').length;
      expect(extractions(after),`Completed source ${source.id} must reuse its extraction`).toBe(extractions(before));
    }
  }
  const calls = (s: Snapshot, operation: string) => s.calls.filter(c => c.scope?.taskId === id && c.operation === operation).length;
  if (original.state.featuresComplete) expect(calls(after, 'feature_extractor')).toBe(calls(before, 'feature_extractor'));
  if (original.state.matchingComplete) expect(calls(after, 'matches_importer')).toBe(calls(before, 'matches_importer'));
  if (original.state.reconstructionComplete) expect(calls(after, 'mapper')).toBe(calls(before, 'mapper'));
  if (original.state.brushComplete) expect(calls(after, 'train')).toBe(calls(before, 'train'));
  for (const name of ['prepared', 'reconstruction', 'trainingDataset', 'brushCandidate']) {
    const previous = original.state.artifactManifests?.entries?.[name];
    if (!previous) continue;
    const currentIdentity = current.state.artifactManifests.entries[name].artifactIdentity;
    if (name === 'prepared' && archivesBefore !== undefined && currentIdentity !== previous.artifactIdentity) {
      expect(newTask.run_id).not.toBe(oldTask.run_id);
      expect(newTask.runs.at(-1).kind).toBe('continue');
      await verifyRestoredPreparedBase(original, current, archivesBefore);
    } else {
      expect(currentIdentity).toBe(previous.artifactIdentity);
    }
  }
  for(const call of after.calls.filter(c=>c.scope?.taskId===id&&c.operation==='train'))expect(Object.fromEntries(call.environment).CUBECL_WGPU_DEFAULT_DEVICE).toBe('DiscreteGpu(0)');
  expect(newTask.generation_policy).toEqual(original.state.generationPolicy??oldTask.request_parameters.generationPolicy);
}
