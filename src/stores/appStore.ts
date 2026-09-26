import { create } from "zustand";
import type { ColmapAccelerationStatus, EngineStatus, FramePlan, ImageSequenceInfo, InputType, PipelineEvent, PipelineResult, ProjectSummary, Quality, RunPhase, RuntimeEstimate, VideoInfo } from "../types/pipeline";

interface AppState {
  inputPath: string | null;
  inputType: InputType;
  projectsRoot: string;
  plannerEnabled: boolean;
  projects: ProjectSummary[];
  quality: Quality;
  colmapAcceleration: ColmapAccelerationStatus | null;
  video: VideoInfo | null;
  imageSequence: ImageSequenceInfo | null;
  plan: FramePlan | null;
  estimate: RuntimeEstimate | null;
  engines: EngineStatus[];
  phase: RunPhase;
  progress: number;
  progressMessage: string;
  latestEvent: PipelineEvent | null;
  events: PipelineEvent[];
  result: PipelineResult | null;
  error: string | null;
  setInputPath: (path: string | null, inputType: InputType) => void;
  setProjectsRoot: (path: string) => void;
  setPlannerEnabled: (enabled: boolean) => void;
  setProjects: (projects: ProjectSummary[]) => void;
  setQuality: (quality: Quality) => void;
  setColmapAcceleration: (acceleration: ColmapAccelerationStatus | null) => void;
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
  video: null,
  imageSequence: null,
  plan: null,
  estimate: null,
  engines: [],
  phase: "idle",
  progress: 0,
  progressMessage: "",
  latestEvent: null,
  events: [],
  result: null,
  error: null,
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
    events: [],
    result: null,
    error: null,
  }),
  setProjectsRoot: (projectsRoot) => set({ projectsRoot }),
  setPlannerEnabled: (plannerEnabled) => set((state) => state.plannerEnabled === plannerEnabled
    ? state
    : { plannerEnabled, plan: null, estimate: null, result: null, error: null }),
  setProjects: (projects) => set({ projects }),
  setQuality: (quality) => set({ quality, plan: null, estimate: null, result: null, error: null }),
  setColmapAcceleration: (colmapAcceleration) => set({ colmapAcceleration }),
  setAnalysis: (inputType, video, imageSequence, plan, estimate) => set({ inputType, video, imageSequence, plan, estimate }),
  setEstimate: (estimate) => set({ estimate }),
  setEngines: (engines) => set({ engines }),
  setPhase: (phase) => set({ phase }),
  beginRun: () => set({ phase: "running", progress: 0, progressMessage: "正在创建项目", latestEvent: null, events: [], result: null, error: null }),
  receiveEvent: (event) => set((state) => {
    if (state.latestEvent && event.sequence > 0 && event.sequence <= state.latestEvent.sequence) return state;
    const events = [...state.events, event].slice(-500);
    const terminal = event.stage === "failed" || event.stage === "cancelled";
    return {
      events,
      latestEvent: event,
      progress: terminal ? state.progress : Math.max(state.progress, Math.min(100, event.progress)),
      progressMessage: event.message,
      colmapAcceleration: event.acceleration ?? state.colmapAcceleration,
    };
  }),
  setResult: (result) => set({ result }),
  setError: (error) => set({ error }),
}));
