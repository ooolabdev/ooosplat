import { create } from "zustand";
import type { ColmapAccelerationStatus, EngineStatus, FramePlan, PipelineEvent, PipelineResult, ProjectSummary, Quality, RunPhase, VideoInfo } from "../types/pipeline";

interface AppState {
  videoPath: string | null;
  projectsRoot: string;
  projects: ProjectSummary[];
  quality: Quality;
  colmapAcceleration: ColmapAccelerationStatus | null;
  video: VideoInfo | null;
  plan: FramePlan | null;
  engines: EngineStatus[];
  phase: RunPhase;
  progress: number;
  progressMessage: string;
  latestEvent: PipelineEvent | null;
  events: PipelineEvent[];
  result: PipelineResult | null;
  error: string | null;
  setVideoPath: (path: string | null) => void;
  setProjectsRoot: (path: string) => void;
  setProjects: (projects: ProjectSummary[]) => void;
  setQuality: (quality: Quality) => void;
  setColmapAcceleration: (acceleration: ColmapAccelerationStatus | null) => void;
  setAnalysis: (video: VideoInfo, plan: FramePlan) => void;
  setEngines: (engines: EngineStatus[]) => void;
  setPhase: (phase: RunPhase) => void;
  beginRun: () => void;
  receiveEvent: (event: PipelineEvent) => void;
  setResult: (result: PipelineResult | null) => void;
  setError: (error: string | null) => void;
}

export const useAppStore = create<AppState>((set) => ({
  videoPath: null,
  projectsRoot: "",
  projects: [],
  quality: "balanced",
  colmapAcceleration: null,
  video: null,
  plan: null,
  engines: [],
  phase: "idle",
  progress: 0,
  progressMessage: "",
  latestEvent: null,
  events: [],
  result: null,
  error: null,
  setVideoPath: (videoPath) => set({ videoPath, video: null, plan: null, result: null, error: null, progress: 0, phase: "idle" }),
  setProjectsRoot: (projectsRoot) => set({ projectsRoot }),
  setProjects: (projects) => set({ projects }),
  setQuality: (quality) => set({ quality, plan: null, result: null, error: null }),
  setColmapAcceleration: (colmapAcceleration) => set({ colmapAcceleration }),
  setAnalysis: (video, plan) => set({ video, plan }),
  setEngines: (engines) => set({ engines }),
  setPhase: (phase) => set({ phase }),
  beginRun: () => set({ phase: "running", progress: 0, progressMessage: "正在创建项目", latestEvent: null, events: [], result: null, error: null }),
  receiveEvent: (event) => set((state) => {
    if (state.latestEvent && event.sequence > 0 && event.sequence <= state.latestEvent.sequence) return state;
    const events = [...state.events, event].slice(-500);
    return {
      events,
      latestEvent: event,
      progress: Math.max(state.progress, Math.min(100, event.progress)),
      progressMessage: event.message,
      colmapAcceleration: event.acceleration ?? state.colmapAcceleration,
    };
  }),
  setResult: (result) => set({ result }),
  setError: (error) => set({ error }),
}));
