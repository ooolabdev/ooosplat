import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { AppRuntimeStatus, AppSettings, ColmapAccelerationStatus, EngineStatus, GaussianCrop, GaussianEditSaveSession, GaussianEditState, GaussianExportProgress, GaussianExportResult, GaussianImportProbe, GaussianImportSelection, GaussianImportSession, GaussianPreviewDescriptor, GaussianTransform, GaussianVideoExportResult, GaussianVideoExportSession, InputType, PipelineEvent, PipelineResult, ProbeAndPlan, ProjectOverview, ProjectSummary, ProjectTaskDetail, Quality, ReshootInputInfo, ReshootSourceInfo, RuntimeEstimate } from "../types/pipeline";
import type { TelemetryPreferences } from "../types/telemetry";
import type { ErrorReportDraft, ErrorReportReceipt } from "../types/diagnostics";
import { getCurrentLocale, translate } from "../i18n";
import { previewAssetUrl } from "./previewAssetUrl";

import type { SharedTask, TaskUpdate, StartReceipt, McpConnection, McpSettings } from "../types/tasks";
import { taskIsActive } from "../types/tasks";

const inTauri = () => "__TAURI_INTERNALS__" in window;

export async function selectMaterialPaths(directory:boolean):Promise<string[]> {
  if(!inTauri())return [];
  const paths=await open({multiple:true,directory,...(!directory?{filters:[{name:translate(getCurrentLocale(),'input.label'),extensions:['mp4','mov','jpg','jpeg','png']}]}:{})});
  return Array.isArray(paths)?paths:typeof paths==='string'?[paths]:[];
}
export async function selectColmapDatabase():Promise<string|null> {
  if(!inTauri())return null;
  const path=await open({multiple:false,directory:false,filters:[{name:translate(getCurrentLocale(),'inputs.database'),extensions:['db']}]});
  return typeof path==='string'?path:null;
}
export async function selectColmapFolder(field:'model'|'images'):Promise<string|null>{
  if(!inTauri())return null;
  const path=await open({multiple:false,directory:true,title:translate(getCurrentLocale(),`inputs.${field}`)});
  return typeof path==='string'?path:null;
}
export const classifyMaterialPaths=(paths:string[]):Promise<import('../types/inputs').MaterialInput[]>=>invoke('classify_material_paths',{paths});
export const classifyGenerationPaths=(paths:string[]):Promise<import('../types/inputs').GenerationPathSelection>=>invoke('classify_generation_paths',{paths});
export const detectMaterialGps=(item:import('../types/inputs').MaterialInput,identity:string):Promise<boolean>=>invoke('detect_material_gps',{item,identity});
export const inspectMaterialInputs=(inputs:import('../types/inputs').GenerationInputs,quality:Quality,generationPolicy:import('../types/pipeline').GenerationPolicy):Promise<import('../types/inputs').InputsInspection>=>invoke('inspect_material_inputs',{inputs,quality,generationPolicy});
export const estimateMaterialInputs=(inputs:import('../types/inputs').GenerationInputs,quality:Quality,generationPolicy:import('../types/pipeline').GenerationPolicy):Promise<RuntimeEstimate|null>=>invoke('estimate_material_inputs',{inputs,quality,generationPolicy});
export async function startInputsPipeline(inputs:import('../types/inputs').GenerationInputs,quality:Quality,projectsRoot:string,workspaceTaskId:string,generationPolicy:import('../types/pipeline').GenerationPolicy,sourceProjectId?:string|null,reshootOptions?:import('../types/pipeline').ReshootOptions):Promise<PipelineResult>{
  const receipt=await invoke<StartReceipt>('start_gui_inputs',{inputs,quality,projectsRoot,workspaceTaskId,generationPolicy,sourceProjectId:sourceProjectId??null,reshootOptions:reshootOptions??null});
  return waitForSharedTask(receipt);
}

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

