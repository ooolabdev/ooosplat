// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { PipelineResult } from '../types/pipeline';
import type { SharedTask, StartReceipt, TaskUpdate } from '../types/tasks';

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke, convertFileSrc: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: vi.fn(), open: vi.fn(), save: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ revealItemInDir: vi.fn() }));
import { continueTaskPipeline, resumePipeline, startInputsPipeline, startPipeline, startReshootPipeline } from './backend';

const result: PipelineResult = {
  projectId: 'project', projectPath: 'E:/project', finalPly: 'E:/project/final.ply',
  fileSize: 100, splatCount: 1, inputImages: 2, registeredImages: 2, registeredRatio: 1,
  points3d: 1, durationMs: 1000, completedAt: '2026-11-11T00:00:00Z', warning: null,
  logsDirectory: 'E:/project/logs',
};
const receipt = (overrides: Partial<StartReceipt> = {}): StartReceipt => ({
  accepted: true, task_id: 'task', run_id: 'current-run', status: 'starting', revision: 10, ...overrides,
});
const task = (overrides: Partial<SharedTask> = {}): SharedTask => ({
  task_id: 'task', run_id: 'current-run', project_id: 'project', project_path: 'E:/project',
  revision: 10, status: 'running', recent_events: [], result: null, error: null,
  input_path: 'E:/input.mp4', quality: 'balanced', source: 'gui', task_kind: 'generation',
  planner_enabled: true, projects_root: 'E:/projects', stage: 'trainingSplats', sequence: 0,
  created_at: '2026-11-11T00:00:00Z', updated_at: '2026-11-11T00:00:00Z', elapsed_ms: 0,
  progress: null, estimated_progress: null, current: null, total: null, eta_seconds: null,
  ...overrides,
});
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };

