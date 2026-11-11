export type PipelineErrorCode = "cancelled" | "pipeline_failed";
export type PipelineFailureKind = "mapper_source" | "mapper_storage" | "brush_gpu" | "brush_device_lost" | "brush_dataset" | "brush_out_of_memory" | "brush_resource_limit" | "colmap_crash";

export interface FailureContext {
  classificationVersion?: number;
  failureKind?: PipelineFailureKind;
}

export interface ProcessTermination {
  exitCode: number | null;
  windowsStatus?: string;
  signal?: number;
  coreDumped?: boolean;
}

export interface PipelineCommandError extends FailureContext {
  code: PipelineErrorCode;
  message: string;
  failedStage?: string;
  engine?: "system" | "ffmpeg" | "colmap" | "brush";
  failureKind?: PipelineFailureKind;
  projectId?: string;
  /** Local task-waiter identity; not part of the native error protocol. */
  taskId?: string;
  runId?: string;
  projectPath?: string;
  logsDirectory?: string;
  failureId?: string;
  classificationIsHeuristic?: boolean;
  termination?: ProcessTermination;
}

const failureKinds = new Set<PipelineFailureKind>(["mapper_source", "mapper_storage", "brush_gpu", "brush_device_lost", "brush_dataset", "brush_out_of_memory", "brush_resource_limit", "colmap_crash"]);

export function taskFailureContext(error: { classification_version?: number | null; classification?: string | null } | null | undefined): FailureContext | undefined {
  if (!error || error.classification_version == null) return undefined;
  return { classificationVersion: error.classification_version,
    failureKind: failureKinds.has(error.classification as PipelineFailureKind) ? error.classification as PipelineFailureKind : undefined };
}

export function pipelineCommandError(error: unknown): PipelineCommandError | null {
  if (typeof error !== "object" || error == null) return null;
  const candidate = error as Record<string, unknown>;
  const details = candidate.details;
  if (details && typeof details === 'object' && 'pipelineError' in details) {
    const saved = details.pipelineError;
    if (saved && typeof saved === 'object') {
      const parsed = parsePipelineCommandError(saved as Record<string, unknown>);
      if (parsed) return {
        ...parsed,
        taskId: parsed.taskId ?? (typeof candidate.task_id === 'string' ? candidate.task_id : undefined),
        runId: parsed.runId ?? (typeof candidate.run_id === 'string' ? candidate.run_id : undefined),
        // The service envelope carries diagnostics, not local path capabilities.
        projectPath: undefined,
        logsDirectory: undefined,
      };
    }
  }
  return parsePipelineCommandError(candidate);
}

function parsePipelineCommandError(candidate: Record<string, unknown>): PipelineCommandError | null {
  const code = candidate.code;
  const message = candidate.message;
  if ((code !== "cancelled" && code !== "pipeline_failed") || typeof message !== "string") return null;
  const failureKind = typeof candidate.failureKind === "string" && failureKinds.has(candidate.failureKind as PipelineFailureKind)
    ? candidate.failureKind as PipelineFailureKind
    : undefined;
  const engine = ["system", "ffmpeg", "colmap", "brush"].includes(String(candidate.engine))
    ? candidate.engine as PipelineCommandError["engine"]
    : undefined;
  return {
    code,
    message,
    failedStage: typeof candidate.failedStage === "string" ? candidate.failedStage : undefined,
    engine,
    failureKind,
    classificationVersion: Number.isSafeInteger(candidate.classificationVersion) && Number(candidate.classificationVersion) > 0 ? Number(candidate.classificationVersion) : undefined,
    classificationIsHeuristic: typeof candidate.classificationIsHeuristic === "boolean" ? candidate.classificationIsHeuristic : undefined,
    termination: parseTermination(candidate.termination),
    projectId: typeof candidate.projectId === "string" ? candidate.projectId : undefined,
    taskId: typeof candidate.taskId === "string" ? candidate.taskId : undefined,
    runId: typeof candidate.runId === "string" ? candidate.runId : undefined,
    projectPath: typeof candidate.projectPath === "string" ? candidate.projectPath : undefined,
    logsDirectory: typeof candidate.logsDirectory === "string" ? candidate.logsDirectory : undefined,
    failureId: typeof candidate.failureId === "string" ? candidate.failureId : undefined,
  };
}

function parseTermination(value: unknown): ProcessTermination | undefined {
  if (!value || typeof value !== "object") return undefined;
  const status = value as Record<string, unknown>;
  return {
    exitCode: Number.isInteger(status.exitCode) ? Number(status.exitCode) : null,
    windowsStatus: typeof status.windowsStatus === "string" && /^0x[\dA-F]{8}$/.test(status.windowsStatus) ? status.windowsStatus : undefined,
    signal: Number.isInteger(status.signal) && Number(status.signal) > 0 ? Number(status.signal) : undefined,
    coreDumped: typeof status.coreDumped === "boolean" ? status.coreDumped : undefined,
  };
}

export function pipelineErrorCode(error: unknown): PipelineErrorCode | null {
  return pipelineCommandError(error)?.code ?? null;
}

export function pipelineErrorMessage(error: unknown): string | null {
  if (typeof error === "string") return error;
  const structured = pipelineCommandError(error);
  if (structured) return structured.message;
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error != null && "message" in error) {
    const message = (error as { message?: unknown }).message;
    return typeof message === "string" ? message : null;
  }
  return null;
}

export function pipelineWasCancelled(error: unknown, terminalStage?: string): boolean {
  return pipelineErrorCode(error) === "cancelled" || terminalStage === "cancelled";
}
