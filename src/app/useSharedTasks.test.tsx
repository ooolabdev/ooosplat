// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useAppStore } from '../stores/appStore';
import type { SharedTask, TaskUpdate } from '../types/tasks';
const mocks = vi.hoisted(() => ({ get: vi.fn(), subscribe: vi.fn() }));
vi.mock('../lib/backend', () => ({ getSharedTasks: mocks.get, onTaskUpdate: mocks.subscribe }));
import { useSharedTasks } from './useSharedTasks';
const task = (revision: number, status: SharedTask['status'] = 'running', run = 'run-1') => ({ task_id: 'task-1', run_id: run, project_id: null, revision, status, recent_events: [], result: null, error: null } as unknown as SharedTask);
let root: Root, container: HTMLDivElement, handler: (event: TaskUpdate) => void;
let latest: ReturnType<typeof useSharedTasks>;
function Harness() { latest = useSharedTasks(); return <div>{Object.values(latest.tasks).map(t => <span key={t.task_id}>{t.status}</span>)}</div>; }
beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  mocks.get.mockReset(); mocks.subscribe.mockReset();
  mocks.subscribe.mockImplementation(async (fn: typeof handler) => { handler = fn; return vi.fn(); });
  useAppStore.setState({ phase: 'idle', latestEvent: null, events: [], lastEventSequence: 0, error: null, result: null });
  container = document.createElement('div'); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); });
describe('shared task synchronization', () => {
  it('subscribes before snapshot and preserves newer events when an older snapshot arrives', async () => {
    let snapshot!: (tasks: SharedTask[]) => void;
    mocks.get.mockImplementation(() => new Promise<SharedTask[]>(resolve => { snapshot = resolve; }));
    await act(async () => { root.render(<Harness />); });
    expect(mocks.subscribe.mock.invocationCallOrder[0]).toBeLessThan(mocks.get.mock.invocationCallOrder[0]);
    await act(async () => { handler({ task: task(5, 'completed'), event: null }); snapshot([task(3)]); });
    expect(latest.tasks['task-1'].status).toBe('completed');
  });
  it('updates terminal state without any awaited generation request and ignores delayed old execution events', async () => {
    mocks.get.mockResolvedValue([]);
    await act(async () => root.render(<Harness />));
    await act(async () => handler({ task: task(10), event: null }));
    expect(useAppStore.getState().phase).toBe('running');
    await act(async () => handler({ task: task(11, 'cancelled'), event: null }));
    expect(useAppStore.getState().phase).toBe('cancelled');
    await act(async () => handler({ task: task(8, 'running', 'old-run'), event: null }));
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
});
