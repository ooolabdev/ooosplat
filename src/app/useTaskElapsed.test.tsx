// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useTaskElapsed } from './useTaskElapsed';

const running = {
  run_id: 'run-1',
  status: 'running' as const,
  elapsed_ms: 5_000,
  updated_at: '2026-10-07T00:00:00.000Z',
  revision: 2,
};

function ElapsedProbe({ task }: { task: Parameters<typeof useTaskElapsed>[0] }) {
  return <output>{useTaskElapsed(task)}</output>;
}

beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});
afterEach(() => vi.useRealTimers());

describe('useTaskElapsed', () => {
  it('ticks from the latest backend elapsed snapshot while a task remains active', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(running.updated_at));
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(<ElapsedProbe task={running} />));
    expect(container.textContent).toBe('5000');

    await act(async () => vi.advanceTimersByTimeAsync(2_000));
    expect(container.textContent).toBe('7000');

    await act(async () => root.unmount());
  });

  it('keeps one timer across progress revisions and stops at the frozen terminal elapsed time', async () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date(running.updated_at));
    const interval = vi.spyOn(window, 'setInterval');
    const clear = vi.spyOn(window, 'clearInterval');
    const container = document.createElement('div'); const root = createRoot(container);
    await act(async () => root.render(<ElapsedProbe task={running} />));
    for (let index = 1; index <= 8; index++) {
      await act(async () => vi.advanceTimersByTimeAsync(250));
      const snapshot = { ...running, revision: index + 2,
        elapsed_ms: 5000 + index * 250, updated_at: new Date().toISOString() };
      await act(async () => root.render(<ElapsedProbe task={snapshot} />));
    }
    expect(interval).toHaveBeenCalledOnce();
    expect(clear).not.toHaveBeenCalled();
    expect(container.textContent).toBe('7000');
    await act(async () => root.render(<ElapsedProbe task={{ ...running, status: 'completed', elapsed_ms: 7250 }} />));
    expect(clear).toHaveBeenCalledOnce();
    await act(async () => vi.advanceTimersByTimeAsync(3000));
    expect(container.textContent).toBe('7250');
    await act(async () => root.unmount()); interval.mockRestore(); clear.mockRestore();
  });
});
