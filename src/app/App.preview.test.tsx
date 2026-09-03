// @vitest-environment jsdom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../stores/appStore";
import { useGaussianTransformStore } from "../stores/gaussianTransformStore";
import type { ProjectSummary } from "../types/pipeline";

const mocks = vi.hoisted(() => ({
  prepareGaussianPreview: vi.fn(),
  releaseGaussianPreview: vi.fn(),
  resumePipeline: vi.fn(),
  getProjectOverview: vi.fn(),
  initializeTelemetry: vi.fn(),
  setTelemetryConsent: vi.fn(),
  notifyPreviewDisposed: vi.fn(),
}));

vi.mock("../lib/backend", () => ({
  cancelPipeline: vi.fn(),
  checkEngines: vi.fn().mockResolvedValue([]),
  confirmAndDeleteProject: vi.fn().mockResolvedValue(false),
  getProjectOverview: mocks.getProjectOverview,
  initializeTelemetry: mocks.initializeTelemetry,
  onPipelineEvent: vi.fn().mockResolvedValue(() => undefined),
  prepareGaussianPreview: mocks.prepareGaussianPreview,
  probeAndPlan: vi.fn(),
  releaseGaussianPreview: mocks.releaseGaussianPreview,
  resumePipeline: mocks.resumePipeline,
  revealProject: vi.fn(),
  selectProjectsRoot: vi.fn(),
  selectVideo: vi.fn(),
  setProjectsRoot: vi.fn(),
  setTelemetryConsent: mocks.setTelemetryConsent,
  startPipeline: vi.fn(),
}));

vi.mock("../components/GaussianViewer", () => ({
  GaussianViewer: ({ onExit, onDisposed }: { onExit: () => void | Promise<void>; onDisposed: (projectId: string) => void }) => <section className="preview-workspace"><h1>高斯泼溅预览</h1><button type="button" onClick={() => {
    void onExit();
    mocks.notifyPreviewDisposed(onDisposed, "11111111-1111-1111-1111-111111111111");
  }}>返回任务</button></section>,
}));

import { App } from "./App";

const project: ProjectSummary = {
  id: "11111111-1111-1111-1111-111111111111",
  name: "示例项目",
  status: "completed",
  projectPath: "E:\\Projects\\示例项目",
  finalPly: "E:\\Projects\\示例项目\\final.ply",
  fileSize: 73_729_603,
  splatCount: 312_407,
  createdAt: "2026-08-22T14:00:00Z",
  completedAt: "2026-08-22T15:00:00Z",
  durationMs: 3_600_000,
  quality: "balanced",
  sourceName: "input.mp4",
  registeredRatio: 0.9,
  points3d: 10_000,
  failureMessage: null,
};

const flush = async () => {
  await act(async () => {
    await Promise.resolve();
    await new Promise((resolve) => window.setTimeout(resolve, 0));
  });
};

