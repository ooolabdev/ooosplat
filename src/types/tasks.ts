import type { PipelineEvent, PipelineResult, Quality } from './pipeline';
export type TaskStatus = 'created' | 'starting' | 'running' | 'cancelling' | 'completed' | 'failed' | 'cancelled' | 'interrupted';
export interface SharedTask {
  task_id: string; run_id: string | null; project_id: string | null; project_path: string | null;
  project_deleted?: boolean; runtime?: import("./pipeline").RuntimeSnapshot | null;
  runs?: Array<{ run_id: string; kind: string }>;
  source_project_id?: string | null;
  input_path: string; input_type?: import('./pipeline').InputType; quality: Quality; source: 'gui' | 'mcp' | null; task_kind: 'generation' | 'reshoot';
  planner_enabled: boolean; projects_root: string; status: TaskStatus; stage: PipelineEvent["stage"] | null;
  revision: number; sequence: number; created_at: string; updated_at: string; elapsed_ms: number;
  progress: number | null; estimated_progress: number | null; current: number | null; total: number | null; eta_seconds: number | null;
  error: { code: string; message: string; failed_stage: PipelineEvent["stage"] | null; engine: string | null; exit_code: number | null; failure_id: string | null; classification: string | null; classification_is_heuristic: boolean } | null;
  result: PipelineResult | null; recent_events: PipelineEvent[];
  /** UI-only count; full engine logs remain available through the log reader. */
  dropped_event_count?: number;
}
export interface TaskUpdate { task: SharedTask; events: PipelineEvent[]; dropped_event_count: number }
export interface StartReceipt { accepted: boolean; task_id: string; run_id: string | null; status: TaskStatus; revision: number }
export interface McpSettings { enabled: boolean; port: number; inputRoots: string[] }
export interface McpConnection { settings: McpSettings; listening: boolean; address: string | null; defaultInputRoot?: string | null; error: string | null }
export const taskIsActive = (task: Pick<SharedTask, 'status'>) => ['starting', 'running', 'cancelling'].includes(task.status);

/** Revisions are monotonic across executions; a delayed snapshot cannot undo a newer event. */
export function mergeTask(tasks: Record<string, SharedTask>, incoming: SharedTask): Record<string, SharedTask> {
  if ((tasks[incoming.task_id]?.revision ?? -1) >= incoming.revision) return tasks;
  const previous = tasks[incoming.task_id];
  const sameRun = previous?.run_id === incoming.run_id;
  const task = {
    ...incoming,
    recent_events: sameRun && incoming.recent_events.length === 0 ? previous.recent_events : incoming.recent_events,
    dropped_event_count: incoming.dropped_event_count ?? (sameRun ? previous.dropped_event_count : 0),
  };
  return { ...tasks, [incoming.task_id]: task };
}
