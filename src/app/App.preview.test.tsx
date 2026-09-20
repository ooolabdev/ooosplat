// @vitest-environment jsdom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../stores/appStore";
import { useGaussianTransformStore } from "../stores/gaussianTransformStore";
import { LanguageProvider } from "../i18n";
import type { ProjectSummary } from "../types/pipeline";

const mocks = vi.hoisted(() => ({
  cancelPipeline: vi.fn(),
  estimateProjectRuntime: vi.fn(),
  exportPly: vi.fn(),
  prepareGaussianPreview: vi.fn(),
  releaseGaussianPreview: vi.fn(),
  revealFile: vi.fn(),
  resumePipeline: vi.fn(),
  getAppRuntimeStatus: vi.fn(),
  getProjectOverview: vi.fn(),
  getAppSettings: vi.fn(),
  initializeTelemetry: vi.fn(),
  setTelemetryConsent: vi.fn(),
  selectVideo: vi.fn(),
  selectImageSequence: vi.fn(),
  probeAndPlan: vi.fn(),
  confirmLargeImageSequence: vi.fn(),
  startPipeline: vi.fn(),
  revealProject: vi.fn(),
  revealProjectLogs: vi.fn(),
  notifyPreviewDisposed: vi.fn(),
}));

vi.mock("../lib/backend", () => ({
  cancelPipeline: mocks.cancelPipeline,
  checkEngines: vi.fn().mockResolvedValue([]),
  confirmAndDeleteProject: vi.fn().mockResolvedValue(false),
  confirmLargeImageSequence: mocks.confirmLargeImageSequence,
  estimateProjectRuntime: mocks.estimateProjectRuntime,
  exportPly: mocks.exportPly,
  getAppRuntimeStatus: mocks.getAppRuntimeStatus,
  getProjectOverview: mocks.getProjectOverview,
  getAppSettings: mocks.getAppSettings,
  initializeTelemetry: mocks.initializeTelemetry,
  onPipelineEvent: vi.fn().mockResolvedValue(() => undefined),
  prepareGaussianPreview: mocks.prepareGaussianPreview,
  probeAndPlan: mocks.probeAndPlan,
  releaseGaussianPreview: mocks.releaseGaussianPreview,
  resumePipeline: mocks.resumePipeline,
  revealProject: mocks.revealProject,
  revealProjectLogs: mocks.revealProjectLogs,
  revealFile: mocks.revealFile,
  selectProjectsRoot: vi.fn(),
  selectImageSequence: mocks.selectImageSequence,
  selectVideo: mocks.selectVideo,
  setProjectsRoot: vi.fn(),
  setPlannerPreference: vi.fn(),
  setTelemetryConsent: mocks.setTelemetryConsent,
  startPipeline: mocks.startPipeline,
}));

