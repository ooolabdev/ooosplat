import { expect, it } from 'vitest';
import { changesOperation, overallPercent, retainEventProgress, stagePercent, staleOperation } from './progressState';
import type { PipelineEvent } from '../types/pipeline';
const event = (id: number, kind: PipelineEvent['kind'] = 'stage') => ({ kind, stage: 'reconstructing', substage: 'panoramaReconstruction', operation: { id, attempt: id }, stageProgress: 100, indeterminate: false }) as PipelineEvent;
it('only lifecycle events switch operations; late process events cannot restore old state', () => {
  expect(changesOperation(event(1), event(2))).toBe(true);
  expect(changesOperation(event(1), { ...event(1, 'log'), substage: null })).toBe(false);
  expect(staleOperation(event(2), event(1, 'runtime'))).toBe(true);
  expect(staleOperation(event(1), event(2, 'runtime'))).toBe(true);
  expect(staleOperation(event(1), event(2))).toBe(false);
});
it('keeps unknown stage progress unknown and prevents rounding active tasks to 100%', () => {
  expect(stagePercent({ ...event(1), indeterminate: true })).toBeNull();
  expect(stagePercent({ ...event(1), stageProgress: null })).toBeNull();
  expect(overallPercent(99.99, false)?.toFixed(1)).toBe('99.9');
  expect(overallPercent(100, true)).toBe(100);
});

it('retains an observed percentage only for compatible empty updates of the same explicit operation', () => {
  const known = { ...event(1, 'progress'), taskId: 'task', runId: 'run', stage: 'extractingFeatures',
    substage: null, operation: { id: 1, attempt: 1, sourceId: 'images' }, current: 18, total: 90,
    unit: 'images', stageProgress: 20,
  } as PipelineEvent;
  const empty = { ...known, current: null, total: null, unit: null, stageProgress: null, indeterminate: true };
  expect(retainEventProgress(known, empty)).toMatchObject({ current: 18, total: 90, unit: 'images', stageProgress: 20, indeterminate: false });
  for (const next of [
    { ...empty, operation: { id: 2, attempt: 1, sourceId: 'images' } },
    { ...empty, runId: 'next' },
    { ...empty, operation: { id: 1, attempt: 1, sourceId: 'other' } },
    { ...empty, substage: 'different' },
    { ...empty, unit: 'matchingBatches' },
    { ...empty, current: 19, total: 90, unit: 'images' },
  ]) expect(retainEventProgress(known, next).stageProgress).toBeNull();
});

it('never invents reconstruction percentage from a registration counter', () => {
  const unknown = { ...event(1, 'progress'), substage: null, stageProgress: null, indeterminate: true,
    current: 90, total: 90, unit: 'registeredImages',
  } as PipelineEvent;
  expect(stagePercent(retainEventProgress(null, unknown))).toBeNull();
  const estimate = { ...unknown, stageProgress: 81, indeterminate: false };
  expect(stagePercent(retainEventProgress(null, estimate))).toBe(81);
});

it('rejects a reused operation ID from another source, including Stage, while accepting a new source operation', () => {
  const previous = { ...event(1), operation: { id: 1, attempt: 1, sourceId: 'first' } };
  for (const kind of ['stage', 'progress', 'runtime', 'log', 'heartbeat'] as const) {
    expect(staleOperation(previous, { ...event(1, kind), operation: { id: 1, attempt: 1, sourceId: 'other' } })).toBe(true);
  }
  expect(staleOperation(previous, { ...event(2), operation: { id: 2, attempt: 1, sourceId: 'other' } })).toBe(false);
});
