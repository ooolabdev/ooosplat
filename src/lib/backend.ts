import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { ColmapAccelerationStatus, EngineDownloadProgress, EngineStatus, FramePlan, PipelineEvent, PipelineResult, ProjectOverview, ProjectSummary, Quality, VideoInfo } from "../types/pipeline";

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
export async function downloadMissingEngines(): Promise<EngineStatus[]> { return inTauri() ? invoke("download_missing_engines") : []; }
export async function onEngineDownloadProgress(handler: (event: EngineDownloadProgress) => void): Promise<UnlistenFn> { return listen<EngineDownloadProgress>("engine-download-progress", ({ payload }) => handler(payload)); }
export async function checkColmapAcceleration(): Promise<ColmapAccelerationStatus> { return invoke("check_colmap_acceleration"); }
export async function probeAndPlan(path: string, quality: Quality): Promise<{ video: VideoInfo; plan: FramePlan }> { return invoke("probe_and_plan", { path, quality }); }
export async function getProjectOverview(): Promise<ProjectOverview> { return invoke("get_project_overview"); }
export async function setProjectsRoot(projectsRoot: string): Promise<{ projectsRoot: string }> { return invoke("set_projects_root", { projectsRoot }); }
export async function startPipeline(path: string, quality: Quality, projectsRoot: string): Promise<PipelineResult> { return invoke("start_pipeline", { path, quality, projectsRoot }); }
export async function cancelPipeline(): Promise<void> { return invoke("cancel_pipeline"); }
export async function onPipelineEvent(handler: (event: PipelineEvent) => void): Promise<UnlistenFn> { return listen<PipelineEvent>("pipeline-event", ({ payload }) => handler(payload)); }

export async function revealProject(project: ProjectSummary): Promise<void> {
  await revealItemInDir(project.finalPly ?? project.projectPath);
}

export async function confirmAndDeleteProject(project: ProjectSummary): Promise<boolean> {
  const accepted = await confirm(`将“${project.name}”及其中的源视频、抽帧、COLMAP、Brush 和日志全部移入回收站。\n\n此操作无法在应用内撤销。`, {
    title: "删除项目",
    kind: "warning",
    okLabel: "移入回收站",
    cancelLabel: "取消",
  });
  if (!accepted) return false;
  await invoke("delete_project", { projectId: project.id });
  return true;
}

export async function exportPly(result: PipelineResult): Promise<string | null> {
  const destination = await save({ defaultPath: "final.ply", filters: [{ name: "Gaussian Splat PLY", extensions: ["ply"] }] });
  if (!destination) return null;
  await invoke("export_ply", { sourcePath: result.finalPly, destinationPath: destination });
  return destination;
}

export async function readPlyBytes(path: string): Promise<Uint8Array> {
  const bytes = await invoke<number[] | Uint8Array>("read_ply_bytes", { path });
  return bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
}