vi.mock("../components/GaussianViewer", () => ({
  GaussianViewer: ({ previewSessionId, onExit, onDisposed }: { previewSessionId: number; onExit: () => void | Promise<void>; onDisposed: (projectId: string, previewSessionId: number) => void }) => <section className="preview-workspace"><h1>高斯泼溅预览</h1><button type="button" onClick={() => {
    void onExit();
    mocks.notifyPreviewDisposed(onDisposed, "11111111-1111-1111-1111-111111111111", previewSessionId);
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
    // jsdom does not implement scrollIntoView, and starting a run renders the live log,
    // whose auto-scroll effect then calls it.
    Element.prototype.scrollIntoView = vi.fn();
    window.localStorage.setItem("ooo-splat-language", "zh-CN");
    if (!window.requestAnimationFrame) {
      window.requestAnimationFrame = (callback) => window.setTimeout(() => callback(performance.now()), 0);
      window.cancelAnimationFrame = (handle) => window.clearTimeout(handle);
    }
    useGaussianTransformStore.getState().close();
    useAppStore.setState({
      inputPath: null, inputType: "video", projectsRoot: "E:\\Projects", projects: [], quality: "balanced", colmapAcceleration: null,
      video: null, imageSequence: null, plan: null, estimate: null, engines: [], phase: "idle", progress: 0, progressMessage: "",
      latestEvent: null, events: [], result: null, error: null,
    });
    mocks.prepareGaussianPreview.mockReset();
    mocks.cancelPipeline.mockReset().mockResolvedValue(undefined);
    mocks.estimateProjectRuntime.mockReset().mockResolvedValue({
      estimatedMs: 4_000_000,
      lowerBoundMs: 3_000_000,
      upperBoundMs: 5_000_000,
      confidence: "medium",
      sampleCount: 3,
      basis: "按同档位历史任务校准",
    });
    mocks.releaseGaussianPreview.mockReset().mockResolvedValue(undefined);
    mocks.getAppRuntimeStatus.mockReset().mockImplementation(async () => ({
      pipelineRunning: false,
      previewProjectId: useGaussianTransformStore.getState().descriptor?.projectId ?? null,
    }));
    mocks.exportPly.mockReset().mockResolvedValue("E:\\Exports\\final.ply");
    mocks.revealFile.mockReset().mockResolvedValue(undefined);
    mocks.revealProject.mockReset().mockResolvedValue(undefined);
    mocks.revealProjectLogs.mockReset().mockResolvedValue(undefined);
    mocks.resumePipeline.mockReset().mockResolvedValue({
      projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly,
      fileSize: project.fileSize, splatCount: project.splatCount, inputImages: 100,
      registeredImages: 90, registeredRatio: 0.9, points3d: 10_000,
      durationMs: project.durationMs, completedAt: project.completedAt, warning: null,
      logsDirectory: `${project.projectPath}\\logs`,
    });
    mocks.getProjectOverview.mockReset().mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [project] });
    mocks.getAppSettings.mockReset().mockResolvedValue({ projectsRoot: "E:\\Projects", plannerEnabled: false, plannerPreference: "askEachTime" });
    mocks.initializeTelemetry.mockReset().mockResolvedValue({ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" });
    mocks.setTelemetryConsent.mockReset().mockResolvedValue({ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" });
    mocks.selectVideo.mockReset().mockResolvedValue(null);
    mocks.selectImageSequence.mockReset().mockResolvedValue(null);
    mocks.probeAndPlan.mockReset();
    mocks.confirmLargeImageSequence.mockReset().mockResolvedValue(true);
    mocks.startPipeline.mockReset();
    mocks.notifyPreviewDisposed.mockReset().mockImplementation((callback: (projectId: string, previewSessionId: number) => void, projectId: string, previewSessionId: number) => {
      queueMicrotask(() => callback(projectId, previewSessionId));
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
    await act(async () => { root.render(<LanguageProvider><App /></LanguageProvider>); });
    await flush();
  });

  afterEach(async () => {
    await act(async () => { root.unmount(); });
    container.remove();
    window.localStorage.clear();
  });

  it("shows the current package version and a start action without a trailing arrow", () => {
    expect(container.querySelector(".brand-name")?.textContent).toBe("OOOSplat");
    expect(container.querySelector(".version-tag")?.textContent).toBe("LOCAL / 0.4.1");
    const startButton = container.querySelector(".primary-action");
    expect(startButton?.textContent?.trim()).toBe("开始生成");
    expect(startButton?.querySelectorAll("svg")).toHaveLength(1);
  });

  it("switches the complete task workspace to English without reloading", async () => {
    const languageButton = container.querySelector<HTMLButtonElement>(".language-action");
    expect(languageButton?.textContent).toContain("EN");
    expect(languageButton?.title).toBe("中英文切换 / Switch language");

    await act(async () => languageButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    expect(container.textContent).toContain("01 Create New Task");
    expect(container.textContent).toContain("02 Task History");
    expect(container.textContent).toContain("Start Generation");
    expect(container.textContent).toContain("Checking bundled engines");
    expect(languageButton?.textContent).toContain("中文");
    expect(languageButton?.title).toBe("中英文切换 / Switch language");
    expect(window.localStorage.getItem("ooo-splat-language")).toBe("en");
  });

  it("shows automatic mask extraction when the selected video has alpha", async () => {
    act(() => useAppStore.setState({
      video: {
        duration: 10,
        width: 1920,
        height: 1080,
        fps: 30,
        totalFrames: 300,
        codec: "prores",
        rotation: 0,
        pixelFormat: "yuva444p10le",
        hasAlpha: true,
      },
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
    }));
    await flush();

    expect(container.textContent).toContain("检测到 Alpha 通道");
    expect(container.textContent).toContain("将自动提取透明画面和 COLMAP Mask");

    act(() => useAppStore.setState({
      video: {
        duration: 10,
        width: 1920,
        height: 1080,
        fps: 30,
        totalFrames: 300,
        codec: "h264",
        rotation: 0,
        pixelFormat: "yuv420p",
        hasAlpha: false,
      },
    }));
    await flush();
    expect(container.textContent).not.toContain("将自动提取透明画面和 COLMAP Mask");
  });

  it("hides the previous live process after selecting new media but keeps new analysis notices", async () => {
    act(() => useAppStore.setState({
      phase: "failed",
      progress: 64,
      progressMessage: "old task output",
      latestEvent: {
        sequence: 1, timestamp: new Date().toISOString(), kind: "log", level: "error", stage: "failed",
        engine: "system", progress: 64, stageProgress: null, indeterminate: false, message: "old task output",
        current: null, total: null, unit: null, elapsedMs: 1_000, acceleration: null,
      },
      events: [{
        sequence: 1, timestamp: new Date().toISOString(), kind: "log", level: "error", stage: "failed",
        engine: "system", progress: 64, stageProgress: null, indeterminate: false, message: "old task output",
        current: null, total: null, unit: null, elapsedMs: 1_000, acceleration: null,
      }],
    }));
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\alpha.mov");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 10, width: 1920, height: 1080, fps: 30, totalFrames: 300, codec: "prores", rotation: 0, pixelFormat: "yuva444p10le", hasAlpha: true },
      imageSequence: null,
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
      estimate: { estimatedMs: 120_000, lowerBoundMs: 60_000, upperBoundMs: 180_000, confidence: "low", sampleCount: 0, basis: "video" },
    });

    expect(container.querySelector(".live-process")).not.toBeNull();
    await act(async () => { container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click(); });
    await flush();

    expect(container.querySelector(".live-process")).toBeNull();
    expect(container.querySelector(".alpha-source-status")).not.toBeNull();
    expect(container.textContent).not.toContain("old task output");
  });

  it("shows a new-media analysis error without restoring the previous process log", async () => {
    act(() => useAppStore.setState({
      events: [{
        sequence: 1, timestamp: new Date().toISOString(), kind: "log", level: "info", stage: "completed",
        engine: "system", progress: 100, stageProgress: 100, indeterminate: false, message: "old completed log",
        current: null, total: null, unit: null, elapsedMs: 1_000, acceleration: null,
      }],
    }));
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\broken.mov");
    mocks.probeAndPlan.mockRejectedValueOnce(new Error("Unable to read the selected media"));

    await act(async () => { container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click(); });
    await flush();

    expect(container.querySelector(".live-process")).toBeNull();
    expect(container.querySelector(".inline-error")?.textContent).toContain("Unable to read the selected media");
    expect(container.textContent).not.toContain("old completed log");
  });

  it("offers to continue an unfinished project from its checkpoint", async () => {
    const unfinished = { ...project, status: "cancelled" as const, finalPly: null, completedAt: null };
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });

    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续任务");
    await act(async () => { resumeButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(mocks.resumePipeline).toHaveBeenCalledWith(project.id);
    expect(mocks.estimateProjectRuntime).toHaveBeenCalledWith(project.id);
  });

  it("shows actionable mapper guidance and opens the validated project log folder", async () => {
    const unfinished = { ...project, status: "failed" as const, finalPly: null, completedAt: null };
    mocks.resumePipeline.mockRejectedValueOnce({
      code: "pipeline_failed",
      message: "Could not find a good initial image pair",
      failedStage: "reconstructing",
      engine: "colmap",
      failureKind: "mapper_source",
      projectId: project.id,
    });
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });

    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续任务");
    await act(async () => { resumeButton?.click(); });
    await flush();

    const dialog = container.querySelector<HTMLElement>(".failure-guidance-dialog");
    expect(dialog).not.toBeNull();
    expect(dialog?.textContent).toContain("COLMAP");
    expect(dialog?.textContent).toContain("Could not find a good initial image pair");

    await act(async () => { dialog?.querySelector<HTMLButtonElement>("button.secondary")?.click(); });
    await flush();
    expect(mocks.revealProjectLogs).toHaveBeenCalledWith(project.id);

    await act(async () => { dialog?.querySelector<HTMLButtonElement>("button.primary")?.click(); });
    await flush();
    expect(mocks.resumePipeline).toHaveBeenCalledTimes(2);
  });

  it("classifies Brush dataset read errors separately and translates the open dialog", async () => {
    const unfinished = { ...project, status: "failed" as const, finalPly: null, completedAt: null };
    mocks.resumePipeline.mockRejectedValueOnce({
      code: "pipeline_failed",
      message: "IO error while loading dataset: early eof",
      failedStage: "trainingSplats",
      engine: "brush",
      failureKind: "brush_dataset",
      projectId: project.id,
    });
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });
    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续任务");
    await act(async () => { resumeButton?.click(); });
    await flush();

    const heading = container.querySelector<HTMLElement>(".failure-guidance-dialog h2")!;
    const chineseHeading = heading.textContent;
    expect(container.querySelector(".failure-guidance-dialog")?.textContent).toContain("early eof");
    await act(async () => { container.querySelector<HTMLButtonElement>(".language-action")?.click(); });
    expect(heading.textContent).not.toBe(chineseHeading);
  });

  it("does not show failure guidance for a cancelled run", async () => {
    const unfinished = { ...project, status: "cancelled" as const, finalPly: null, completedAt: null };
    mocks.resumePipeline.mockRejectedValueOnce({ code: "cancelled", message: "cancelled", projectId: project.id });
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });
    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续任务");
    await act(async () => { resumeButton?.click(); });
    await flush();
    expect(container.querySelector(".failure-guidance-dialog")).toBeNull();
  });

  it("blocks the interface while a slow cancellation is still terminating processes", async () => {
    act(() => useAppStore.setState({ phase: "running" }));
    await flush();

    const cancelButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "取消任务并终止所有进程");
    await act(async () => { cancelButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    expect(mocks.cancelPipeline).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".cancellation-backdrop")).toBeNull();

    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 350)); });
    expect(container.querySelector(".cancellation-backdrop")).not.toBeNull();
    expect(container.textContent).toContain("正在关闭当前阶段及其子进程");

    act(() => useAppStore.setState({ phase: "cancelled" }));
    await flush();
    expect(container.querySelector(".cancellation-backdrop")).toBeNull();
  });

  it("shows the estimated total generation time after video analysis", async () => {
    act(() => useAppStore.setState({
      video: {
        duration: 10,
        width: 1920,
        height: 1080,
        fps: 30,
        totalFrames: 300,
        codec: "h264",
        rotation: 0,
        pixelFormat: "yuv420p",
        hasAlpha: false,
      },
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
      estimate: {
        estimatedMs: 120_000,
        lowerBoundMs: 60_000,
        upperBoundMs: 180_000,
        confidence: "medium",
        sampleCount: 3,
        basis: "本机历史任务校准",
      },
    }));
    await flush();

    expect(container.textContent).toContain("预计时长");
    expect(container.textContent).toContain("约 2 分 0 秒");
    expect(container.querySelector('[title="本机历史任务校准"]')).not.toBeNull();
  });

  it("labels Brush heartbeat progress as an estimate", async () => {
    act(() => useAppStore.setState({
      phase: "running",
      progress: 79,
      progressMessage: "Brush 训练中 · 估算进度 50%",
      latestEvent: {
        sequence: 7,
        timestamp: new Date().toISOString(),
        kind: "heartbeat",
        level: "info",
        stage: "TrainingSplats",
        engine: "brush",
        progress: 79,
        stageProgress: 50,
        indeterminate: false,
        message: "Brush 训练中 · 估算进度 50%",
        current: null,
        total: 15_000,
        unit: "estimated_progress",
        elapsedMs: 120_000,
        acceleration: null,
      },
    }));
    await flush();

    expect(container.querySelector(".current-message")?.textContent).toBe("Brush 训练中 · 估算进度 50%");
    expect(Array.from(container.querySelectorAll(".process-metrics b"), (node) => node.textContent)).not.toContain("估算 50%");
    expect(container.textContent).not.toContain("15,000 / 15,000");
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
    mocks.notifyPreviewDisposed.mockImplementationOnce((callback: (projectId: string, previewSessionId: number) => void, projectId: string, previewSessionId: number) => {
      finishDisposal = () => callback(projectId, previewSessionId);
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

  it("unlocks the project after the preview disposal watchdog and ignores a late callback", async () => {
    let finishDisposal: (() => void) | undefined;
    mocks.notifyPreviewDisposed.mockImplementationOnce((callback: (projectId: string, previewSessionId: number) => void, projectId: string, previewSessionId: number) => {
      finishDisposal = () => callback(projectId, previewSessionId);
    });
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.click(); });
    await flush();

    vi.useFakeTimers();
    try {
      const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
      await act(async () => { backButton?.click(); });
      await act(async () => { await vi.advanceTimersByTimeAsync(8_001); });

      const reopenedButton = [...container.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === "预览");
      expect(reopenedButton?.disabled).toBe(false);
      expect(useGaussianTransformStore.getState().descriptor).toBeNull();
      expect(mocks.releaseGaussianPreview).toHaveBeenCalledTimes(1);

      await act(async () => { finishDisposal?.(); });
      expect(mocks.releaseGaussianPreview).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
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

  it("selects the input type on the left before opening the matching picker", async () => {
    mocks.selectImageSequence.mockResolvedValueOnce("E:\\Photos\\object");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "images",
      video: null,
      imageSequence: {
        imageCount: 24,
        width: 1920,
        height: 1080,
        hasAlpha: true,
        requiresLargeSequenceConfirmation: false,
      },
      plan: { retentionRatio: 1, samplingFps: 0, estimatedFrames: 24 },
      estimate: {
        estimatedMs: 120_000,
        lowerBoundMs: 80_000,
        upperBoundMs: 180_000,
        confidence: "low",
        sampleCount: 0,
        basis: "图片序列",
      },
    });

    const toggle = container.querySelector<HTMLButtonElement>('[aria-label="选择输入素材类型"]');
    const picker = container.querySelector<HTMLButtonElement>(".input-picker > .path-picker");
    expect(toggle?.textContent).toContain("视频");
    await act(async () => picker?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(mocks.selectVideo).toHaveBeenCalledOnce();

    await act(async () => toggle?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    const imageOption = [...container.querySelectorAll(".input-picker-menu button")].find((button) =>
      button.textContent?.includes("图片"),
    );
    await act(async () => imageOption?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(mocks.selectImageSequence).not.toHaveBeenCalled();
    expect(toggle?.textContent).toContain("图片");

    await act(async () => picker?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await flush();

    expect(mocks.selectImageSequence).toHaveBeenCalledOnce();
    expect(container.textContent).toContain("24 张");
    expect(container.textContent).toContain("生成时将精确检测透明度");
    expect(container.querySelectorAll(".input-picker")).toHaveLength(1);
  });

  it("requires confirmation before exhaustive matching more than 500 images", async () => {
    await act(async () => {
      useAppStore.setState({
        inputPath: "E:\\Photos\\large",
        inputType: "images",
        imageSequence: {
          imageCount: 501,
          width: 1920,
          height: 1080,
          hasAlpha: false,
          requiresLargeSequenceConfirmation: true,
        },
        plan: { retentionRatio: 1, samplingFps: 0, estimatedFrames: 501 },
        estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "test" },
      });
    });
    mocks.confirmLargeImageSequence.mockResolvedValueOnce(false);
    await flush();

    const generate = [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("开始生成"));
    await act(async () => generate?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await flush();

    expect(mocks.confirmLargeImageSequence).toHaveBeenCalledWith(501);
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it("renders the completed result card and wires its file actions", async () => {
    const result = await mocks.resumePipeline();
    await act(async () => {
      useAppStore.setState({ phase: "completed", result, projects: [project] });
    });
    await flush();

    expect(container.querySelector(".completion-result")).not.toBeNull();
    const actions = container.querySelectorAll<HTMLButtonElement>(".completion-result-actions button");
    await act(async () => actions[1].click());
    await act(async () => actions[2].click());
    expect(mocks.exportPly).toHaveBeenCalledWith(result);
    expect(mocks.revealProject).toHaveBeenCalledWith(project);
  });

  it("restores the low-registration warning from historical project data", async () => {
    await act(async () => {
      useAppStore.getState().setProjects([{ ...project, registeredRatio: 0.62 }]);
    });
    await flush();
    expect(container.querySelector(".project-quality-warning")?.textContent).toContain("62.0%");
  });
});
