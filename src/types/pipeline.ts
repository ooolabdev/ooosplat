export type Quality = "fast" | "balanced" | "high";
export interface GenerationPolicy { version: 1 | 2 | 3 | 4 | 5 | 6 | 7; autoRetryEnabled: boolean; frameSamplingRatio: number | null; frameSamplingFps?:number|null; }
export type EngineKind = "ffmpeg" | "ffprobe" | "colmap" | "brush";
export type RunPhase = "idle" | "analyzing" | "running" | "completed" | "failed" | "cancelled";
export type ProjectStatus = "running" | "completed" | "failed" | "cancelled" | "interrupted";

export type ColmapBackend = "cpu" | "gpu";
export type GpuDetectionState = "ready" | "temporarilyUnavailable" | "unavailable";
export type AccelerationReasonCode =
  | "gpuReady" | "macOsCpuOnly" | "colmapUnavailable" | "colmapCudaUnavailable" | "requirementsUnavailable"
  | "nvidiaSmiNotFound" | "probeFailed" | "probeTimeout" | "noNvidiaGpu"
  | "driverVersionUnknown" | "driverTooOld" | "computeCapabilityUnknown"
  | "computeCapabilityTooLow";
export interface GpuDeviceInfo { index: number; name: string; driverVersion: string; computeCapability: string; totalMemoryMb?: number; }
export interface AccelerationRequirements { minimumDriverVersion: string; minimumComputeCapability: string; }
export interface ColmapAccelerationStatus {
  backend: ColmapBackend;
  detectionState: GpuDetectionState;
  reasonCode: AccelerationReasonCode;
  reason: string;
  device: GpuDeviceInfo | null;
  requirements: AccelerationRequirements;
  detectedNvidiaDeviceCount?: number;
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
  retentionRatio: number;
  samplingFps: number;
  estimatedFrames: number;
  selectedFrames?: PlannedFrame[];
  candidateFrames?: PlannedFrame[];
  rescueMaxFrames?: number;
  bridgeSourceFrames?: number | null;
  minimumFrameOverrideApplied?: boolean;
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
  projectionDetection?: ProjectionDetection | null;
  inputType: InputType;
  video: VideoInfo | null;
  imageSequence: ImageSequenceInfo | null;
  plan: FramePlan;
  estimate: RuntimeEstimate;
}
export interface VideoAnalysis {
  video: VideoInfo;
  projectionDetection: ProjectionDetection;
}
export interface InputAnalysis {
  inputType: InputType;
  video: VideoInfo | null;
  imageSequence: ImageSequenceInfo | null;
  projectionDetection: ProjectionDetection;
}
export interface PipelineOperation { id: number; attempt: number; sourceId?:string|null; }
export type VideoProjection = "auto" | "perspective" | "equirectangular";
export type ImageMatchingStrategy = "sequential" | "exhaustive";
export interface ProjectionDetection { sourceIdentity?: string; unsupported?: boolean; confirmedErp: boolean; requiresChoice: boolean; supportedErp: boolean; reason: string; previewJpeg: number[]; }

export type ReshootCondition = "good" | "appearance" | "geometry" | "unsure";
export type ReshootCameraRelation = "same" | "different" | "unsure";
export interface ReshootOptions {
  condition: ReshootCondition;
  cameraRelation?: ReshootCameraRelation;
  allowFallback: boolean;
  sourceIdentity: string;
  inputIdentity: string;
  projection: "perspective" | "equirectangular";
  imageMatchingStrategy: ImageMatchingStrategy;
}
export interface ReshootSourceInfo {
  sourceIdentity?: string;
  projection?: "perspective" | "equirectangular";
  warmAvailable?: boolean;
  projectId: string;
  projectName: string;
  quality: Quality;
  plannerEnabled: boolean;
  cameraId: number;
  cameraModel: string;
  width: number;
  height: number;
  sourceImageCount: number;
  eligible: boolean;
  reason: string | null;
  imported: boolean;
}

export interface ReshootInputInfo {
  video?: VideoInfo | null;
  sourceIdentity?: string;
  projectionDetection?: ProjectionDetection;
  imageSequence?: ImageSequenceInfo;
  inputType: InputType;
  imageCount: number | null;
  duration: number | null;
  preparedWidth: number;
  preparedHeight: number;
  estimatedFrames: number;
  hasAlpha: boolean;
  maskCount: number;
  compatible: boolean;
  incompatibilityReason: string | null;
  estimate: RuntimeEstimate;
}

export interface PipelineEvent {
  operation?: PipelineOperation | null;
  taskId?: string | null; runId?: string | null; revision?: number;
  sequence: number;
  timestamp: string;
  kind: "stage" | "progress" | "log" | "heartbeat" | "capability" | "runtime";
  level: "info" | "warning" | "error";
  stage: string;
  substage?: string | null;
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
  runtime?: RuntimeSnapshot | null;
}

