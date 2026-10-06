import { useEffect, useState } from 'react';
import { taskIsActive, type SharedTask } from '../types/tasks';

type ElapsedTask = Pick<SharedTask, 'run_id' | 'status' | 'elapsed_ms' | 'updated_at' | 'revision'>;

export function taskElapsedAt(task: ElapsedTask | null | undefined, now: number): number | null {
  if (!task?.run_id) return null;
  if (!taskIsActive(task)) return task.elapsed_ms;
  const updatedAt = Date.parse(task.updated_at);
  const sinceUpdate = Number.isFinite(updatedAt) ? Math.max(0, now - updatedAt) : 0;
  return task.elapsed_ms + sinceUpdate;
}

/** Keep the displayed elapsed time moving between backend task events. */
export function useTaskElapsed(task: ElapsedTask | null | undefined): number | null {
  const active = task ? taskIsActive(task) && task.run_id != null : false;
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    setNow(Date.now());
    if (!active) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [active, task?.revision, task?.run_id]);

  return taskElapsedAt(task, now);
}
