// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import type { PipelineEvent } from '../types/pipeline';
import type { TaskLogPage } from '../lib/backend';
const mocks = vi.hoisted(() => ({ logs: vi.fn() }));
vi.mock('../lib/backend', () => ({ readSharedTaskLogs: mocks.logs }));
import { useTaskActivityLog } from './useTaskActivityLog';

const event = (sequence: number, runId = 'run', message = 'repeated line'): PipelineEvent => ({
  taskId: 'task', runId, sequence, kind: 'log', level: 'info', engine: 'system', message,
  timestamp: '2026-11-11T00:00:00Z', stage: 'extractingFrames', progress: 10,
  stageProgress: null, indeterminate: true, current: null, total: null, unit: null,
  elapsedMs: 1000, acceleration: null,
});
const page = (events: PipelineEvent[] = [], extra: Partial<TaskLogPage> = {}): TaskLogPage => ({
  entries: [], activity_events: events, activity_journal_persisted: true,
  next_cursor: 'cursor', cursor_reset: false, has_more: false, reset_reason: null, ...extra,
});
let root: Root, container: HTMLDivElement;
let latest: ReturnType<typeof useTaskActivityLog>;
function Harness({ run = 'run', active = true, events = [] }: { run?: string; active?: boolean; events?: PipelineEvent[] }) {
  latest = useTaskActivityLog('task', run, active, events, 'en');
  return <div>{latest.logs.map(line => <p key={line.key}>{line.text}</p>)}</div>;
}
async function render(props: Parameters<typeof Harness>[0] = {}) {
  await act(async () => root.render(<Harness {...props} />));
}
beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers(); mocks.logs.mockReset(); mocks.logs.mockResolvedValue(page());
  container = document.createElement('div'); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); });

it('drains available pages immediately and merges journal/live events without text deduplication or raw splicing', async () => {
  mocks.logs.mockResolvedValueOnce(page([event(1)], { has_more: true, next_cursor: 'first' }))
    .mockResolvedValueOnce(page([event(2), event(3)], { next_cursor: 'second', entries: [{ source: 'brush', text: 'unsequenced raw', partial_line: false }] }));
  await render({ events: [event(3), event(4)] });
  expect(mocks.logs).toHaveBeenCalledTimes(2);
  expect(mocks.logs).toHaveBeenLastCalledWith('task', 'run', 'first');
  expect(latest.events.map(value => value.sequence)).toEqual([1, 2, 3, 4]);
  expect(latest.logs).toHaveLength(4);
  expect(latest.rawLogs).toEqual([]);
  expect(latest.journalPersisted).toBe(true);
});

it('keeps verified history through cursor errors and re-reads without affecting task lifecycle', async () => {
  mocks.logs.mockResolvedValueOnce(page([event(1)], { next_cursor: 'old' }))
    .mockRejectedValueOnce({ code: 'CURSOR_MISMATCH' })
    .mockResolvedValueOnce(page([event(2)], { cursor_reset: true, next_cursor: 'new' }));
  await render();
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(latest.unavailable).toBe(true);
  expect(latest.events.map(value => value.sequence)).toEqual([1]);
  expect(latest.journalPersisted).toBe(false);
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(mocks.logs).toHaveBeenLastCalledWith('task', 'run', undefined);
  expect(latest.events.map(value => value.sequence)).toEqual([1, 2]);
  expect(latest.unavailable).toBe(false);
});

it('retains its cursor on I/O errors rather than repeatedly reading a legacy tail', async () => {
  mocks.logs.mockResolvedValueOnce(page([event(1)], { next_cursor: 'position' }))
    .mockRejectedValueOnce({ code: 'IO_ERROR' })
    .mockResolvedValueOnce(page([event(2)]));
  await render();
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(latest.unavailable).toBe(true);
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(mocks.logs).toHaveBeenLastCalledWith('task', 'run', 'position');
  expect(latest.events.map(value => value.sequence)).toEqual([1, 2]);
});

