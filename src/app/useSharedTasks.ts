import { useCallback, useEffect, useRef, useState } from 'react';
import { getSharedTasks, onTaskUpdate } from '../lib/backend';
import { mergeTask, taskIsActive, type SharedTask } from '../types/tasks';
import { useAppStore } from '../stores/appStore';
import type { PipelineEvent } from '../types/pipeline';
import { mergeActivityEvents } from '../lib/taskActivity';

interface TaskRefreshSummary { hasActiveTask: boolean }

export function useSharedTasks() {
  const [tasks, setTasks] = useState<Record<string, SharedTask>>({});
  const [syncError, setSyncError] = useState<string | null>(null);
  const refreshRef = useRef<() => Promise<TaskRefreshSummary | null>>(async () => null);
  const refreshTasks = useCallback(() => refreshRef.current(), []);
  useEffect(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    let refreshing: Promise<TaskRefreshSummary | null> | null = null;
    let cache: Record<string, SharedTask> = {};
    let tracked: { task: string; run: string | null; revision: number } | undefined;
    const phase = (task: SharedTask, events: PipelineEvent[]) => {
      const state = useAppStore.getState();
      if (taskIsActive(task)) {
        const reset = tracked?.task !== task.task_id || tracked.run !== task.run_id || state.phase !== 'running';
        if (!reset && tracked && tracked.revision > task.revision) return;
        state.receiveTaskBatch(task, events, reset);
        tracked = { task: task.task_id, run: task.run_id, revision: task.revision };
      } else if ((tracked?.task === task.task_id && tracked.run === task.run_id)
        || (state.liveTask?.task_id === task.task_id && state.liveTask.run_id === task.run_id)) {
        state.receiveTaskBatch(task, events, false);
        tracked = undefined;
      }
    };
    const apply = (task: SharedTask, batch: PipelineEvent[], dropped: number) => {
      if (disposed) return;
      const events = batch.filter(event => (!event.taskId || event.taskId === task.task_id)
        && (!event.runId || event.runId === task.run_id)).sort((a, b) => a.sequence - b.sequence);
      const previous = cache[task.task_id];
      const sameRun = previous?.run_id === task.run_id;
      task = { ...task,
        recent_events: mergeActivityEvents(sameRun ? previous.recent_events : [], [...(task.recent_events ?? []), ...events],
          { taskId: task.task_id, runId: task.run_id, allowUnbound: true }),
        dropped_event_count: (sameRun ? previous.dropped_event_count ?? 0 : 0)
          + ((previous?.revision ?? -1) < task.revision ? dropped : 0),
      };
      const merged = mergeTask(cache, task);
      if (merged === cache) return;
      cache = merged;
      setTasks(cache);
      const current = cache[task.task_id];
      phase(current, [...current.recent_events, ...events]);
    };
    const refresh = async () => {
      if (disposed) return null;
      if (refreshing) return refreshing;
      const pending = (async (): Promise<TaskRefreshSummary | null> => {
        try {
          const snapshot = await getSharedTasks();
          if (disposed) return null;
          const before = cache;
          for (const task of snapshot) cache = mergeTask(cache, task);
          if (before !== cache) setTasks(cache);
          const active = Object.values(cache).find(taskIsActive);
          if (active) {
            phase(active, active.recent_events);
          } else {
            const live = useAppStore.getState().liveTask;
            const previous = tracked ? cache[tracked.task] : live ? cache[live.task_id] : undefined;
            const run = tracked?.run ?? live?.run_id;
            if (previous && previous.run_id === run) phase(previous, previous.recent_events);
            else if (tracked || live) {
              useAppStore.setState({ phase: 'idle', liveTask: null });
              tracked = undefined;
            }
          }
          if (!disposed) setSyncError(null);
          return { hasActiveTask: Boolean(active) };
        } catch (error) {
          if (!disposed) setSyncError(error instanceof Error ? error.message : '无法同步任务状态');
          return null;
        }
      })();
      refreshing = pending;
      try { return await pending; }
      finally { if (refreshing === pending) refreshing = null; }
    };
    refreshRef.current = refresh;
    void (async () => {
      // Subscribe before reading a snapshot, and clean up even if unmounted during subscription.
      const fn = await onTaskUpdate(update => {
        // Revisions can legitimately jump because runtime/progress are coalesced.
        apply(update.task, update.events, update.dropped_event_count);
      });
      if (disposed) { fn(); return; }
      unsubscribe = fn;
      await refresh();
    })().catch(() => { if (!disposed) setSyncError('无法订阅任务状态'); });
    const timer = window.setInterval(() => void refresh(), 15000);
    const focus = () => void refresh();
    window.addEventListener('focus', focus);
    document.addEventListener('visibilitychange', focus);
    return () => { disposed = true; unsubscribe?.(); window.clearInterval(timer); window.removeEventListener('focus', focus); document.removeEventListener('visibilitychange', focus); };
  }, []);
  return { tasks, syncError, refreshTasks };
}
