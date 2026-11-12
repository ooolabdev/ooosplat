import { expect, it } from 'vitest';
import { mergeActivityEvents } from './taskActivity';
import type { PipelineEvent } from '../types/pipeline';

const event = (sequence: number, message = 'same output', runId = 'run') => ({
  taskId: 'task', runId, sequence, message, kind: 'log',
} as PipelineEvent);

it('joins history by execution/sequence, not message, revision or operation', () => {
  const previous = [event(5), event(9)];
  const result = mergeActivityEvents(previous, [event(9), event(2), event(8, 'other run', 'old')], { taskId: 'task', runId: 'run' });
  expect(result.map(value => value.sequence)).toEqual([2, 5, 9]);
  expect(result.map(value => value.message)).toEqual(['same output', 'same output', 'same output']);
  expect(mergeActivityEvents(result, [event(5)], { taskId: 'task', runId: 'run' })).toBe(result);
});

it('keeps history bounded and never includes runtime or heartbeat snapshots', () => {
  const events = Array.from({ length: 800 }, (_, index) => event(index + 1));
  events.push({ ...event(801), kind: 'runtime' }, { ...event(802), kind: 'heartbeat' });
  const result = mergeActivityEvents([], events);
  expect(result).toHaveLength(500);
  expect(result[0].sequence).toBe(301);
  expect(result.at(-1)?.sequence).toBe(800);
  expect(mergeActivityEvents([], [event(1, 'x'.repeat(300_000))])).toEqual([]);
});

it('accepts anonymous legacy records only from an explicitly task-bound snapshot', () => {
  const legacy = { ...event(1), taskId: undefined, runId: undefined };
  expect(mergeActivityEvents([], [legacy], { taskId: 'task', runId: 'run' })).toEqual([]);
  expect(mergeActivityEvents([], [legacy], { taskId: 'task', runId: 'run', allowUnbound: true })).toEqual([legacy]);
});