it.each([
  { code: 'CURSOR_INVALID_OR_EXPIRED' },
  JSON.stringify({ code: 'CURSOR_INVALID_OR_EXPIRED', message: 'expired signature' }),
])('rebases an expired cursor without treating absent legacy journals as permanent failures (%j)', async error => {
  const legacyPage = (text: string, next_cursor: string) => page([], {
    activity_events: undefined, activity_journal_persisted: undefined,
    entries: [{ source: 'brush', text, partial_line: false }], next_cursor,
  });
  mocks.logs.mockResolvedValueOnce(legacyPage('verified old tail', 'expired'))
    .mockRejectedValueOnce(error)
    .mockResolvedValueOnce(legacyPage('re-read tail', 'fresh'))
    .mockRejectedValueOnce(error)
    .mockResolvedValueOnce(legacyPage('new re-read tail', 'fresh-again'));
  await render();
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(latest.unavailable).toBe(true);
  expect(latest.rawLogs.map(value => value.text)).toEqual(['verified old tail']);
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(mocks.logs).toHaveBeenLastCalledWith('task', 'run', undefined);
  expect(latest.unavailable).toBe(false);
  expect(latest.rawLogs.map(value => value.text)).toEqual(['re-read tail']);
  // A later expiration after a successful read gets one fresh rebase allowance.
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(mocks.logs).toHaveBeenLastCalledWith('task', 'run', 'fresh');
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(mocks.logs).toHaveBeenLastCalledWith('task', 'run', undefined);
  expect(latest.unavailable).toBe(false);
  expect(latest.rawLogs.map(value => value.text)).toEqual(['new re-read tail']);
});

it('does not loop immediately or repeatedly rebase when a fresh read still rejects the cursor', async () => {
  mocks.logs.mockResolvedValueOnce(page([event(1)], { next_cursor: 'expired' }))
    .mockRejectedValue({ code: 'CURSOR_INVALID_OR_EXPIRED' });
  await render();
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(mocks.logs).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(mocks.logs).toHaveBeenCalledTimes(3);
  expect(mocks.logs).toHaveBeenLastCalledWith('task', 'run', undefined);
  expect(latest.events.map(value => value.sequence)).toEqual([1]);
  expect(latest.unavailable).toBe(true);
  await act(async () => vi.advanceTimersByTimeAsync(0));
  expect(mocks.logs).toHaveBeenCalledTimes(3);
});

it('replaces only reset legacy source chunks and keeps the backend truncation evidence', async () => {
  mocks.logs.mockResolvedValueOnce(page([], { activity_events: undefined, activity_journal_persisted: undefined,
    entries: [{ source: 'brush', text: 'old brush', partial_line: false }, { source: 'colmap', text: 'old colmap', partial_line: false }], next_cursor: 'old' }))
    .mockResolvedValueOnce(page([], { activity_events: undefined, activity_journal_persisted: undefined,
      entries: [{ source: 'brush', text: 'replacement brush', partial_line: false }], cursor_reset: true, truncated: true }));
  await render();
  await act(async () => vi.advanceTimersByTimeAsync(15000));
  expect(latest.rawLogs.map(value => value.text)).toEqual(['old colmap', 'replacement brush']);
  expect(latest.cursorReset).toBe(true);
  expect(latest.truncated).toBe(true);
  expect(latest.journalPersisted).toBe(false);
});

it('does not admit anonymous disk events as if they belonged to the new run', async () => {
  mocks.logs.mockResolvedValueOnce(page([{ ...event(7), taskId: undefined, runId: undefined }, event(8, 'old')]));
  const snapshotLegacy = { ...event(1), taskId: undefined, runId: undefined };
  await render({ events: [snapshotLegacy] });
  expect(latest.events.map(value => value.sequence)).toEqual([1]);
});

