// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), confirm: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn(),
  invoke: mocks.invoke,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: mocks.confirm, open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: vi.fn() }));

import { probeAndPlan, prepareErrorReport,sendErrorReport,beginGaussianVideoExport,beginGaussianHtmlExport,commitGaussianHtmlExport,cancelGaussianHtmlExport,checkColmapAcceleration, classifyDroppedInput, confirmSmallImageSequence, getAppRuntimeStatus, revealProject, revealProjectLogs } from "./backend";
import { resumePipeline, startPipeline } from './backend';

describe("backend browser guards", () => {
  it('continues an identified task before project creation and preserves terminal task/run identity', async () => {
    const { continueTaskPipeline } = await import('./backend');
    mocks.invoke.mockResolvedValueOnce({ accepted: true, task_id: 'task-without-project', run_id: 'next-run', status: 'starting', revision: 1 })
      .mockResolvedValueOnce({ task_id: 'task-without-project', run_id: 'next-run', project_id: null, revision: 2,
        status: 'failed', error: { message: 'Preparation failed', engine: 'system', failed_stage: null } });
    await expect(continueTaskPipeline('task-without-project', 'captured-failed-run')).rejects.toMatchObject({
      code: 'pipeline_failed', taskId: 'task-without-project', runId: 'next-run', projectId: null,
    });
    expect(mocks.invoke).toHaveBeenNthCalledWith(1, 'start_gui_task', { taskId: 'task-without-project', retryFailedRunId: 'captured-failed-run' });
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
  });
  it('does not query or retry a newer task after the captured failed run was rejected as stale', async () => {
    const { continueTaskPipeline } = await import('./backend');
    mocks.invoke.mockRejectedValueOnce({ code: 'STALE_RUN_ID', message: 'stale execution' });
    await expect(continueTaskPipeline('task', 'old-run')).rejects.toMatchObject({ code: 'STALE_RUN_ID' });
    expect(mocks.invoke).toHaveBeenCalledOnce();
    expect(mocks.invoke).toHaveBeenCalledWith('start_gui_task', { taskId: 'task', retryFailedRunId: 'old-run' });
  });
  it.each(['saved-run', undefined])('continues through the existing project endpoint with run identity %s', async expectedRunId => {
    const result = { projectId: 'project' };
    mocks.invoke.mockResolvedValueOnce({ accepted: true, task_id: 'task', run_id: 'next-run', status: 'starting', revision: 1 })
      .mockResolvedValueOnce({ task_id: 'task', run_id: 'next-run', revision: 1, status: 'completed', result });
    await expect(resumePipeline('project', expectedRunId)).resolves.toEqual(result);
    expect(mocks.invoke).toHaveBeenNthCalledWith(1, 'resume_gui_task', {
      projectId: 'project', ...(expectedRunId ? { expectedRunId } : {}),
    });
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
  });
  it("preserves new failure classification and native termination when waiting for a task", async () => {
    const termination = { exitCode: -1073740940, windowsStatus: "0xC0000374" };
    mocks.invoke.mockResolvedValueOnce({ task_id: 'task' }).mockResolvedValueOnce({ accepted: true, task_id: 'task', run_id: 'run', status: 'starting', revision: 1 }).mockResolvedValueOnce({ task_id: 'task', run_id: 'run', revision: 1, status: 'failed', project_id: 'project',
      error: { message: 'Could not register image', failed_stage: 'reconstructing', engine: 'colmap', classification: 'colmap_crash', classification_version: 1, classification_is_heuristic: false, termination } });
    await expect(startPipeline('E:/input.mp4', 'balanced', 'E:/projects', false)).rejects.toMatchObject({ code: 'pipeline_failed', failureKind: 'colmap_crash', classificationVersion: 1, classificationIsHeuristic: false, termination });
    expect(mocks.invoke).toHaveBeenCalledTimes(3);
  });
  it.each(['sequential', 'exhaustive', undefined] as const)('forwards image matching %s before starting exactly one task', async imageMatchingStrategy => {
    const result = { projectId: 'project' };
    mocks.invoke.mockResolvedValueOnce({ task_id: 'task' }).mockResolvedValueOnce({ accepted: true, task_id: 'task', run_id: 'run', status: 'starting', revision: 1 }).mockResolvedValueOnce({ task_id: 'task', run_id: 'run', revision: 1, status: 'completed', result });
    await expect(startPipeline('E:/photos', 'balanced', 'E:/projects', true, 'draft', 'equirectangular', 'identity', imageMatchingStrategy)).resolves.toEqual(result);
    expect(mocks.invoke).toHaveBeenNthCalledWith(1, 'create_gui_task', { path: 'E:/photos', quality: 'balanced', projectsRoot: 'E:/projects', plannerEnabled: true, workspaceTaskId: 'draft', inputProjection: 'equirectangular', inputSourceIdentity: 'identity', imageMatchingStrategy });
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, 'start_gui_task', { taskId: 'task' });
    expect(mocks.invoke).toHaveBeenCalledTimes(3);
  });
  it("publishes fast metadata before planning and preserves the legacy replanning interface", async () => {
    const metadata = { inputType: 'video', video: null, imageSequence: null, projectionDetection: { sourceIdentity: 'fresh', confirmedErp: false, requiresChoice: false, supportedErp: false, reason: '', previewJpeg: [] } };
    mocks.invoke.mockResolvedValueOnce(metadata);
    const analyzed = vi.fn();
    await probeAndPlan("E:/clip.mp4", "balanced", true, undefined, analyzed);
    expect(mocks.invoke).toHaveBeenNthCalledWith(1, 'analyze_input', { path: 'E:/clip.mp4' });
    expect(analyzed).toHaveBeenCalledWith(metadata);
    expect(mocks.invoke).toHaveBeenLastCalledWith("probe_and_plan", { path: "E:/clip.mp4", quality: "balanced", plannerEnabled: true, inputAnalysis: metadata });
    const videoAnalysis = {
      video: { duration: 10, width: 200, height: 100, fps: 30, totalFrames: 300, codec: "h264", rotation: 0, pixelFormat: "yuv420p", hasAlpha: false },
      projectionDetection: { sourceIdentity: "same-source", confirmedErp: true, requiresChoice: false, supportedErp: true, reason: "", previewJpeg: [] },
    };
    await probeAndPlan("E:/clip.mp4", "fast", false, videoAnalysis);
    expect(mocks.invoke).toHaveBeenLastCalledWith("probe_and_plan", { path: "E:/clip.mp4", quality: "fast", plannerEnabled: false, videoAnalysis });
  });
  it("submits only opaque diagnostic tokens, never frontend paths or report payloads", async () => {
    await prepareErrorReport("failure-token"); expect(mocks.invoke).toHaveBeenLastCalledWith("prepare_error_report", {failureId:"failure-token"});
    await sendErrorReport("draft-token"); expect(mocks.invoke).toHaveBeenLastCalledWith("send_error_report", {draftId:"draft-token"});
  });
  it("sends validated export intentions instead of arbitrary paths or video dimensions",async()=>{
    await beginGaussianVideoExport("id");expect(mocks.invoke).toHaveBeenLastCalledWith("begin_gaussian_video_export",{projectId:"id",orientation:"portrait"});
    await beginGaussianVideoExport("id","landscape");expect(mocks.invoke).toHaveBeenLastCalledWith("begin_gaussian_video_export",{projectId:"id",orientation:"landscape"});
    const view={target:[0,0,0] as [number,number,number],yaw:0,pitch:0,distance:5,horizontalFrameOffset:0,projection:0,orthoHeight:5,orthographicView:null,fov:52,nearClip:0.01,farClip:10000};
    await beginGaussianHtmlExport("id",4,view,"en");expect(mocks.invoke).toHaveBeenLastCalledWith("begin_gaussian_html_export",{projectId:"id",editRevision:4,view,locale:"en"});
    await commitGaussianHtmlExport("token");expect(mocks.invoke).toHaveBeenLastCalledWith("commit_gaussian_html_export",{exportId:"token"});
    await cancelGaussianHtmlExport("token");expect(mocks.invoke).toHaveBeenLastCalledWith("cancel_gaussian_html_export",{exportId:"token"});
  });
  beforeEach(() => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    mocks.invoke.mockReset();
    mocks.confirm.mockReset();
  });

  it("does not invoke the acceleration command outside Tauri", async () => {
    await expect(checkColmapAcceleration()).resolves.toBeNull();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("uses validated backend commands for runtime state and project folders", async () => {
    (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    mocks.invoke.mockResolvedValueOnce({ pipelineRunning: false, previewProjectId: null, taskAcceleration: null });
    await expect(getAppRuntimeStatus()).resolves.toEqual({ pipelineRunning: false, previewProjectId: null, taskAcceleration: null });
    await revealProject({ id: "project-id" } as Parameters<typeof revealProject>[0]);
    await revealProjectLogs("project-id");

    expect(mocks.invoke).toHaveBeenNthCalledWith(1, "get_app_runtime_status");
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, "open_project_location", { projectId: "project-id", location: "project" });
    expect(mocks.invoke).toHaveBeenNthCalledWith(3, "open_project_location", { projectId: "project-id", location: "logs" });
  });

  it("classifies a native drop through the dedicated path-only command", async () => {
    mocks.invoke.mockResolvedValueOnce({ inputType: "video" });
    await expect(classifyDroppedInput("E:\\素材\\clip.mov")).resolves.toEqual({ inputType: "video" });
    expect(mocks.invoke).toHaveBeenCalledWith("classify_dropped_input", { path: "E:\\素材\\clip.mov" });
  });

  it("shows localized capture advice and explicit continue/cancel labels for small image sets", async () => {
    localStorage.setItem("ooo-splat-language", "zh-CN");
    mocks.confirm.mockResolvedValueOnce(false);
    await expect(confirmSmallImageSequence(29)).resolves.toBe(false);
    expect(mocks.confirm).toHaveBeenCalledWith(
      expect.stringContaining("AI 生成图片"),
      expect.objectContaining({ title: "图片数量较少", kind: "warning", okLabel: "继续生成", cancelLabel: "取消生成" }),
    );
    expect(mocks.confirm.mock.calls[0][0]).toContain("60%–80%");
  });
});
