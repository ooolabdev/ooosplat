import { create } from "zustand";
import type { ColmapAccelerationStatus, EngineStatus, FramePlan, ImageSequenceInfo, InputType, PipelineEvent, PipelineResult, ProjectSummary, Quality, RunPhase, RuntimeEstimate, RuntimeSnapshot, VideoInfo } from "../types/pipeline";
import type { SharedTask } from "../types/tasks";

interface AppState {
  inputPath: string | null;
  inputType: InputType;
  projectsRoot: string;
  plannerEnabled: boolean;
  projects: ProjectSummary[];
  quality: Quality;
  colmapAcceleration: ColmapAccelerationStatus | null;
  taskColmapAcceleration: ColmapAccelerationStatus | null;
  video: VideoInfo | null;
  imageSequence: ImageSequenceInfo | null;
  plan: FramePlan | null;
  estimate: RuntimeEstimate | null;
  engines: EngineStatus[];
  phase: RunPhase;
  progress: number;
  progressMessage: string;
  latestEvent: PipelineEvent | null;
  latestRuntime: RuntimeSnapshot | null;
  lastEventSequence: number;
  events: PipelineEvent[];
  liveTask: Pick<SharedTask, "task_id" | "run_id" | "status" | "elapsed_ms" | "updated_at"> | null;
  result: PipelineResult | null;
  error: string | null;
  errorAt: number | null;
  setInputPath: (path: string | null, inputType: InputType) => void;
  setProjectsRoot: (path: string) => void;
  setPlannerEnabled: (enabled: boolean) => void;
  setProjects: (projects: ProjectSummary[]) => void;
  setQuality: (quality: Quality) => void;
  setColmapAcceleration: (acceleration: ColmapAccelerationStatus | null) => void;
  setTaskColmapAcceleration: (acceleration: ColmapAccelerationStatus | null) => void;
  setAnalysis: (inputType: InputType, video: VideoInfo | null, imageSequence: ImageSequenceInfo | null, plan: FramePlan, estimate: RuntimeEstimate) => void;
  setEstimate: (estimate: RuntimeEstimate | null) => void;
  setEngines: (engines: EngineStatus[]) => void;
  setPhase: (phase: RunPhase) => void;
  beginRun: () => void;
  receiveEvent: (event: PipelineEvent) => void;
  receiveTaskBatch: (task: SharedTask, events: PipelineEvent[], reset: boolean) => void;
  setResult: (result: PipelineResult | null) => void;
  setError: (error: string | null) => void;
}

/** Reduce a notification in sequence order, allocating the log array only once. */
function reduceEvents(state: AppState, batch: PipelineEvent[]): AppState {
  let next = state;
  let logs: PipelineEvent[] | undefined;
  for (const event of batch) {
    if (event.taskId && next.latestEvent?.taskId === event.taskId && event.runId !== next.latestEvent.runId && (event.revision ?? 0) <= (next.latestEvent.revision ?? 0)) continue;
    const sameRun = !event.runId || (next.liveTask
      ? event.runId === next.liveTask.run_id && (!event.taskId || event.taskId === next.liveTask.task_id)
      : event.runId === next.latestEvent?.runId);
    if (sameRun && event.sequence > 0 && event.sequence <= Math.max(next.lastEventSequence, next.latestEvent?.sequence ?? 0)) continue;
    if (event.kind === "runtime") {
      if (next.phase !== "running" || ["completed", "failed", "cancelled"].includes(next.latestEvent?.stage ?? "")) continue;
      next = { ...next, latestRuntime: event.runtime ?? next.latestRuntime, lastEventSequence: event.sequence };
      continue;
    }
    logs ??= state.events.slice();
    logs.push(event);
    const terminal = event.stage === "failed" || event.stage === "cancelled";
    next = {
      ...next,
      latestEvent: event,
      lastEventSequence: event.sequence,
      progress: terminal ? next.progress : Math.max(next.progress, Math.min(100, event.progress)),
      progressMessage: event.message,
      taskColmapAcceleration: event.acceleration ?? next.taskColmapAcceleration,
    };
  }
  return logs ? { ...next, events: logs.slice(-500) } : next;
}

