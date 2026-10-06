import type { PipelineEvent, PipelineResult, Quality } from './pipeline';
export type TaskStatus = 'created' | 'starting' | 'running' | 'cancelling' | 'completed' | 'failed' | 'cancelled' | 'interrupted';
export interface SharedTask {
  task_id: string; run_id: string | null; project_id: string | null; project_path: string | null;
  input_path: string; quality: Quality; source: 'gui' | 'mcp'; task_kind: 'generation' | 'reshoot';
  planner_enabled: boolean; projects_root: string; status: TaskStatus; stage: PipelineEvent["stage"] | null;
  revision: number; sequence: number; created_at: string; updated_at: string; elapsed_ms: number;
  progress: number | null; estimated_progress: number | null; current: number | null; total: number | null; eta_seconds: number | null;
  error: { code: string; message: string; failed_stage: PipelineEvent["stage"] | null; engine: string | null; exit_code: number | null; failure_id: string | null; classification: string | null; classification_is_heuristic: boolean } | null;
  result: PipelineResult | null; recent_events: PipelineEvent[];
}
export interface TaskUpdate { task: SharedTask; event: PipelineEvent | null }
export interface StartReceipt { accepted: boolean; task_id: string; run_id: string | null; status: TaskStatus; revision: number }
export interface McpSettings { enabled: boolean; port: number; inputRoots: string[] }
export interface McpConnection { settings: McpSettings; listening: boolean; address: string | null; token: string | null; error: string | null }
export const taskIsActive = (task: Pick<SharedTask, 'status'>) => ['starting', 'running', 'cancelling'].includes(task.status);

/** Revisions are monotonic across executions; a delayed snapshot cannot undo a newer event. */
export function mergeTask(tasks: Record<string, SharedTask>, incoming: SharedTask): Record<string, SharedTask> {
  if ((tasks[incoming.task_id]?.revision ?? -1) >= incoming.revision) return tasks;
  return { ...tasks, [incoming.task_id]: incoming };
}
