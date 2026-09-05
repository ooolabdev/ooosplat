import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { ColmapAccelerationStatus, EngineStatus, FramePlan, GaussianExportProgress, GaussianExportResult, GaussianPreviewDescriptor, GaussianTransform, GaussianVideoExportResult, GaussianVideoExportSession, PipelineEvent, PipelineResult, ProjectOverview, ProjectSummary, Quality, RuntimeEstimate, VideoInfo } from "../types/pipeline";
import type { TelemetryPreferences } from "../types/telemetry";
import { previewAssetUrl } from "./previewAssetUrl";

const inTauri = () => "__TAURI_INTERNALS__" in window;

export async function selectVideo(): Promise<string | null> {
  if (!inTauri()) return null;
  const selected = await open({ multiple: false, directory: false, filters: [{ name: "视频", extensions: ["mp4", "mov"] }] });
  return typeof selected === "string" ? selected : null;
}

export async function selectProjectsRoot(current: string): Promise<string | null> {
  if (!inTauri()) return null;
  const selected = await open({ multiple: false, directory: true, defaultPath: current || undefined });
  return typeof selected === "string" ? selected : null;
}

export async function checkEngines(): Promise<EngineStatus[]> { return inTauri() ? invoke("check_engines") : []; }
export async function checkColmapAcceleration(): Promise<ColmapAccelerationStatus> { return invoke("check_colmap_acceleration"); }
export async function probeAndPlan(path: string, quality: Quality): Promise<{ video: VideoInfo; plan: FramePlan; estimate: RuntimeEstimate }> { return invoke("probe_and_plan", { path, quality }); }
export async function getProjectOverview(): Promise<ProjectOverview> { return invoke("get_project_overview"); }
export async function setProjectsRoot(projectsRoot: string): Promise<{ projectsRoot: string }> { return invoke("set_projects_root", { projectsRoot }); }
export async function startPipeline(path: string, quality: Quality, projectsRoot: string): Promise<PipelineResult> { return invoke("start_pipeline", { path, quality, projectsRoot }); }
export async function resumePipeline(projectId: string): Promise<PipelineResult> { return invoke("resume_pipeline", { projectId }); }
export async function cancelPipeline(): Promise<void> { return invoke("cancel_pipeline"); }
export async function onPipelineEvent(handler: (event: PipelineEvent) => void): Promise<UnlistenFn> { return listen<PipelineEvent>("pipeline-event", ({ payload }) => handler(payload)); }
export async function initializeTelemetry(): Promise<TelemetryPreferences> { return invoke("initialize_telemetry"); }
export async function setTelemetryConsent(enabled: boolean): Promise<TelemetryPreferences> { return invoke("set_telemetry_consent", { enabled }); }

export async function prepareGaussianPreview(projectId: string): Promise<GaussianPreviewDescriptor & { assetUrl: string }> {
  const descriptor = await invoke<GaussianPreviewDescriptor>("prepare_gaussian_preview", { projectId });
  return {
    ...descriptor,
    assetUrl: previewAssetUrl(convertFileSrc(descriptor.assetPath), "previewSession", crypto.randomUUID()),
  };
}
export async function releaseGaussianPreview(projectId: string): Promise<void> { return invoke("release_gaussian_preview", { projectId }); }
export async function saveGaussianTransform(projectId: string, transform: GaussianTransform): Promise<GaussianTransform> { return invoke("save_gaussian_transform", { projectId, transform }); }
export async function exportTransformedGaussian(projectId: string, transform: GaussianTransform): Promise<GaussianExportResult> { return invoke("export_transformed_gaussian", { projectId, transform }); }
export async function onGaussianExportProgress(handler: (event: GaussianExportProgress) => void): Promise<UnlistenFn> { return listen<GaussianExportProgress>("gaussian-export-progress", ({ payload }) => handler(payload)); }
export async function beginGaussianVideoExport(projectId: string): Promise<GaussianVideoExportSession> { return invoke("begin_gaussian_video_export", { projectId }); }
export async function commitGaussianVideoExport(exportId: string, bytes: Uint8Array): Promise<GaussianVideoExportResult> {
  return invoke("commit_gaussian_video_export", bytes, { headers: { "x-ooosplat-export-id": exportId } });
}
export async function cancelGaussianVideoExport(exportId: string): Promise<void> { return invoke("cancel_gaussian_video_export", { exportId }); }

export async function revealProject(project: ProjectSummary): Promise<void> {
  await revealItemInDir(project.finalPly ?? project.projectPath);
}

export async function revealFile(path: string): Promise<void> {
  await revealItemInDir(path);
}

export async function confirmAndDeleteProject(project: ProjectSummary, beforeDelete?: () => void | Promise<void>): Promise<boolean> {
  const accepted = await confirm(`将“${project.name}”及其中的源视频、抽帧、COLMAP、Brush 和日志全部移入回收站。\n\n此操作无法在应用内撤销。`, {
    title: "删除项目",
    kind: "warning",
    okLabel: "移入回收站",
    cancelLabel: "取消",
  });
  if (!accepted) return false;
  await beforeDelete?.();
  await invoke("delete_project", { projectId: project.id });
  return true;
}

export async function exportPly(result: PipelineResult): Promise<string | null> {
  const destination = await save({ defaultPath: "final.ply", filters: [{ name: "Gaussian Splat PLY", extensions: ["ply"] }] });
  if (!destination) return null;
  await invoke("export_ply", { sourcePath: result.finalPly, destinationPath: destination });
  return destination;
}
