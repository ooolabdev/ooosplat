export type PipelineErrorCode = "cancelled" | "pipeline_failed";
export type PipelineFailureKind = "mapper_source" | "mapper_storage" | "brush_gpu" | "brush_device_lost" | "brush_dataset";

export interface PipelineCommandError {
  code: PipelineErrorCode;
  message: string;
  failedStage?: string;
  engine?: "system" | "ffmpeg" | "colmap" | "brush";
  failureKind?: PipelineFailureKind;
  projectId?: string;
  projectPath?: string;
  logsDirectory?: string;
  failureId?: string;
}

const failureKinds = new Set<PipelineFailureKind>(["mapper_source", "mapper_storage", "brush_gpu", "brush_device_lost", "brush_dataset"]);

export function pipelineCommandError(error: unknown): PipelineCommandError | null {
  if (typeof error !== "object" || error == null) return null;
  const candidate = error as Record<string, unknown>;
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
    projectId: typeof candidate.projectId === "string" ? candidate.projectId : undefined,
    projectPath: typeof candidate.projectPath === "string" ? candidate.projectPath : undefined,
    logsDirectory: typeof candidate.logsDirectory === "string" ? candidate.logsDirectory : undefined,
    failureId: typeof candidate.failureId === "string" ? candidate.failureId : undefined,
  };
}

export function pipelineErrorCode(error: unknown): PipelineErrorCode | null {
  return pipelineCommandError(error)?.code ?? null;
}

export function pipelineErrorMessage(error: unknown): string | null {
  if (typeof error === "string") return error;
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
