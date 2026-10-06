import { useEffect, useState } from 'react';
import { getSharedTasks, onTaskUpdate } from '../lib/backend';
import { mergeTask, taskIsActive, type SharedTask } from '../types/tasks';
import { useAppStore } from '../stores/appStore';

export function useSharedTasks() {
  const [tasks, setTasks] = useState<Record<string, SharedTask>>({});
  const [syncError, setSyncError] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    let refreshing = false;
    let cache: Record<string, SharedTask> = {};
    let tracked: { task: string; run: string | null } | undefined;
    const phase = (task: SharedTask, event: import('../types/pipeline').PipelineEvent | null) => {
      const state = useAppStore.getState();
      if (taskIsActive(task)) {
        if (tracked?.run !== task.run_id || state.phase !== 'running') state.beginRun();
        tracked = { task: task.task_id, run: task.run_id };
        if (event) state.receiveEvent(event);
      } else if (tracked?.task === task.task_id && tracked.run === task.run_id) {
        state.setPhase(task.status === 'completed' || task.status === 'failed' || task.status === 'cancelled' ? task.status : 'idle');
        if (task.result) state.setResult(task.result);
        if (task.error && task.status === 'failed') state.setError(task.error.message);
        tracked = undefined;
      }
    };
    const apply = (task: SharedTask, event: import('../types/pipeline').PipelineEvent | null) => {
      if (disposed || (cache[task.task_id]?.revision ?? -1) >= task.revision) return;
      cache = mergeTask(cache, task);
      setTasks(cache);
      phase(task, event);
    };
    const refresh = async () => {
      if (disposed || refreshing) return;
      refreshing = true;
      try {
        const snapshot = await getSharedTasks();
        if (disposed) return;
        for (const task of snapshot) cache = mergeTask(cache, task);
        setTasks(cache);
        const active = Object.values(cache).find(taskIsActive);
        if (active) {
          phase(active, null);
          for (const event of active.recent_events) useAppStore.getState().receiveEvent(event);
        } else if (tracked && cache[tracked.task]) phase(cache[tracked.task], null);
        if (!disposed) setSyncError(null);
      } catch (error) { if (!disposed) setSyncError(error instanceof Error ? error.message : '无法同步任务状态'); }
      finally { refreshing = false; }
    };
    void (async () => {
      // Subscribe before reading a snapshot, and clean up even if unmounted during subscription.
      const fn = await onTaskUpdate(update => {
        const previous = cache[update.task.task_id];
        apply(update.task, update.event);
        if (previous && update.task.revision > previous.revision + 1) void refresh();
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
  return { tasks, syncError };
}
