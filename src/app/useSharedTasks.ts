import { useCallback, useEffect, useRef, useState } from 'react';
import { getSharedTasks, onTaskUpdate } from '../lib/backend';
import { mergeTask, taskIsActive, type SharedTask } from '../types/tasks';
import { useAppStore } from '../stores/appStore';
import type { PipelineEvent } from '../types/pipeline';

export function useSharedTasks() {
  const [tasks, setTasks] = useState<Record<string, SharedTask>>({});
  const [syncError, setSyncError] = useState<string | null>(null);
  const refreshRef = useRef<() => Promise<void>>(async () => {});
  const refreshTasks = useCallback(() => refreshRef.current(), []);
  useEffect(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    let refreshing = false;
    let cache: Record<string, SharedTask> = {};
    let tracked: { task: string; run: string | null; revision: number } | undefined;
    const phase = (task: SharedTask, events: PipelineEvent[]) => {
      const state = useAppStore.getState();
      if (taskIsActive(task)) {
        const reset = tracked?.task !== task.task_id || tracked.run !== task.run_id || state.phase !== 'running';
        if (!reset && tracked && tracked.revision >= task.revision) return;
        state.receiveTaskBatch(task, events, reset);
        tracked = { task: task.task_id, run: task.run_id, revision: task.revision };
      } else if ((tracked?.task === task.task_id && tracked.run === task.run_id)
        || (state.liveTask?.task_id === task.task_id && state.liveTask.run_id === task.run_id)) {
        state.receiveTaskBatch(task, events, false);
        tracked = undefined;
      }
    };
    const apply = (task: SharedTask, batch: PipelineEvent[], dropped: number) => {
      if (disposed || (cache[task.task_id]?.revision ?? -1) >= task.revision) return;
      const events = batch.filter(event => (!event.taskId || event.taskId === task.task_id)
        && (!event.runId || event.runId === task.run_id)).sort((a, b) => a.sequence - b.sequence);
      const previous = cache[task.task_id];
      const sameRun = previous?.run_id === task.run_id;
      const history = sameRun ? previous.recent_events : [];
      const seen = new Set(history.map(event => event.sequence));
      const appended = events.filter(event => {
        if (['runtime', 'heartbeat'].includes(event.kind) || seen.has(event.sequence)) return false;
        seen.add(event.sequence);
        return true;
      });
      task = { ...task,
        recent_events: appended.length ? [...history, ...appended].sort((a, b) => a.sequence - b.sequence).slice(-500) : history,
        dropped_event_count: (sameRun ? previous.dropped_event_count ?? 0 : 0) + dropped,
      };
      cache = mergeTask(cache, task);
      setTasks(cache);
      phase(task, events);
    };
    const refresh = async () => {
      if (disposed || refreshing) return;
      refreshing = true;
      try {
        const snapshot = await getSharedTasks();
        if (disposed) return;
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
      } catch (error) { if (!disposed) setSyncError(error instanceof Error ? error.message : '无法同步任务状态'); }
      finally { refreshing = false; }
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