export async function selectGaussianImportSource(kind: "file" | "folder"): Promise<GaussianImportSelection | null> { return inTauri() ? invoke("select_gaussian_import_source", { kind }) : null; }
export async function probeGaussianImport(selectionId: string): Promise<GaussianImportProbe> { return invoke("probe_gaussian_import", { selectionId }); }
export async function beginGaussianImport(request: { probeId: string; modelId: string; colmapId: string | null; projectName: string }): Promise<GaussianImportSession> { return invoke("begin_gaussian_import", { request }); }
export async function readGaussianImportChunk(importId: string, filename: string, offset: number, length: number): Promise<ArrayBuffer | Uint8Array> {
  return invoke("read_gaussian_import_chunk", undefined, { headers: { "x-ooosplat-import-id": importId, "x-ooosplat-import-file": filename, "x-ooosplat-offset": String(offset), "x-ooosplat-length": String(length) } });
}
export async function writeGaussianImportChunk(importId: string, offset: number, bytes: Uint8Array): Promise<number> { return invoke("write_gaussian_import_chunk", bytes, { headers: { "x-ooosplat-import-id": importId, "x-ooosplat-offset": String(offset) } }); }
export async function completeGaussianImport(request: { importId: string; splatCount: number; shBands: number; model: string | null }): Promise<ProjectSummary> { return invoke("complete_gaussian_import", { request }); }
export async function cancelGaussianImport(importId: string): Promise<void> { return invoke("cancel_gaussian_import", { importId }); }
export async function relinkImportedProject(projectId: string, selectionId: string): Promise<ProjectSummary> { return invoke("relink_imported_project", { projectId, selectionId }); }


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

