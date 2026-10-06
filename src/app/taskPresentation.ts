import type { Locale, TranslationKey } from '../i18n';
import type { SharedTask, TaskStatus } from '../types/tasks';
import type { ProjectSummary } from '../types/pipeline';
export function taskStatusLabel(locale: Locale, status: TaskStatus): string {
  const labels: Record<TaskStatus, string> = locale === 'zh-CN'
    ? { created: '待启动', starting: '正在启动', running: '运行中', cancelling: '正在取消', completed: '已完成', failed: '失败', cancelled: '已取消', interrupted: '已中断' }
    : { created: 'Ready', starting: 'Starting', running: 'Running', cancelling: 'Cancelling', completed: 'Completed', failed: 'Failed', cancelled: 'Cancelled', interrupted: 'Interrupted' };
  return labels[status];
}
export const taskStageLabels: Record<string, TranslationKey> = {
  created: 'stage.preparing', probingVideo: 'stage.material', planningFrames: 'stage.material',
  extractingFrames: 'stage.frames', extractingFeatures: 'stage.features', matching: 'stage.matching',
  reconstructing: 'stage.reconstruction', validatingReconstruction: 'stage.reconstruction',
  trainingSplats: 'stage.training', exporting: 'stage.export',
  completed: 'stage.completed', failed: 'stage.failed', cancelled: 'stage.cancelled',
};

export const taskStages = [
  ['probingVideo', 'stage.material'], ['extractingFrames', 'stage.frames'],
  ['extractingFeatures', 'stage.features'], ['matching', 'stage.matching'],
  ['reconstructing', 'stage.reconstruction'], ['trainingSplats', 'stage.training'],
  ['exporting', 'stage.export'],
] as const;

export function taskStagePosition(stage?: string | null) {
  if (stage === 'validatingReconstruction') return 4;
  if (stage === 'completed') return 6;
  const index = taskStages.findIndex(([key]) => key === stage);
  return index < 0 ? 0 : index;
}

export const taskBasename = (path: string) => path.split(/[\\/]/).at(-1) ?? path;
export const taskParentPath = (path: string) => path.replace(/[\\/][^\\/]+[\\/]?$/, '') || path;
export function taskFormatBytes(bytes: number | null, locale: string) {
  if (bytes == null) return '—';
  const [value, unit, digits] = bytes >= 1024 ** 3 ? [bytes / 1024 ** 3, 'GB', 2] as const
    : bytes >= 1024 ** 2 ? [bytes / 1024 ** 2, 'MB', 1] as const : [bytes / 1024, 'KB', 1] as const;
  return `${new Intl.NumberFormat(locale, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(value)} ${unit}`;
}

/** Project metadata enriches a task; it never overrides the service's lifecycle. */
export function taskProjectSummary(task: SharedTask, project?: ProjectSummary | null): ProjectSummary | null {
  if (!task.project_id || task.project_deleted) return null;
  const result = task.result;
  const path = task.project_path ?? project?.projectPath ?? result?.projectPath;
  if (!path) return null;
  return {
    id: task.project_id, workspaceTaskId: task.task_id, taskKind: task.task_kind,
    name: project?.name ?? taskBasename(path),
    status: ['created', 'starting', 'running', 'cancelling'].includes(task.status) ? 'running' : task.status as ProjectSummary['status'],
    projectPath: path, finalPly: result?.finalPly ?? project?.finalPly ?? null,
    fileSize: result?.fileSize ?? project?.fileSize ?? null,
    splatCount: result?.splatCount ?? project?.splatCount ?? null,
    createdAt: project?.createdAt ?? task.created_at,
    completedAt: result?.completedAt ?? project?.completedAt ?? null,
    durationMs: result?.durationMs ?? project?.durationMs ?? task.elapsed_ms,
    quality: task.quality, sourceName: taskBasename(task.input_path),
    registeredRatio: result?.registeredRatio ?? project?.registeredRatio ?? null,
    points3d: result?.points3d ?? project?.points3d ?? null,
    failureMessage: task.error?.message ?? null,
  };
}
