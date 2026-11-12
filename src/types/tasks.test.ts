import { expect, it } from 'vitest';
import { mergeTask, taskCanContinue, type SharedTask, type TaskStatus } from './tasks';
import type { PipelineEvent } from './pipeline';

const task = (task_id: string, run_id: string, revision: number, estimated_progress: number | null, status = 'running') =>
  ({ task_id, run_id, revision, estimated_progress, status, recent_events: [] }) as unknown as SharedTask;

it('retains display progress per task/run across snapshots and selection, without altering backend estimates', () => {
  let tasks = mergeTask({}, task('a', 'run1', 1, 63.8));
  tasks = mergeTask(tasks, task('b', 'run1', 1, null));
  tasks = mergeTask(tasks, task('a', 'run1', 2, null));
  expect(tasks.a.display_progress).toBe(63.8);
  expect(tasks.a.estimated_progress).toBeNull();
  expect(tasks.b.display_progress).toBe(0);
  tasks = mergeTask(tasks, task('a', 'run1', 3, 20));
  expect(tasks.a.display_progress).toBe(63.8);
  tasks = mergeTask(tasks, task('a', 'run2', 4, null));
  expect(tasks.a.display_progress).toBe(0);
  expect(mergeTask(tasks, task('a', 'run1', 3, 99))).toBe(tasks);
  tasks = mergeTask(tasks, task('a', 'run2', 5, 99.99));
  expect(tasks.a.display_progress).toBe(99.9);
  tasks = mergeTask(tasks, task('a', 'run2', 6, null, 'completed'));
  expect(tasks.a.display_progress).toBe(100);
});

it('never turns invalid progress into NaN or an unknown display value', () => {
  let tasks = mergeTask({}, task('a', 'run1', 1, NaN));
  expect(tasks.a.display_progress).toBe(0);
  tasks = mergeTask(tasks, task('a', 'run1', 2, Infinity));
  expect(tasks.a.display_progress).toBe(0);
});

it('fills snapshot history gaps even at an older revision without changing run status', () => {
  const event = (sequence: number, runId = 'run') => ({ sequence, taskId: 'a', runId, kind: 'log', message: `line ${sequence}` } as PipelineEvent);
  let tasks = mergeTask({}, { ...task('a', 'run', 20, 80, 'completed'), recent_events: [event(10)] });
  tasks = mergeTask(tasks, { ...task('a', 'run', 10, 20), recent_events: [event(2), event(10)] });
  expect(tasks.a.status).toBe('completed');
  expect(tasks.a.revision).toBe(20);
  expect(tasks.a.recent_events.map(value => value.sequence)).toEqual([2, 10]);
  tasks = mergeTask(tasks, { ...task('a', 'old-run', 19, 10), recent_events: [event(1, 'old-run')] });
  expect(tasks.a.recent_events.map(value => value.sequence)).toEqual([2, 10]);
});

it('keeps snapshot percentages through same-operation empty updates, but not new scopes or runs', () => {
  const initial = { ...task('a', 'run', 1, 35), stage: 'extractingFeatures', substage: null, progress: 20,
    current: 18, total: 90, unit: 'images', operation: { id: 1, attempt: 1, sourceId: 'photos' },
  } as SharedTask;
  let tasks = mergeTask({}, initial);
  tasks = mergeTask(tasks, { ...initial, revision: 2, progress: null, current: null, total: null, unit: null });
  expect(tasks.a).toMatchObject({ progress: 20, current: 18, total: 90, unit: 'images' });
  tasks = mergeTask(tasks, { ...initial, revision: 3, progress: null, operation: { id: 2, attempt: 1, sourceId: 'next' } });
  expect(tasks.a.progress).toBeNull();
  tasks = mergeTask(tasks, { ...initial, revision: 4, run_id: 'new-run', progress: null });
  expect(tasks.a.progress).toBeNull();
});

it.each(['failed', 'cancelled', 'interrupted'] as const)('offers one continuation flow for %s, including before project creation', status => {
  expect(taskCanContinue({ status })).toBe(true);
  expect(taskCanContinue({ status, project_deleted: true })).toBe(false);
});

it.each(['created', 'starting', 'running', 'cancelling', 'completed'] satisfies TaskStatus[])('does not continue a %s task', status => {
  expect(taskCanContinue({ status })).toBe(false);
});
