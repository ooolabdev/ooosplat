export type Quality = "fast" | "balanced" | "high";
export type ColmapAcceleration = "cpu" | "gpu";
export type EngineKind = "ffmpeg" | "ffprobe" | "colmap" | "brush";
export type RunPhase = "idle" | "analyzing" | "running" | "completed" | "failed" | "cancelled";
export type ProjectStatus = "running" | "completed" | "failed" | "cancelled" | "interrupted";

export interface EngineStatus { kind: EngineKind; path: string; exists: boolean; canStart: boolean; version: string | null; cpuOnly: boolean | null; gpuAvailable: boolean | null; detail: string; }
export interface VideoInfo { duration: number; width: number; height: number; fps: number; totalFrames: number; codec: string; rotation: number; }
export interface FramePlan { retentionRatio: number; samplingFps: number; estimatedFrames: number; }

export interface PipelineEvent {
  sequence: number;
  timestamp: string;
  kind: "stage" | "progress" | "log" | "heartbeat";
  level: "info" | "warning" | "error";
  stage: string;
  engine: "system" | "ffmpeg" | "colmap" | "brush" | null;
  progress: number;
  stageProgress: number | null;
  indeterminate: boolean;
  message: string;
  current: number | null;
  total: number | null;
  unit: string | null;
  elapsedMs: number;
}

export interface PipelineResult {
  projectId: string;
  projectPath: string;
  finalPly: string;
  fileSize: number;
  splatCount: number;
  inputImages: number;
  registeredImages: number;
  registeredRatio: number;
  points3d: number;
  durationMs: number;
  completedAt: string;
  warning: string | null;
  logsDirectory: string;
}

export interface ProjectSummary {
  id: string;
  name: string;
  status: ProjectStatus;
  projectPath: string;
  finalPly: string | null;
  fileSize: number | null;
  splatCount: number | null;
  createdAt: string;
  completedAt: string | null;
  durationMs: number | null;
  quality: Quality;
  sourceName: string;
  registeredRatio: number | null;
  points3d: number | null;
  failureMessage: string | null;
}

export interface ProjectOverview { projectsRoot: string; colmapAcceleration: ColmapAcceleration; projects: ProjectSummary[]; }