const newRunState = {
  phase: "running" as const, progress: 0, progressMessage: "正在创建项目", latestEvent: null,
  latestRuntime: null, lastEventSequence: 0, events: [] as PipelineEvent[], result: null,
  error: null, errorAt: null, taskColmapAcceleration: null, liveTask: null,
};

export const useAppStore = create<AppState>((set) => ({
  inputPath: null,
  inputType: "video",
  projectsRoot: "",
  plannerEnabled: true,
  projects: [],
  quality: "balanced",
  colmapAcceleration: null,
  taskColmapAcceleration: null,
  video: null,
  imageSequence: null,
  plan: null,
  estimate: null,
  engines: [],
  phase: "idle",
  progress: 0,
  progressMessage: "",
  latestEvent: null,
  latestRuntime: null,
  lastEventSequence: 0,
  events: [],
  liveTask: null,
  result: null,
  error: null,
  errorAt: null,
  setInputPath: (inputPath, inputType) => set({
    inputPath,
    inputType,
    video: null,
    imageSequence: null,
    plan: null,
    estimate: null,
    phase: "idle",
    progress: 0,
    progressMessage: "",
    latestEvent: null,
    latestRuntime: null,
    lastEventSequence: 0,
    events: [],
    liveTask: null,
    result: null,
    error: null,
    errorAt: null,
  }),
  setProjectsRoot: (projectsRoot) => set({ projectsRoot }),
  setPlannerEnabled: (plannerEnabled) => set((state) => state.plannerEnabled === plannerEnabled
    ? state
    : { plannerEnabled, plan: null, estimate: null, result: null, error: null, errorAt: null }),
  setProjects: (projects) => set({ projects }),
  setQuality: (quality) => set({ quality, plan: null, estimate: null, result: null, error: null, errorAt: null }),
  setColmapAcceleration: (colmapAcceleration) => set({ colmapAcceleration }),
  setTaskColmapAcceleration: (taskColmapAcceleration) => set({ taskColmapAcceleration }),
  setAnalysis: (inputType, video, imageSequence, plan, estimate) => set({ inputType, video, imageSequence, plan, estimate }),
  setEstimate: (estimate) => set({ estimate }),
  setEngines: (engines) => set({ engines }),
  setPhase: (phase) => set({ phase }),
  beginRun: () => set(newRunState),
  receiveEvent: (event) => set((state) => reduceEvents(state, [event])),
  receiveTaskBatch: (task, events, reset) => set((state) => {
    const ordered = events.filter(event => (!event.taskId || event.taskId === task.task_id)
      && (!event.runId || event.runId === task.run_id)).sort((a, b) => a.sequence - b.sequence);
    const next = reduceEvents(reset ? { ...state, ...newRunState } : state, ordered);
    return {
      ...next,
      phase: ["starting", "running", "cancelling"].includes(task.status) ? "running"
        : task.status === "completed" || task.status === "failed" || task.status === "cancelled" ? task.status : "idle",
      progress: task.estimated_progress == null ? next.progress : Math.max(next.progress, Math.min(100, task.estimated_progress)),
      latestRuntime: task.runtime === undefined ? next.latestRuntime : task.runtime,
      result: task.result ?? next.result,
      // Advance the snapshot watermark only after its earlier log entries are merged.
      lastEventSequence: Math.max(next.lastEventSequence, task.sequence ?? 0),
      liveTask: { task_id: task.task_id, run_id: task.run_id, status: task.status, elapsed_ms: task.elapsed_ms, updated_at: task.updated_at },
    };
  }),
  setResult: (result) => set({ result }),
  setError: (error) => set({ error, errorAt: error ? Date.now() : null }),
}));
