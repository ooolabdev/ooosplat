import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { ColmapAccelerationStatus, EngineStatus, GaussianCrop, GaussianEditSaveSession, GaussianEditState, GaussianExportProgress, GaussianExportResult, GaussianPreviewDescriptor, GaussianTransform, GaussianVideoExportResult, GaussianVideoExportSession, PipelineEvent, PipelineResult, ProbeAndPlan, ProjectOverview, ProjectSummary, Quality, RuntimeEstimate } from "../types/pipeline";
import type { TelemetryPreferences } from "../types/telemetry";
import { getCurrentLocale, translate } from "../i18n";
import { previewAssetUrl } from "./previewAssetUrl";

const inTauri = () => "__TAURI_INTERNALS__" in window;

export async function selectVideo(): Promise<string | null> {
  if (!inTauri()) return null;
  const locale = getCurrentLocale();
  const selected = await open({ title: translate(locale, "dialog.selectVideoTitle"), multiple: false, directory: false, filters: [{ name: translate(locale, "input.video"), extensions: ["mp4", "mov"] }] });
  return typeof selected === "string" ? selected : null;
}

export async function selectImageSequence(): Promise<string | null> {
  if (!inTauri()) return null;
  const locale = getCurrentLocale();
  const selected = await open({ title: translate(locale, "dialog.selectImagesTitle"), multiple: false, directory: true });
  return typeof selected === "string" ? selected : null;
}

export async function confirmLargeImageSequence(imageCount: number): Promise<boolean> {
  const locale = getCurrentLocale();
  return confirm(
    translate(locale, "dialog.largeSequence", { count: imageCount.toLocaleString(locale) }),
    {
      title: translate(locale, "dialog.largeSequenceTitle"),
      kind: "warning",
      okLabel: translate(locale, "dialog.continue"),
      cancelLabel: translate(locale, "common.cancel"),
    },
  );
}

export async function selectProjectsRoot(current: string): Promise<string | null> {
  if (!inTauri()) return null;
  const locale = getCurrentLocale();
  const selected = await open({ title: translate(locale, "dialog.selectProjectRootTitle"), multiple: false, directory: true, defaultPath: current || undefined });
  return typeof selected === "string" ? selected : null;
}

export async function checkEngines(): Promise<EngineStatus[]> { return inTauri() ? invoke("check_engines") : []; }
export async function checkColmapAcceleration(): Promise<ColmapAccelerationStatus> { return invoke("check_colmap_acceleration"); }
export async function probeAndPlan(path: string, quality: Quality): Promise<ProbeAndPlan> { return invoke("probe_and_plan", { path, quality }); }
export async function estimateProjectRuntime(projectId: string): Promise<RuntimeEstimate> { return invoke("estimate_project_runtime", { projectId }); }
export async function getProjectOverview(): Promise<ProjectOverview> { return invoke("get_project_overview"); }
export async function setProjectsRoot(projectsRoot: string): Promise<{ projectsRoot: string }> { return invoke("set_projects_root", { projectsRoot }); }
export async function startPipeline(path: string, quality: Quality, projectsRoot: string): Promise<PipelineResult> { return invoke("start_pipeline", { path, quality, projectsRoot }); }
export async function resumePipeline(projectId: string): Promise<PipelineResult> { return invoke("resume_pipeline", { projectId }); }
export async function cancelPipeline(): Promise<void> { return invoke("cancel_pipeline"); }
export async function onPipelineEvent(handler: (event: PipelineEvent) => void): Promise<UnlistenFn> { return listen<PipelineEvent>("pipeline-event", ({ payload }) => handler(payload)); }
export async function initializeTelemetry(): Promise<TelemetryPreferences> { return invoke("initialize_telemetry"); }
export async function setTelemetryConsent(enabled: boolean): Promise<TelemetryPreferences> { return invoke("set_telemetry_consent", { enabled }); }

export async function prepareGaussianPreview(projectId: string): Promise<GaussianPreviewDescriptor & { assetUrl: string; editMaskAssetUrl: string | null }> {
  let descriptor: GaussianPreviewDescriptor;
  try {
    descriptor = await invoke<GaussianPreviewDescriptor>("prepare_gaussian_preview", { projectId });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const damagedEditState = message.includes("编辑") && (message.includes("位图") || message.includes("损坏") || message.includes("校验") || message.includes("数量不匹配"));
    if (!damagedEditState) throw error;
    const locale = getCurrentLocale();
    const accepted = await confirm(translate(locale, "dialog.editRecovery", { detail: message }), {
      title: translate(locale, "dialog.editRecoveryTitle"),
      kind: "warning",
      okLabel: translate(locale, "dialog.clearEdits"),
      cancelLabel: translate(locale, "common.cancel"),
    });
    if (!accepted) throw error;
    await resetGaussianEdits(projectId);
    descriptor = await invoke<GaussianPreviewDescriptor>("prepare_gaussian_preview", { projectId });
  }
  return {
    ...descriptor,
    assetUrl: previewAssetUrl(convertFileSrc(descriptor.assetPath), "previewSession", crypto.randomUUID()),
    editMaskAssetUrl: descriptor.editMaskAssetPath
      ? previewAssetUrl(convertFileSrc(descriptor.editMaskAssetPath), "editSession", crypto.randomUUID())
      : null,
  };
}
export async function releaseGaussianPreview(projectId: string): Promise<void> { return invoke("release_gaussian_preview", { projectId }); }
export async function saveGaussianTransform(projectId: string, transform: GaussianTransform): Promise<GaussianTransform> { return invoke("save_gaussian_transform", { projectId, transform }); }
export async function exportTransformedGaussian(projectId: string, transform: GaussianTransform, editRevision?: number): Promise<GaussianExportResult> { return invoke("export_transformed_gaussian", { projectId, transform, editRevision }); }
export async function beginGaussianEditSave(projectId: string, crop: GaussianCrop, baseRevision: number): Promise<GaussianEditSaveSession> {
  return invoke("begin_gaussian_edit_save", { projectId, editState: { crop, baseRevision } });
}
export async function commitGaussianEditSave(editId: string, mask: Uint8Array): Promise<GaussianEditState> {
  return invoke("commit_gaussian_edit_save", mask, { headers: { "x-ooosplat-edit-id": editId } });
}
export async function resetGaussianEdits(projectId: string): Promise<GaussianEditState> { return invoke("reset_gaussian_edits", { projectId }); }
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
  const locale = getCurrentLocale();
  const accepted = await confirm(translate(locale, "dialog.deleteProject", { name: project.name }), {
    title: translate(locale, "dialog.deleteTitle"),
    kind: "warning",
    okLabel: translate(locale, "dialog.trash"),
    cancelLabel: translate(locale, "common.cancel"),
  });
  if (!accepted) return false;
  await beforeDelete?.();
  await invoke("delete_project", { projectId: project.id });
  return true;
}

export async function exportPly(result: PipelineResult): Promise<string | null> {
  const locale = getCurrentLocale();
  const destination = await save({ title: translate(locale, "dialog.savePlyTitle"), defaultPath: "final.ply", filters: [{ name: "Gaussian Splat PLY", extensions: ["ply"] }] });
  if (!destination) return null;
  await invoke("export_ply", { sourcePath: result.finalPly, destinationPath: destination });
  return destination;
}