export interface RuntimeSnapshot {
  processId: number;
  phase: string;
  updatedAt: string;
  lastOutputAgeMs: number;
  training: {
    iteration: number | null;
    total: number | null;
    startIter: number;
    lod: number;
    stepsPerSecond: number | null;
    remainingSeconds: number | null;
    splatCount: number | null;
    psnr: number | null;
    ssim: number | null;
  } | null;
  device: string | null;
  backend: string | null;
  config: Record<string, string>;
  resources: {
    processId: number;
    sampledAt: string;
    cpuPercent: number | null;
    memoryBytes: number | null;
    gpuStatus: "available" | "unavailable" | "unsupported";
    gpus: Array<{
      uuid: string;
      name: string;
      utilizationPercent: number | null;
      memoryUsedMib: number | null;
      memoryTotalMib: number | null;
    }>;
  } | null;
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
  workspaceTaskId?: string | null;
  taskKind?: "generation" | "reshoot" | "import";
  name: string;
  status: ProjectStatus;
  projectPath: string;
  finalPly: string | null;
  fileSize: number | null;
  splatCount: number | null;
  createdAt: string;
  completedAt: string | null;
  durationMs: number | null;
  quality: Quality | null;
  sourceName: string;
  modelFormat?: ImportedGaussianFormat | null;
  sourceAvailable?: boolean | null;
  reshootEligible?: boolean | null;
  reshootReason?: string | null;
  registeredRatio: number | null;
  points3d: number | null;
  failureMessage: string | null;
}

export interface AppSettings { schemaVersion: number; projectsRoot: string; plannerEnabled: boolean; }
export interface ProjectOverview { projectsRoot: string; plannerEnabled: boolean; projects: ProjectSummary[]; }
export interface AppRuntimeStatus {
  pipelineRunning: boolean;
  pipelineProjectId?: string | null;
  pipelineWorkspaceTaskId?: string | null;
  pipelineRunElapsedMs: number;
  pipelineElapsedOffsetMs: number;
  previewProjectId: string | null;
  taskAcceleration: ColmapAccelerationStatus | null;
}

export interface ProjectTaskLogLine { source: string; message: string; }
export interface ProjectTaskDetail {
  inputs?:import('./inputs').SavedInputs|null;
  generationPolicy?: GenerationPolicy | null;
  automaticallyRetried?: boolean;
  reshootRecipe?: ReshootRecipeSummary | null;
  videoProjection?: VideoProjection | null;
  inputProjection?: VideoProjection | null;
  project: ProjectSummary;
  inputType: InputType | null;
  sourcePath: string;
  projectsRoot: string;
  plannerEnabled: boolean | null;
  estimatedFrames: number | null;
  stage: string | null;
  progress: number | null;
  inputImages: number | null;
  registeredImages: number | null;
  video: VideoInfo | null;
  imageSequence: ImageSequenceInfo | null;
  sourceProjectId: string | null;
  logs: ProjectTaskLogLine[];
  imported: ImportedProjectDetail | null;
}

export interface ReshootRecipeSummary {
  options: ReshootOptions;
  initial: { reconstruction: 'incremental' | 'full'; training: 'warm' | 'cold' };
  actual: { reconstruction: 'incremental' | 'full'; training: 'warm' | 'cold' };
}

export type ImportedGaussianFormat = "ply" | "compressedPly" | "sog" | "spz" | "splat" | "ksplat";
export interface ImportedProjectDetail { sourceModelPath: string; format: ImportedGaussianFormat; sourceAvailable: boolean; colmapRoot: string | null; sparseModelPath: string | null; databaseAvailable: boolean; imagesAvailable: boolean; reshootEligible: boolean; reshootReason: string | null; }
export interface GaussianImportCandidate { id: string; name: string; format: ImportedGaussianFormat; fileSize: number; directPly: boolean; splatCount: number | null; }
export interface ColmapImportCandidate { id: string; label: string; registeredImages: number; points3d: number; cameraCount: number; reshootEligible: boolean; reason: string | null; }
export interface GaussianImportProbe { probeId: string; suggestedName: string; models: GaussianImportCandidate[]; colmapModels: ColmapImportCandidate[]; defaultModelId: string | null; defaultColmapId: string | null; }
export interface GaussianImportSelection { selectionId: string; displayName: string; }
export interface GaussianImportSession { importId: string; sourceName: string; sourceSize: number; sourceTotalSize: number; sourceFormat: ImportedGaussianFormat; needsConversion: boolean; outputName: string; }

export type GaussianFormat = "ply" | "sog" | "spz";
export interface GaussianTransform {
  position: [number, number, number];
  rotation: [number, number, number];
  scale: number;
}
export type GaussianEditorTool = "transform" | "rectangle" | "sphere" | "box";
export type GaussianOrthographicView = "side" | "front" | "top";
export type GaussianCrop =
  | { kind: "sphere"; center: [number, number, number]; radius: number; inverted?: boolean }
  | { kind: "box"; center: [number, number, number]; size: [number, number, number]; inverted?: boolean }
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
  readToken?: string | null;
  referenceView?: TrainingReferenceView | null;
  referenceViewUnavailable?: "imported" | "unavailable" | null;
}
export interface TrainingReferenceView {
  imageName: string;
  position: [number, number, number];
  forward: [number, number, number];
  up: [number, number, number];
  fov: number;
  projectionOffset: [number, number];
  projectionScaleX: number;
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

export type VideoOrientation = "portrait" | "landscape";
export interface GaussianHtmlView {
  target: [number, number, number];
  yaw: number;
  pitch: number;
  distance: number;
  horizontalFrameOffset: number;
  projection: number;
  orthoHeight: number;
  orthographicView: GaussianOrthographicView | null;
  roll?: number;
  projectionOffset?: [number, number];
  projectionScaleX?: number;
  fov: number;
  nearClip: number;
  farClip: number;
}
export interface GaussianHtmlExportProgress {
  exportId: string;
  phase: "model" | "packing" | "completed";
  progress: number;
}
export interface GaussianHtmlExportResult {
  path: string;
  fileSize: number;
  splatCount: number;
}

export interface GaussianVideoExportResult {
  path: string;
  fileSize: number;
  width: number;
  height: number;
  fps: number;
  durationMs: number;
}
