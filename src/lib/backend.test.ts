// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn(),
  invoke: mocks.invoke,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: vi.fn() }));

import { prepareErrorReport,sendErrorReport,beginGaussianVideoExport,beginGaussianHtmlExport,commitGaussianHtmlExport,cancelGaussianHtmlExport,checkColmapAcceleration, classifyDroppedInput, getAppRuntimeStatus, revealProject, revealProjectLogs } from "./backend";

describe("backend browser guards", () => {
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
});
