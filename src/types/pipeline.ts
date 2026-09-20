export type Quality = "fast" | "balanced" | "high";
export type EngineKind = "ffmpeg" | "ffprobe" | "colmap" | "brush";
export type RunPhase = "idle" | "analyzing" | "running" | "completed" | "failed" | "cancelled";
export type ProjectStatus = "running" | "completed" | "failed" | "cancelled" | "interrupted";

export type ColmapBackend = "cpu" | "gpu";
export type AccelerationReasonCode =
  | "gpuReady" | "macOsCpuOnly" | "colmapUnavailable" | "colmapCudaUnavailable" | "requirementsUnavailable"
  | "nvidiaSmiNotFound" | "probeFailed" | "probeTimeout" | "noNvidiaGpu"
  | "driverVersionUnknown" | "driverTooOld" | "computeCapabilityUnknown"
  | "computeCapabilityTooLow";
export interface GpuDeviceInfo { index: number; name: string; driverVersion: string; computeCapability: string; totalMemoryMb?: number; }
export interface AccelerationRequirements { minimumDriverVersion: string; minimumComputeCapability: string; }
export interface ColmapAccelerationStatus {
  backend: ColmapBackend;
  reasonCode: AccelerationReasonCode;
  reason: string;
  device: GpuDeviceInfo | null;
  requirements: AccelerationRequirements;
}
export interface EngineStatus { kind: EngineKind; path: string; exists: boolean; canStart: boolean; version: string | null; cpuOnly: boolean | null; acceleration: ColmapAccelerationStatus | null; colmapCliFamily?: "legacy39" | "modern4"; detail: string; }
export interface VideoInfo {
  duration: number;
  width: number;
  height: number;
  fps: number;
  totalFrames: number;
  codec: string;
  rotation: number;
  pixelFormat: string;
  hasAlpha: boolean;
}
export type InputType = "video" | "images";
export interface ImageSequenceInfo {
  imageCount: number;
  width: number;
  height: number;
  hasAlpha: boolean;
  requiresLargeSequenceConfirmation: boolean;
}
export interface PlannedFrame { sourceFrameIndex: number; timestampSeconds: number; }
export interface FramePlan {
  quality?: Quality | null;
  retentionRatio: number;
  samplingFps: number;
  actualAverageFps?: number;
  targetFps?: number;
  candidateFps?: number;
  estimatedFrames: number;
  planningMode?: "legacy" | "budgeted";
  preferredFps?: number;
  selectedFrames?: PlannedFrame[];
  candidateFrames?: PlannedFrame[];
  shortCapture?: boolean;
  minimumRequiredFps?: number;
  minimumFrameTarget?: number;
  minimumFrameOverrideApplied?: boolean;
  minimumFrameTargetUnreachable?: boolean;
  selectedFramesBeforeFilter?: number;
  selectedFramesAfterFilter?: number;
  backfilledForMinimumCount?: number;
}
export interface RuntimeEstimate {
  estimatedMs: number;
  lowerBoundMs: number;
  upperBoundMs: number;
  confidence: "low" | "medium" | "high";
  sampleCount: number;
  basis: string;
}
export interface ProbeAndPlan {
  inputType: InputType;
  video: VideoInfo | null;
  imageSequence: ImageSequenceInfo | null;
  plan: FramePlan;
  estimate: RuntimeEstimate;
}

export interface PipelineEvent {
  sequence: number;
  timestamp: string;
  kind: "stage" | "progress" | "log" | "heartbeat" | "capability";
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
  acceleration: ColmapAccelerationStatus | null;
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
  qualityMetrics: QualityRunMetrics;
  durationMs: number;
  completedAt: string;
  warning: string | null;
  logsDirectory: string;
}

export interface QualityRunMetrics {
  actualFrameCount: number;
  actualSfmResolution: number;
  actualFeatureCount: number | null;
  actualBrushResolution: number;
  actualBrushIterations: number;
  registeredImages: number;
  reprojectionError: number | null;
  splatCount: number;
  stageDurationsMs: Record<string, number>;
  peakGpuMemoryMb: number | null;
  plannerEnabled: boolean;
  plannerVersion: number | null;
  captureType: string | null;
  pairingPlanned: string | null;
  pairingActual: string | null;
  mapperPlanned: string | null;
  mapperActual: string | null;
  largestComponentRatio: number | null;
  twoCoreRatio: number | null;
  bridgeRatio: number | null;
  normalRescueRounds: number;
  successRecoveryRounds: number;
  normalBudgetExhausted: boolean;
  successRecoveryEntered: boolean;
  budgetOverriddenForSuccess: boolean;
  normalDurationMs: number;
  recoveryDurationMs: number;
  reconstructionQuality: string | null;
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

export interface ProjectOverview { projectsRoot: string; projects: ProjectSummary[]; }
export interface AppRuntimeStatus { pipelineRunning: boolean; previewProjectId: string | null; }

export type GaussianFormat = "ply" | "sog" | "spz";
export interface GaussianTransform {
  position: [number, number, number];
  rotation: [number, number, number];
  scale: number;
}
export type GaussianEditorTool = "transform" | "rectangle" | "sphere" | "box";
export type GaussianOrthographicView = "side" | "front" | "top";
export type GaussianCrop =
  | { kind: "sphere"; center: [number, number, number]; radius: number }
  | { kind: "box"; center: [number, number, number]; size: [number, number, number] }
  | null;
export interface GaussianEditState {
  crop: GaussianCrop;
  revision: number;
  sourceSplatCount: number;
  deletedCount: number;
}
export interface GaussianEditSaveSession {
  editId: string;
  expectedMaskBytes: number;
}
export interface GaussianPreviewDescriptor {
  projectId: string;
  modelPath: string;
  assetPath: string;
  format: GaussianFormat;
  fileSize: number;
  splatCount: number;
  transform: GaussianTransform;
  editing: GaussianEditState;
  editMaskAssetPath: string | null;
}
export interface GaussianExportProgress {
  projectId: string;
  processedSplats: number;
  totalSplats: number;
  progress: number;
}
export interface GaussianExportResult {
  path: string;
  fileSize: number;
  splatCount: number;
}

export interface GaussianVideoExportSession {
  exportId: string;
  destinationPath: string;
}

export interface GaussianVideoExportResult {
  path: string;
  fileSize: number;
  width: number;
  height: number;
  fps: number;
  durationMs: number;
}