let handler: (event: { payload: TaskUpdate }) => void;
let unsubscribe: ReturnType<typeof vi.fn>;
const emit = (value: SharedTask) => handler({ payload: { task: value, events: [], dropped_event_count: 0 } });
beforeEach(() => {
  vi.useFakeTimers();
  (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  mocks.invoke.mockReset(); mocks.listen.mockReset();
  unsubscribe = vi.fn();
  mocks.listen.mockImplementation(async (_event, listener) => { handler = listener; return unsubscribe; });
});
afterEach(() => {
  vi.clearAllTimers(); vi.useRealTimers();
  delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});

describe('identified task result waiter', () => {
  it.each(['failed', 'cancelled', 'interrupted', 'completed'] as const)('ignores a delayed old-run %s event and accepts only the admitted result', async status => {
    let snapshot!: (value: SharedTask) => void;
    mocks.invoke.mockResolvedValueOnce(receipt()).mockImplementationOnce(() => new Promise(resolve => { snapshot = resolve; }));
    let settled = false;
    const pending = continueTaskPipeline('task', 'old-run');
    void pending.then(() => { settled = true; }, () => { settled = true; });
    await flush();
    emit(task({ run_id: 'old-run', revision: 9, status, result: { ...result, projectId: 'wrong-project' } }));
    await flush();
    expect(settled).toBe(false);
    snapshot(task());
    await flush();
    emit(task({ revision: 11, status: 'completed', result }));
    await expect(pending).resolves.toEqual(result);
    expect(unsubscribe).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
  });

  it.each([true, false])('accepts the receipt revision itself, including an idempotent existing receipt (accepted=%s)', async accepted => {
    mocks.invoke.mockResolvedValueOnce(receipt({ accepted })).mockResolvedValueOnce(task({ status: 'completed', result }));
    await expect(continueTaskPipeline('task', 'old-run')).resolves.toEqual(result);
    expect(unsubscribe).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each(['event', 'query'])('ends local waiting when a newer run is observed through %s without following or cancelling it', async source => {
    let snapshot!: (value: SharedTask) => void;
    mocks.invoke.mockResolvedValueOnce(receipt()).mockImplementationOnce(() => new Promise(resolve => { snapshot = resolve; }));
    const pending = continueTaskPipeline('task', 'old-run');
    const rejection = expect(pending).rejects.toMatchObject({ code: 'STALE_RUN_ID', taskId: 'task', runId: 'current-run' });
    await flush();
    const newer = task({ run_id: 'newer-run', revision: 12, status: 'completed', result: { ...result, projectId: 'wrong-project' } });
    if (source === 'event') emit(newer); else snapshot(newer);
    await rejection;
    if (source === 'event') snapshot(task());
    await flush();
    expect(unsubscribe).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    expect(mocks.invoke.mock.calls.map(call => call[0])).toEqual(['start_gui_task', 'get_shared_task']);
  });

  it('does not let another task or an older snapshot end polling', async () => {
    mocks.invoke.mockResolvedValueOnce(receipt())
      .mockResolvedValueOnce(task({ run_id: 'old-run', revision: 9, status: 'failed' }))
      .mockResolvedValueOnce(task({ revision: 11, status: 'completed', result }));
    const pending = continueTaskPipeline('task', 'old-run');
    await flush();
    emit(task({ task_id: 'another-task', revision: 500, status: 'failed' }));
    expect(vi.getTimerCount()).toBe(1);
    await vi.advanceTimersByTimeAsync(15000);
    await expect(pending).resolves.toEqual(result);
    expect(vi.getTimerCount()).toBe(0);
    expect(unsubscribe).toHaveBeenCalledOnce();
  });

  it.each([
    { run_id: null },
    { run_id: '' },
    { task_id: '' },
    { revision: undefined },
    { revision: -1 },
    { revision: 1.5 },
  ])('rejects an unbindable receipt rather than guessing the latest run: %j', async invalid => {
    mocks.invoke.mockResolvedValueOnce({ ...receipt(), ...invalid });
    await expect(continueTaskPipeline('task', 'old-run')).rejects.toMatchObject({ code: 'INVALID_DATA' });
    expect(mocks.invoke).toHaveBeenCalledOnce();
    expect(mocks.listen).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('releases a subscription returned after an early terminal callback and does not query', async () => {
    let attach!: (release: () => void) => void;
    mocks.invoke.mockResolvedValueOnce(receipt());
    mocks.listen.mockImplementation((_event, listener) => {
      handler = listener;
      return new Promise(resolve => { attach = resolve; });
    });
    const pending = continueTaskPipeline('task', 'old-run');
    await flush();
    emit(task({ revision: 11, status: 'completed', result }));
    await expect(pending).resolves.toEqual(result);
    expect(unsubscribe).not.toHaveBeenCalled();
    attach(unsubscribe);
    await flush();
    expect(unsubscribe).toHaveBeenCalledOnce();
    expect(mocks.invoke).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each(['snapshot', 'error'])('does not create a polling interval when the initial %s arrives after completion', async late => {
    let snapshot!: (value: SharedTask) => void;
    let failSnapshot!: (reason: unknown) => void;
    mocks.invoke.mockResolvedValueOnce(receipt()).mockImplementationOnce(() => new Promise((resolve, reject) => { snapshot = resolve; failSnapshot = reject; }));
    const pending = continueTaskPipeline('task', 'old-run');
    await flush();
    emit(task({ revision: 12, status: 'completed', result }));
    await expect(pending).resolves.toEqual(result);
    expect(unsubscribe).toHaveBeenCalledOnce();
    if (late === 'snapshot') snapshot(task()); else failSnapshot(new Error('late query failure'));
    await flush();
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(30000);
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
    expect(unsubscribe).toHaveBeenCalledOnce();
  });

  it('keeps one query in flight and ignores its result after a terminal event', async () => {
    let snapshot!: (value: SharedTask) => void;
    mocks.invoke.mockResolvedValueOnce(receipt()).mockResolvedValueOnce(task())
      .mockImplementationOnce(() => new Promise(resolve => { snapshot = resolve; }));
    const pending = continueTaskPipeline('task', 'old-run');
    await flush();
    await vi.advanceTimersByTimeAsync(45000);
    expect(mocks.invoke).toHaveBeenCalledTimes(3);
    emit(task({ revision: 20, status: 'completed', result }));
    await expect(pending).resolves.toEqual(result);
    snapshot(task({ revision: 15 }));
    await flush();
    expect(vi.getTimerCount()).toBe(0);
    expect(unsubscribe).toHaveBeenCalledOnce();
  });

  it('preserves an identified failure when releasing its subscription throws', async () => {
    unsubscribe.mockImplementation(() => { throw new Error('cleanup failed'); });
    mocks.invoke.mockResolvedValueOnce(receipt()).mockResolvedValueOnce(task({ status: 'failed',
      error: { message: 'actual root cause' } as SharedTask['error'] }));
    await expect(continueTaskPipeline('task', 'old-run')).rejects.toMatchObject({
      code: 'pipeline_failed', message: 'actual root cause', taskId: 'task', runId: 'current-run',
    });
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each([
    ['new materials', () => startInputsPipeline({ mode: 'materials', items: [] }, 'balanced', 'E:/projects', 'draft', { version: 7, autoRetryEnabled: true, frameSamplingRatio: null })],
    ['legacy generation', () => startPipeline('E:/input.mp4', 'balanced', 'E:/projects')],
    ['project continuation', () => resumePipeline('project', 'old-run')],
    ['legacy reshoot', () => startReshootPipeline({ sourceProjectId: 'source', reshootPath: 'E:/input.mp4', inputType: 'video', projectsRoot: 'E:/projects' })],
    ['unbound continuation', () => continueTaskPipeline('task', 'old-run')],
  ] as const)('binds the full receipt for %s', async (_name, start) => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === 'create_gui_task') return { task_id: 'task' };
      if (command === 'get_shared_task') return task({ run_id: 'superseding-run', revision: 11, status: 'completed', result });
      return receipt();
    });
    await expect(start()).rejects.toMatchObject({ code: 'STALE_RUN_ID', runId: 'current-run' });
    expect(unsubscribe).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });
});
