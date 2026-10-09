// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useAppStore } from '../stores/appStore';
import type { SharedTask, TaskUpdate } from '../types/tasks';
import type { PipelineEvent } from '../types/pipeline';
const mocks = vi.hoisted(() => ({ get: vi.fn(), subscribe: vi.fn() }));
vi.mock('../lib/backend', () => ({ getSharedTasks: mocks.get, onTaskUpdate: mocks.subscribe }));
import { useSharedTasks } from './useSharedTasks';
const task = (revision: number, status: SharedTask['status'] = 'running', run = 'run-1') => ({ task_id: 'task-1', run_id: run, project_id: null, revision, status, recent_events: [], result: null, error: null } as unknown as SharedTask);
const event = (sequence: number, run = 'run-1'): PipelineEvent => ({ taskId: 'task-1', runId: run, sequence,
  revision: sequence, timestamp: '2026-10-09T00:00:00Z', kind: 'log', level: 'info', stage: 'reconstructing',
  engine: 'colmap', progress: 50, stageProgress: null, indeterminate: true, message: `line ${sequence}`,
  current: null, total: null, unit: null, elapsedMs: sequence, acceleration: null });
let root: Root, container: HTMLDivElement, handler: (event: TaskUpdate) => void;
let latest: ReturnType<typeof useSharedTasks>;
function Harness() { latest = useSharedTasks(); return <div>{Object.values(latest.tasks).map(t => <span key={t.task_id}>{t.status}</span>)}</div>; }
beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  mocks.get.mockReset(); mocks.subscribe.mockReset();
  mocks.subscribe.mockImplementation(async (fn: typeof handler) => { handler = fn; return vi.fn(); });
  useAppStore.setState({ phase: 'idle', latestEvent: null, events: [], liveTask: null, lastEventSequence: 0, error: null, result: null });
  container = document.createElement('div'); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); });
