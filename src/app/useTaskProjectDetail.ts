import { useEffect, useState } from 'react';
import { getProjectTaskDetail } from '../lib/backend';
import type { ProjectTaskDetail } from '../types/pipeline';
import type { SharedTask } from '../types/tasks';

/** Reading an existing checkpoint is cheap; never reprobe or replan submitted input. */
export function useTaskProjectDetail(task?: SharedTask | null) {
  const projectId = task?.project_deleted ? null : task?.project_id;
  const key = projectId ? `${projectId}:${task?.run_id}:${task?.status}:${task?.stage}` : null;
  const [loaded, setLoaded] = useState<{ key: string; detail: ProjectTaskDetail | null }>({ key: '', detail: null });
  useEffect(() => {
    if (!projectId || !key) return;
    let disposed = false;
    void getProjectTaskDetail(projectId).then(detail => {
      if (!disposed && detail?.project.id === projectId) setLoaded({ key, detail });
    }).catch(() => { if (!disposed) setLoaded({ key, detail: null }); });
    return () => { disposed = true; };
  }, [key, projectId]);
  return loaded.key === key ? loaded.detail : null;
}
