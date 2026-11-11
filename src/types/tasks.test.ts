import { expect, it } from 'vitest';
import { mergeTask, taskCanContinue, type SharedTask, type TaskStatus } from './tasks';

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

it.each(['failed', 'cancelled', 'interrupted'] as const)('offers one continuation flow for %s, including before project creation', status => {
  expect(taskCanContinue({ status })).toBe(true);
  expect(taskCanContinue({ status, project_deleted: true })).toBe(false);
});

it.each(['created', 'starting', 'running', 'cancelling', 'completed'] satisfies TaskStatus[])('does not continue a %s task', status => {
  expect(taskCanContinue({ status })).toBe(false);
});