it('yields after eight pages then drains the remaining page without waiting a polling interval', async () => {
  let pageNumber = 0;
  mocks.logs.mockImplementation(async () => { pageNumber++; return page([event(pageNumber)], { has_more: pageNumber < 9, next_cursor: `page-${pageNumber}`, truncated: true }); });
  await render();
  expect(mocks.logs).toHaveBeenCalledTimes(8);
  await act(async () => vi.advanceTimersByTimeAsync(0));
  expect(mocks.logs).toHaveBeenCalledTimes(9);
  expect(latest.events).toHaveLength(9);
  expect(latest.truncated).toBe(true);
});

it('preserves lines and cursor at terminal transition, then stops polling', async () => {
  mocks.logs.mockResolvedValueOnce(page([event(1)], { next_cursor: 'first' }))
    .mockResolvedValueOnce(page([event(2)], { next_cursor: 'final' }));
  await render();
  await render({ active: false });
  expect(mocks.logs).toHaveBeenLastCalledWith('task', 'run', 'first');
  expect(latest.events.map(value => value.sequence)).toEqual([1, 2]);
  await act(async () => vi.advanceTimersByTimeAsync(60000));
  expect(mocks.logs).toHaveBeenCalledTimes(2);
});

it('serializes a terminal read behind an in-flight request and ignores its disposed result', async () => {
  let finish!: (value: TaskLogPage) => void;
  mocks.logs.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
    .mockResolvedValueOnce(page([event(2)]));
  await render();
  await render({ active: false, events: [event(1)] });
  expect(mocks.logs).toHaveBeenCalledOnce();
  await act(async () => finish(page([event(99)])));
  expect(mocks.logs).toHaveBeenCalledTimes(2);
  expect(latest.events.map(value => value.sequence)).toEqual([1, 2]);
});

it('isolates late reads from an old run and disposes all one-shot timers', async () => {
  let finish!: (value: TaskLogPage) => void;
  mocks.logs.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
    .mockResolvedValueOnce(page([event(1, 'new', 'new run')]));
  await render();
  await render({ run: 'new' });
  await act(async () => finish(page([event(1, 'run', 'old run')])));
  expect(latest.logs.map(value => value.text)).toEqual(['new run']);
  await act(async () => root.unmount()); root = createRoot(container);
  await act(async () => vi.advanceTimersByTimeAsync(60000));
  expect(mocks.logs).toHaveBeenCalledTimes(2);
});

it('accepts successful legacy completed log reads without a journal or persistence flag and stops polling', async () => {
  mocks.logs.mockResolvedValueOnce(page([], { activity_events: undefined, activity_journal_persisted: undefined,
    entries: [{ source: 'brush', text: 'legacy completion', partial_line: false }], next_cursor: null,
  }));
  await render({ active: false });
  expect(latest.unavailable).toBe(false);
  expect(latest.journalPersisted).toBe(false);
  expect(latest.rawLogs.map(value => value.text)).toEqual(['legacy completion']);
  await act(async () => vi.advanceTimersByTimeAsync(60000));
  expect(mocks.logs).toHaveBeenCalledOnce();
});

it('still reports real completed-run I/O failures, retries once per interval and stops after recovery', async () => {
  mocks.logs.mockRejectedValueOnce({ code: 'IO_ERROR' })
    .mockResolvedValueOnce(page([], { activity_events: undefined, activity_journal_persisted: undefined,
      entries: [{ source: 'brush', text: 'recovered completion', partial_line: false }], next_cursor: null,
    }));
  await render({ active: false });
  expect(latest.unavailable).toBe(true);
  await act(async () => vi.advanceTimersByTimeAsync(14999));
  expect(mocks.logs).toHaveBeenCalledOnce();
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(latest.unavailable).toBe(false);
  expect(latest.rawLogs.map(value => value.text)).toEqual(['recovered completion']);
  await act(async () => vi.advanceTimersByTimeAsync(60000));
  expect(mocks.logs).toHaveBeenCalledTimes(2);
});