export async function confirmSmallImageSequence(imageCount: number): Promise<boolean> {
  const locale = getCurrentLocale();
  const advice = [
    translate(locale, "captureAdvice.title"),
    `• ${translate(locale, "captureAdvice.coverage")}`,
    `• ${translate(locale, "captureAdvice.overlap")}`,
    `• ${translate(locale, "captureAdvice.consistency")}`,
    "",
    translate(locale, "captureAdvice.aiWarning"),
  ].join("\n");
  return confirm(
    `${translate(locale, "dialog.smallSequence", { count: imageCount.toLocaleString(locale) })}\n\n${advice}`,
    {
      title: translate(locale, "dialog.smallSequenceTitle"),
      kind: "warning",
      okLabel: translate(locale, "dialog.continueGeneration"),
      cancelLabel: translate(locale, "dialog.cancelGeneration"),
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
export async function detectImageGps(path: string, sourceIdentity: string): Promise<boolean> {
  return invoke('detect_image_gps', { path, sourceIdentity });
}
export async function checkColmapAcceleration(): Promise<ColmapAccelerationStatus | null> { return inTauri() ? invoke("check_colmap_acceleration") : null; }
export async function probeAndPlan(path: string, quality: Quality, plannerEnabled = true, cached?: import("../types/pipeline").InputAnalysis | import("../types/pipeline").VideoAnalysis, onAnalyzed?: (value: import("../types/pipeline").InputAnalysis) => void, generationPolicy?: import('../types/pipeline').GenerationPolicy): Promise<ProbeAndPlan> {
  const analysis = cached ?? await invoke<import("../types/pipeline").InputAnalysis>("analyze_input", { path });
  if ('inputType' in analysis) onAnalyzed?.(analysis);
  return invoke("probe_and_plan", { path, quality, plannerEnabled, ...(generationPolicy ? { generationPolicy } : {}), ...('inputType' in analysis ? { inputAnalysis: analysis } : { videoAnalysis: analysis }) });
}
export async function estimateProjectRuntime(projectId: string): Promise<RuntimeEstimate> { return invoke("estimate_project_runtime", { projectId }); }
export async function getProjectOverview(): Promise<ProjectOverview> { return invoke("get_project_overview"); }
export async function getProjectTaskDetail(projectId: string): Promise<ProjectTaskDetail> { return invoke("get_project_task_detail", { projectId }); }
export async function getAppRuntimeStatus(): Promise<AppRuntimeStatus> { return invoke("get_app_runtime_status"); }
export async function setProjectsRoot(projectsRoot: string): Promise<AppSettings> { return invoke("set_projects_root", { projectsRoot }); }
export async function setPlannerEnabled(enabled: boolean): Promise<AppSettings> { return invoke("set_planner_enabled", { enabled }); }
export async function classifyDroppedInput(path: string): Promise<{ inputType: InputType }> { return invoke("classify_dropped_input", { path }); }

export type NativeInputDragEvent =
  | { type: "enter" | "over" | "drop"; paths: string[]; x: number; y: number }
  | { type: "leave"; paths: [] };

/** Tauri reports physical pixels; expose logical client coordinates to the React layout. */
export async function onInputDragDrop(handler: (event: NativeInputDragEvent) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => undefined;
  let scaleFactor = await getCurrentWindow().scaleFactor();
  return getCurrentWebview().onDragDropEvent(async ({ payload }) => {
    if (payload.type === "leave") {
      handler({ type: "leave", paths: [] });
      return;
    }
    if (payload.type === "enter") scaleFactor = await getCurrentWindow().scaleFactor();
    handler({
      type: payload.type,
      paths: "paths" in payload ? payload.paths : [],
      x: payload.position.x / scaleFactor,
      y: payload.position.y / scaleFactor,
    });
  });
}
export async function startPipeline(path: string, quality: Quality, projectsRoot: string, plannerEnabled = true, workspaceTaskId?: string, videoProjection?: import("../types/pipeline").VideoProjection, videoSourceIdentity?: string, imageMatchingStrategy?: import("../types/pipeline").ImageMatchingStrategy, generationPolicy?: import('../types/pipeline').GenerationPolicy): Promise<PipelineResult> {
  const task = await invoke<SharedTask>("create_gui_task", { path, quality, projectsRoot, plannerEnabled, workspaceTaskId, inputProjection: videoProjection, inputSourceIdentity: videoSourceIdentity, imageMatchingStrategy, ...(generationPolicy ? { generationPolicy } : {}) });
  const receipt = await startGuiTask(task.task_id);
  return waitForSharedTask(receipt);
}
export async function resumePipeline(projectId: string, expectedRunId?: string): Promise<PipelineResult> {
  const receipt = await invoke<StartReceipt>("resume_gui_task", { projectId, ...(expectedRunId ? { expectedRunId } : {}) });
  return waitForSharedTask(receipt);
}
export async function inspectReshootSource(projectId: string): Promise<ReshootSourceInfo> { return invoke("inspect_reshoot_source", { projectId, recipeVersion: 2 }); }
export async function probeReshootInput(projectId: string, path: string, inputType: InputType, quality: Quality = "balanced", plannerEnabled = true, generationPolicy?: import('../types/pipeline').GenerationPolicy, videoAnalysis?: import('../types/pipeline').VideoAnalysis): Promise<ReshootInputInfo> { return invoke("probe_reshoot_input", { projectId, path, inputType, recipeVersion: 2, quality, plannerEnabled, generationPolicy, videoAnalysis }); }
export async function startReshootPipeline(request: { sourceProjectId: string; reshootPath: string; inputType: InputType; projectsRoot: string; workspaceTaskId?: string; quality?: Quality; plannerEnabled?: boolean; generationPolicy?: import('../types/pipeline').GenerationPolicy; reshootOptions?: import('../types/pipeline').ReshootOptions }): Promise<PipelineResult> {
  const receipt = await invoke<StartReceipt>("start_gui_reshoot", { request });
  return waitForSharedTask(receipt);
}
export async function cancelPipeline(): Promise<void> { return invoke("cancel_pipeline"); }
export async function prepareErrorReport(failureId: string): Promise<ErrorReportDraft> { return invoke("prepare_error_report", { failureId }); }
export async function sendErrorReport(draftId: string): Promise<ErrorReportReceipt> { return invoke("send_error_report", { draftId }); }
export async function onPipelineEvent(handler: (event: PipelineEvent) => void): Promise<UnlistenFn> { return inTauri() ? listen<PipelineEvent>("pipeline-event", ({ payload }) => handler(payload)) : () => undefined; }
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
    assetUrl: descriptor.readToken ? `ooosplat-import-${descriptor.readToken}.ply` : previewAssetUrl(convertFileSrc(descriptor.assetPath), "previewSession", crypto.randomUUID()),
    editMaskAssetUrl: descriptor.editMaskAssetPath
      ? previewAssetUrl(convertFileSrc(descriptor.editMaskAssetPath), "editSession", crypto.randomUUID())
      : null,
  };
}
export async function readGaussianPreviewChunk(readToken: string, offset: number, length: number): Promise<ArrayBuffer | Uint8Array> {
  return invoke("read_gaussian_preview_chunk", undefined, { headers: { "x-ooosplat-preview-token": readToken, "x-ooosplat-offset": String(offset), "x-ooosplat-length": String(length) } });
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
export async function beginGaussianVideoExport(projectId: string, orientation: import("../types/pipeline").VideoOrientation = "portrait", editRevision?:number): Promise<GaussianVideoExportSession> { return invoke("begin_gaussian_video_export", { projectId, orientation, ...(editRevision === undefined ? {} : {editRevision}) }); }
export async function commitGaussianVideoExport(exportId: string, bytes: Uint8Array): Promise<GaussianVideoExportResult> {
  return invoke("commit_gaussian_video_export", bytes, { headers: { "x-ooosplat-export-id": exportId } });
}
export async function cancelGaussianVideoExport(exportId: string): Promise<void> { return invoke("cancel_gaussian_video_export", { exportId }); }

export async function beginGaussianHtmlExport(projectId: string, editRevision: number, view: import("../types/pipeline").GaussianHtmlView, locale: string): Promise<GaussianVideoExportSession> {
  return invoke("begin_gaussian_html_export", { projectId, editRevision, view, locale });
}
export async function commitGaussianHtmlExport(exportId: string): Promise<import("../types/pipeline").GaussianHtmlExportResult> { return invoke("commit_gaussian_html_export", { exportId }); }
export async function cancelGaussianHtmlExport(exportId: string): Promise<void> { return invoke("cancel_gaussian_html_export", { exportId }); }
export async function onGaussianHtmlExportProgress(handler: (event: import("../types/pipeline").GaussianHtmlExportProgress) => void): Promise<UnlistenFn> { return listen<import("../types/pipeline").GaussianHtmlExportProgress>("gaussian-html-export-progress", ({ payload }) => handler(payload)); }

export async function revealProject(project: ProjectSummary): Promise<void> {
  await invoke("open_project_location", { projectId: project.id, location: "project" });
}

export async function revealProjectLogs(projectId: string): Promise<void> { await invoke("open_project_location", { projectId, location: "logs" }); }
export async function revealImportedSource(projectId: string): Promise<void> { await invoke("open_project_location", { projectId, location: "source" }); }

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

export async function exportPly(result: Pick<PipelineResult, 'finalPly'>): Promise<string | null> {
  const locale = getCurrentLocale();
  const destination = await save({ title: translate(locale, "dialog.savePlyTitle"), defaultPath: "final.ply", filters: [{ name: "Gaussian Splat PLY", extensions: ["ply"] }] });
  if (!destination) return null;
  await invoke("export_ply", { sourcePath: result.finalPly, destinationPath: destination });
  return destination;
}


export async function getSharedTasks(): Promise<SharedTask[]> { return inTauri() ? invoke("get_shared_tasks") : []; }
export async function getSharedTask(taskId: string): Promise<SharedTask> { return invoke("get_shared_task", { taskId }); }
export async function startGuiTask(taskId: string, retryFailedRunId?: string): Promise<StartReceipt> { return invoke("start_gui_task", { taskId, ...(retryFailedRunId ? {retryFailedRunId} : {}) }); }
/** Continue an identified task even when it failed before project creation. */
export async function continueTaskPipeline(taskId: string, expectedRunId: string): Promise<PipelineResult> {
  const receipt = await startGuiTask(taskId, expectedRunId);
  return waitForSharedTask(receipt);
}
export async function cancelSharedTask(taskId: string, runId: string): Promise<SharedTask> { return invoke("cancel_shared_task", { taskId, runId }); }
export async function onTaskUpdate(handler: (update: TaskUpdate) => void): Promise<UnlistenFn> { return inTauri() ? listen<TaskUpdate>("task-update", ({ payload }) => handler(payload)) : () => undefined; }
export async function getMcpSettings(): Promise<McpConnection> { return inTauri() ? invoke("get_mcp_settings") : { settings: { enabled: false, port: 39877, inputRoots: [] }, listening: false, address: null, error: null }; }
export async function setMcpSettings(settings: McpSettings): Promise<McpConnection> { return invoke("set_mcp_settings", { settings }); }
export async function selectMcpInputRoot(): Promise<string | null> { if (!inTauri()) return null; const path = await open({ directory: true, multiple: false }); return typeof path === 'string' ? path : null; }

/** Local completion waiter; the only native requests are short queries/admissions. */
async function waitForSharedTask(receipt: StartReceipt): Promise<PipelineResult> {
  // A legacy project-only continuation still returns an identified receipt.
  // Never guess its run from a later snapshot or silently follow another run.
  if (!receipt.task_id || !receipt.run_id || !Number.isSafeInteger(receipt.revision) || receipt.revision < 0) {
    throw { code: 'INVALID_DATA', message: 'The saved task execution identity is missing', taskId: receipt.task_id, runId: receipt.run_id };
  }
  return new Promise<PipelineResult>((resolve, reject) => {
    let settled = false;
    let unsubscribe: UnlistenFn | undefined;
    let timer: ReturnType<typeof setInterval> | undefined;
    let queryPending = false;
    let revision = receipt.revision - 1;
    const cleanup = () => {
      if (timer !== undefined) { clearInterval(timer); timer = undefined; }
      const release = unsubscribe;
      unsubscribe = undefined;
      try { release?.(); } catch { /* Cleanup must not replace the execution outcome. */ }
    };
    const finish = (outcome: () => void) => {
      if (settled) return;
      settled = true;
      cleanup();
      outcome();
    };
    const fail = (error: unknown) => finish(() => reject(error));
    const apply = (task: SharedTask) => {
      if (settled || task.task_id !== receipt.task_id || task.revision < receipt.revision || task.revision <= revision) return;
      if (task.run_id !== receipt.run_id) {
        fail({ code: 'STALE_RUN_ID', message: 'The task execution has changed', taskId: receipt.task_id, runId: receipt.run_id });
        return;
      }
      revision = task.revision;
      if (taskIsActive(task)) return;
      const result = task.result;
      if (task.status === 'completed' && result) finish(() => resolve(result));
      else fail({ code: task.status === 'cancelled' ? 'cancelled' : 'pipeline_failed', message: task.error?.message ?? task.status, failedStage: task.error?.failed_stage, engine: task.error?.engine, failureKind: task.error?.classification, classificationVersion: task.error?.classification_version, classificationIsHeuristic: task.error?.classification_is_heuristic, termination: task.error?.termination, failureId: task.error?.failure_id, projectId: task.project_id, taskId: task.task_id, runId: task.run_id });
    };
    const query = async () => {
      if (settled || queryPending) return;
      queryPending = true;
      try {
        const snapshot = await getSharedTask(receipt.task_id);
        if (!settled) apply(snapshot);
      } catch (error) { if (!settled) fail(error); }
      finally { queryPending = false; }
    };
    void (async () => {
      const release = await onTaskUpdate(update => apply(update.task));
      // An event can settle the waiter before native subscription returns.
      if (settled) { try { release(); } catch { /* already settled */ } return; }
      unsubscribe = release;
      await query();
      // Even an ignored old terminal snapshot must leave a live polling path.
      if (!settled) timer = setInterval(() => { void query(); }, 15000);
    })().catch(error => { if (!settled) fail(error); });
  });
}

export interface TaskLogPage {
  entries: Array<{ source: string; text: string; partial_line: boolean }>;
  next_cursor: string | null; has_more: boolean; cursor_reset: boolean; reset_reason: string | null;
  activity_events?: PipelineEvent[] | null;
  activity_journal_persisted?: boolean;
  truncated?: boolean;
}
export async function readSharedTaskLogs(taskId: string, runId: string | null, cursor?: string): Promise<TaskLogPage> { return invoke("read_shared_task_logs", { request: { task_id: taskId, run_id: runId, cursor, max_bytes: 32768 } }); }
