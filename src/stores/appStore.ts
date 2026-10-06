import { create } from "zustand";
import type { ColmapAccelerationStatus, EngineStatus, FramePlan, ImageSequenceInfo, InputType, PipelineEvent, PipelineResult, ProjectSummary, Quality, RunPhase, RuntimeEstimate, RuntimeSnapshot, VideoInfo } from "../types/pipeline";

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
  setResult: (result: PipelineResult | null) => void;
  setError: (error: string | null) => void;
}

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
  beginRun: () => set({ phase: "running", progress: 0, progressMessage: "正在创建项目", latestEvent: null, latestRuntime: null, lastEventSequence: 0, events: [], result: null, error: null, errorAt: null, taskColmapAcceleration: null }),
  receiveEvent: (event) => set((state) => {
    if (event.taskId && state.latestEvent?.taskId === event.taskId && event.runId !== state.latestEvent.runId && (event.revision ?? 0) <= (state.latestEvent.revision ?? 0)) return state;
    const sameRun = !event.runId || event.runId === state.latestEvent?.runId;
    if (sameRun && event.sequence > 0 && event.sequence <= Math.max(state.lastEventSequence, state.latestEvent?.sequence ?? 0)) return state;
    if (event.kind === "runtime") {
      if (state.phase !== "running" || ["completed", "failed", "cancelled"].includes(state.latestEvent?.stage ?? "")) return state;
      return { latestRuntime: event.runtime ?? state.latestRuntime, lastEventSequence: event.sequence };
    }
    const events = [...state.events, event].slice(-500);
    const terminal = event.stage === "failed" || event.stage === "cancelled";
    return {
      events,
      latestEvent: event,
      lastEventSequence: event.sequence,
      progress: terminal ? state.progress : Math.max(state.progress, Math.min(100, event.progress)),
      progressMessage: event.message,
      taskColmapAcceleration: event.acceleration ?? state.taskColmapAcceleration,
    };
  }),
  setResult: (result) => set({ result }),
  setError: (error) => set({ error, errorAt: error ? Date.now() : null }),
}));