describe('shared task synchronization', () => {
  it('subscribes before snapshot and preserves newer events when an older snapshot arrives', async () => {
    let snapshot!: (tasks: SharedTask[]) => void;
    mocks.get.mockImplementation(() => new Promise<SharedTask[]>(resolve => { snapshot = resolve; }));
    await act(async () => { root.render(<Harness />); });
    expect(mocks.subscribe.mock.invocationCallOrder[0]).toBeLessThan(mocks.get.mock.invocationCallOrder[0]);
    await act(async () => { handler({ task: task(5, 'completed'), events: [], dropped_event_count: 0 }); snapshot([task(3)]); });
    expect(latest.tasks['task-1'].status).toBe('completed');
  });
  it('updates terminal state without any awaited generation request and ignores delayed old execution events', async () => {
    mocks.get.mockResolvedValue([]);
    await act(async () => root.render(<Harness />));
    await act(async () => handler({ task: task(10), events: [], dropped_event_count: 0 }));
    expect(useAppStore.getState().phase).toBe('running');
    await act(async () => handler({ task: task(11, 'cancelled'), events: [], dropped_event_count: 0 }));
    expect(useAppStore.getState().phase).toBe('cancelled');
    await act(async () => handler({ task: task(8, 'running', 'old-run'), events: [], dropped_event_count: 0 }));
    expect(latest.tasks['task-1'].status).toBe('cancelled');
    expect(useAppStore.getState().phase).toBe('cancelled');
  });
  it('restores an interrupted task when reopening and cleans up its subscription', async () => {
    const unsubscribe = vi.fn(); mocks.subscribe.mockResolvedValue(unsubscribe); mocks.get.mockResolvedValue([task(30, 'interrupted')]);
    await act(async () => root.render(<Harness />));
    expect(latest.tasks['task-1'].status).toBe('interrupted');
    await act(async () => root.unmount()); expect(unsubscribe).toHaveBeenCalledOnce();
    root = createRoot(container);
  });

  it('accepts coalesced revision jumps without fetching again and commits an ordered batch once', async () => {
    mocks.get.mockResolvedValue([task(1)]);
    await act(async () => root.render(<Harness />));
    const commits = vi.fn(); const unsubscribe = useAppStore.subscribe(commits);
    await act(async () => handler({ task: task(500), events: [event(3), event(1), event(2), event(999, 'other-run')], dropped_event_count: 25 }));
    unsubscribe();
    expect(mocks.get).toHaveBeenCalledOnce();
    expect(commits).toHaveBeenCalledOnce();
    expect(latest.tasks['task-1'].recent_events.map(item => item.sequence)).toEqual([1, 2, 3]);
    expect(useAppStore.getState().events.map(item => item.sequence)).toEqual([1, 2, 3]);
    expect(latest.tasks['task-1'].dropped_event_count).toBe(25);
  });

  it('keeps batch history bounded, rejects delayed batches, and resets history for a new run', async () => {
    mocks.get.mockResolvedValue([]);
    await act(async () => root.render(<Harness />));
    for (let batch = 0; batch < 5; batch++) {
      await act(async () => handler({ task: task((batch + 1) * 200),
        events: Array.from({ length: 200 }, (_, index) => event(batch * 200 + index + 1)), dropped_event_count: 2 }));
    }
    expect(latest.tasks['task-1'].recent_events).toHaveLength(500);
    expect(latest.tasks['task-1'].recent_events[0].sequence).toBe(501);
    expect(useAppStore.getState().events).toHaveLength(500);
    await act(async () => handler({ task: task(800), events: [event(700)], dropped_event_count: 100 }));
    expect(latest.tasks['task-1'].revision).toBe(1000);
    expect(latest.tasks['task-1'].dropped_event_count).toBe(10);
    await act(async () => handler({ task: task(1001, 'running', 'run-2'), events: [event(1, 'run-2')], dropped_event_count: 0 }));
    expect(latest.tasks['task-1'].recent_events.map(item => item.sequence)).toEqual([1]);
    expect(latest.tasks['task-1'].dropped_event_count).toBe(0);
    expect(useAppStore.getState().events.map(item => item.sequence)).toEqual([1]);
  });

  it('recovers a missed terminal notification through periodic and focus snapshots', async () => {
    vi.useFakeTimers();
    mocks.get.mockResolvedValue([task(1)]);
    await act(async () => root.render(<Harness />));
    mocks.get.mockResolvedValue([task(30, 'completed')]);
    await act(async () => vi.advanceTimersByTimeAsync(15000));
    expect(latest.tasks['task-1'].status).toBe('completed');
    expect(useAppStore.getState().phase).toBe('completed');
    mocks.get.mockResolvedValue([task(31, 'interrupted')]);
    await act(async () => window.dispatchEvent(new Event('focus')));
    expect(latest.tasks['task-1'].status).toBe('interrupted');
  });

  it('reconciles an existing live run to a terminal snapshot after remounting the page', async () => {
    useAppStore.setState({ phase: 'running', liveTask: { task_id: 'task-1', run_id: 'run-1', status: 'running',
      elapsed_ms: 10_000, updated_at: '2026-10-09T00:00:00Z' } });
    mocks.get.mockResolvedValue([task(30, 'completed')]);
    await act(async () => root.render(<Harness />));
    expect(useAppStore.getState().phase).toBe('completed');
    expect(useAppStore.getState().liveTask?.status).toBe('completed');
  });

  it('clears a stale live identity that is absent from a successful snapshot', async () => {
    useAppStore.setState({ phase: 'running', liveTask: { task_id: 'removed-task', run_id: 'removed-run', status: 'running',
      elapsed_ms: 10_000, updated_at: '2026-10-09T00:00:00Z' } });
    mocks.get.mockResolvedValue([]);
    await act(async () => root.render(<Harness />));
    expect(useAppStore.getState()).toMatchObject({ phase: 'idle', liveTask: null });
  });
});
