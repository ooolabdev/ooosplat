import type { PipelineEvent, PipelineResult, Quality } from './pipeline';
import { nextOverallPercent, retainOperationProgress } from '../lib/progressState';
import { mergeActivityEvents } from '../lib/taskActivity';
export type TaskStatus = 'created' | 'starting' | 'running' | 'cancelling' | 'completed' | 'failed' | 'cancelled' | 'interrupted';
export interface SharedTask {
  gps_assistance?: import('./inputs').GpsAssistanceSummary | null;
  imported_reconstruction?:boolean;
  generation_policy?: import('./pipeline').GenerationPolicy | null;
  automatically_retried?: boolean;
  operation?: import('./pipeline').PipelineOperation | null;
  unit?: string | null;
  substage?: string | null;
  task_id: string; run_id: string | null; project_id: string | null; project_path: string | null;
  project_deleted?: boolean; runtime?: import("./pipeline").RuntimeSnapshot | null;
  runs?: Array<{ run_id: string; kind: string }>;
  source_project_id?: string | null;
  input_path: string; input_type?: import('./pipeline').InputType; quality: Quality; source: 'gui' | 'mcp' | null; task_kind: 'generation' | 'reshoot';
  planner_enabled: boolean; projects_root: string; status: TaskStatus; stage: PipelineEvent["stage"] | null;
  revision: number; sequence: number; created_at: string; updated_at: string; elapsed_ms: number;
  progress: number | null; estimated_progress: number | null; current: number | null; total: number | null; eta_seconds: number | null;
  error: { code: string; message: string; failed_stage: PipelineEvent["stage"] | null; engine: string | null; exit_code: number | null; failure_id: string | null; classification: string | null; classification_is_heuristic: boolean; classification_version?: number | null; termination?: import('../lib/pipelineError').ProcessTermination | null } | null;
  result: PipelineResult | null; recent_events: PipelineEvent[];
  /** UI-only count; full engine logs remain available through the log reader. */
  dropped_event_count?: number;
  /** UI-only retained percentage for this run; never sent back to the backend. */
  display_progress?: number;
}
export interface TaskUpdate { task: SharedTask; events: PipelineEvent[]; dropped_event_count: number }
export interface StartReceipt { accepted: boolean; task_id: string; run_id: string | null; status: TaskStatus; revision: number }
export interface McpSettings { enabled: boolean; port: number; inputRoots: string[] }
export interface McpConnection { settings: McpSettings; listening: boolean; address: string | null; defaultInputRoot?: string | null; error: string | null }
export const taskIsActive = (task: Pick<SharedTask, 'status'>) => ['starting', 'running', 'cancelling'].includes(task.status);
/** Stop reasons do not select a different continuation flow; the backend validates saved work. */
export const taskCanContinue = (task: Pick<SharedTask, 'status' | 'project_deleted'>) =>
  !task.project_deleted && ['failed', 'cancelled', 'interrupted'].includes(task.status);

/** Revisions are monotonic across executions; a delayed snapshot cannot undo a newer event. */
export function mergeTask(tasks: Record<string, SharedTask>, incoming: SharedTask): Record<string, SharedTask> {
  const previous = tasks[incoming.task_id];
  const sameRun = previous != null && previous.run_id === incoming.run_id;
  if (sameRun && previous.operation && incoming.operation && previous.operation.id === incoming.operation.id
    && (previous.operation.sourceId ?? null) !== (incoming.operation.sourceId ?? null)) return tasks;
  const history = mergeActivityEvents(sameRun ? previous.recent_events : [], incoming.recent_events ?? [],
    { taskId: incoming.task_id, runId: incoming.run_id, allowUnbound: true });
  if ((previous?.revision ?? -1) >= incoming.revision) {
    // An old snapshot may fill a history gap, but cannot rewind status or
    // contribute events from an obsolete execution.
    return sameRun && history !== previous.recent_events
      ? { ...tasks, [incoming.task_id]: { ...previous, recent_events: history } } : tasks;
  }
  const observed = retainOperationProgress(sameRun ? { ...previous, taskId: previous.task_id, runId: previous.run_id, value: previous.progress } : null,
    { ...incoming, taskId: incoming.task_id, runId: incoming.run_id, value: incoming.progress });
  const task = {
    ...incoming,
    current: observed.current ?? null, total: observed.total ?? null, unit: observed.unit ?? null, progress: observed.value,
    display_progress: nextOverallPercent(sameRun ? previous.display_progress ?? previous.estimated_progress ?? 0 : 0,
      incoming.estimated_progress, incoming.status === 'completed'),
    recent_events: history,
    dropped_event_count: incoming.dropped_event_count ?? (sameRun ? previous.dropped_event_count : 0),
  };
  return { ...tasks, [incoming.task_id]: task };
}
