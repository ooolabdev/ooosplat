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

function ElapsedProbe({ task }: { task: typeof running }) {
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
});