describe("App preview workspace", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(async () => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    if (!window.requestAnimationFrame) {
      window.requestAnimationFrame = (callback) => window.setTimeout(() => callback(performance.now()), 0);
      window.cancelAnimationFrame = (handle) => window.clearTimeout(handle);
    }
    useGaussianTransformStore.getState().close();
    useAppStore.setState({
      videoPath: null, projectsRoot: "E:\\Projects", projects: [], quality: "balanced", colmapAcceleration: null,
      video: null, plan: null, estimate: null, engines: [], phase: "idle", progress: 0, progressMessage: "",
      latestEvent: null, events: [], result: null, error: null,
    });
    mocks.prepareGaussianPreview.mockReset();
    mocks.releaseGaussianPreview.mockReset().mockResolvedValue(undefined);
    mocks.resumePipeline.mockReset().mockResolvedValue({
      projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly,
      fileSize: project.fileSize, splatCount: project.splatCount, inputImages: 100,
      registeredImages: 90, registeredRatio: 0.9, points3d: 10_000,
      durationMs: project.durationMs, completedAt: project.completedAt, warning: null,
      logsDirectory: `${project.projectPath}\\logs`,
    });
    mocks.getProjectOverview.mockReset().mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [project] });
    mocks.initializeTelemetry.mockReset().mockResolvedValue({ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" });
    mocks.setTelemetryConsent.mockReset().mockResolvedValue({ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" });
    mocks.notifyPreviewDisposed.mockReset().mockImplementation((callback: (projectId: string) => void, projectId: string) => {
      queueMicrotask(() => callback(projectId));
    });
    mocks.prepareGaussianPreview.mockResolvedValue({
      projectId: project.id,
      modelPath: project.finalPly,
      assetPath: `${project.projectPath}\\work\\preview\\preview-session-a.ply`,
      assetUrl: "http://asset.localhost/final.ply",
      format: "ply",
      fileSize: project.fileSize,
      splatCount: project.splatCount,
      transform: { position: [0, 0, 0], rotation: [0, 0, 0], scale: 1 },
    });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => { root.render(<App />); });
    await flush();
  });

  afterEach(async () => {
    await act(async () => { root.unmount(); });
    container.remove();
  });

  it("shows the 0.3.0 branding and a start action without a trailing arrow", () => {
    expect(container.querySelector(".brand-name")?.textContent).toBe("OOOSplat");
    expect(container.querySelector(".version-tag")?.textContent).toBe("LOCAL / 0.3.0");
    const startButton = container.querySelector(".primary-action");
    expect(startButton?.textContent?.trim()).toBe("开始生成");
    expect(startButton?.querySelectorAll("svg")).toHaveLength(1);
  });

  it("offers to continue an unfinished project from its checkpoint", async () => {
    const unfinished = { ...project, status: "cancelled" as const, finalPly: null, completedAt: null };
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });

    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续任务");
    await act(async () => { resumeButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(mocks.resumePipeline).toHaveBeenCalledWith(project.id);
  });

  it("shows only the task panes until a completed project is opened", async () => {
    expect(container.textContent).toContain("01 创建新任务");
    expect(container.textContent).toContain("02 历史任务");
    expect(container.querySelector(".preview-workspace")).toBeNull();

    const controlPane = container.querySelector<HTMLElement>(".control-pane");
    const projectsPane = container.querySelector<HTMLElement>(".projects-pane");
    if (controlPane) controlPane.scrollTop = 48;
    if (projectsPane) projectsPane.scrollTop = 96;

    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.querySelector(".topbar")).toBeNull();
    expect(container.textContent).toContain("高斯泼溅预览");
    expect(container.textContent).not.toContain("01 创建新任务");

    const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
    await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain("01 创建新任务");
    expect(container.textContent).toContain("02 历史任务");
    expect(container.querySelector<HTMLElement>(".control-pane")?.scrollTop).toBe(48);
    expect(container.querySelector<HTMLElement>(".projects-pane")?.scrollTop).toBe(96);
    expect(mocks.releaseGaussianPreview).toHaveBeenCalledWith(project.id);
  });

  it("keeps the task workspace visible when preview preparation fails", async () => {
    mocks.prepareGaussianPreview.mockRejectedValueOnce(new Error("PLY 无法读取"));
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain("01 创建新任务");
    expect(container.textContent).toContain("02 历史任务");
    expect(container.textContent).toContain("PLY 无法读取");
    expect(container.querySelector(".preview-workspace")).toBeNull();
  });

  it("restores the task workspace before preview resource release settles", async () => {
    let finishRelease: (() => void) | undefined;
    mocks.releaseGaussianPreview.mockImplementationOnce(() => new Promise<void>((resolve) => { finishRelease = resolve; }));
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
    await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain("01 创建新任务");
    expect(container.textContent).toContain("02 历史任务");
    expect(container.querySelector(".preview-workspace")).toBeNull();
    expect(mocks.releaseGaussianPreview).toHaveBeenCalledWith(project.id);

    await act(async () => { finishRelease?.(); });
    await flush();
    expect(useGaussianTransformStore.getState().descriptor).toBeNull();
  });

  it("does not revoke the PLY permission before the renderer is disposed", async () => {
    let finishDisposal: (() => void) | undefined;
    mocks.notifyPreviewDisposed.mockImplementationOnce((callback: (projectId: string) => void, projectId: string) => {
      finishDisposal = () => callback(projectId);
    });
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
    await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();
    expect(container.textContent).toContain("01 创建新任务");
    expect(mocks.releaseGaussianPreview).not.toHaveBeenCalled();

    await act(async () => { finishDisposal?.(); });
    await flush();
    expect(mocks.releaseGaussianPreview).toHaveBeenCalledWith(project.id);
  });

  it("keeps the task workspace visible when preview resource release fails", async () => {
    mocks.releaseGaussianPreview.mockRejectedValueOnce(new Error("预览资源释放失败"));
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
    await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain("01 创建新任务");
    expect(container.textContent).toContain("02 历史任务");
    expect(container.textContent).toContain("预览资源释放失败");
    expect(container.querySelector(".preview-workspace")).toBeNull();
  });

  it("can dispose and reopen the same project repeatedly", async () => {
    let session = 0;
    mocks.prepareGaussianPreview.mockImplementation(async () => ({
      projectId: project.id,
      modelPath: project.finalPly,
      assetPath: `${project.projectPath}\\work\\preview\\preview-session-${session + 1}.ply`,
      assetUrl: `http://asset.localhost/final.ply?previewSession=${++session}`,
      format: "ply",
      fileSize: project.fileSize,
      splatCount: project.splatCount,
      transform: { position: [0, 0, 0], rotation: [0, 0, 0], scale: 1 },
    }));

    for (let cycle = 1; cycle <= 3; cycle += 1) {
      const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
      await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
      await flush();
      expect(useGaussianTransformStore.getState().descriptor?.assetUrl).toContain(`previewSession=${cycle}`);

      const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
      await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
      await flush();
      expect(container.textContent).toContain("01 创建新任务");
      expect(useGaussianTransformStore.getState().descriptor).toBeNull();
    }

    expect(mocks.prepareGaussianPreview).toHaveBeenCalledTimes(3);
    expect(mocks.releaseGaussianPreview).toHaveBeenCalledTimes(3);
  });
});
