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
  getSharedTasks: vi.fn(),
  onTaskUpdate: vi.fn(),
  getAppRuntimeStatus: vi.fn(),
  getProjectOverview: vi.fn(),
  getProjectTaskDetail: vi.fn(),
  initializeTelemetry: vi.fn(),
  inspectReshootSource: vi.fn(),
  probeReshootInput: vi.fn(),
  startReshootPipeline: vi.fn(),
  setTelemetryConsent: vi.fn(),
  selectVideo: vi.fn(),
  selectImageSequence: vi.fn(),
  probeAndPlan: vi.fn(),
  confirmLargeImageSequence: vi.fn(),
  confirmSmallImageSequence: vi.fn(),
  startPipeline: vi.fn(),
  revealProject: vi.fn(),
  revealProjectLogs: vi.fn(),
  notifyPreviewDisposed: vi.fn(),
  prepareErrorReport: vi.fn(),
  sendErrorReport: vi.fn(),
  classifyDroppedInput: vi.fn(),
  onInputDragDrop: vi.fn(),
}));

vi.mock("../lib/backend", () => ({
  cancelPipeline: mocks.cancelPipeline,
  prepareErrorReport: mocks.prepareErrorReport,
  sendErrorReport: mocks.sendErrorReport,
  checkEngines: vi.fn().mockResolvedValue([]),
  checkColmapAcceleration: vi.fn().mockResolvedValue(null),
  classifyDroppedInput: mocks.classifyDroppedInput,
  confirmAndDeleteProject: vi.fn().mockResolvedValue(false),
  confirmLargeImageSequence: mocks.confirmLargeImageSequence,
  confirmSmallImageSequence: mocks.confirmSmallImageSequence,
  estimateProjectRuntime: mocks.estimateProjectRuntime,
  exportPly: mocks.exportPly,
  getSharedTasks: mocks.getSharedTasks,
  onTaskUpdate: mocks.onTaskUpdate,
  getAppRuntimeStatus: mocks.getAppRuntimeStatus,
  getProjectOverview: mocks.getProjectOverview,
  getProjectTaskDetail: mocks.getProjectTaskDetail,
  initializeTelemetry: mocks.initializeTelemetry,
  inspectReshootSource: mocks.inspectReshootSource,
  probeReshootInput: mocks.probeReshootInput,
  onPipelineEvent: vi.fn().mockResolvedValue(() => undefined),
  onInputDragDrop: mocks.onInputDragDrop,
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
  setTelemetryConsent: mocks.setTelemetryConsent,
  startPipeline: mocks.startPipeline,
  startReshootPipeline: mocks.startReshootPipeline,
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
    mocks.getSharedTasks.mockReset().mockResolvedValue([]);
    mocks.onTaskUpdate.mockReset().mockResolvedValue(() => undefined);
    useAppStore.setState({
      inputPath: null, inputType: "video", projectsRoot: "E:\\Projects", projects: [], quality: "balanced", colmapAcceleration: null, taskColmapAcceleration: null,
      video: null, imageSequence: null, plan: null, estimate: null, engines: [], phase: "idle", progress: 0, progressMessage: "",
      latestEvent: null, latestRuntime: null, lastEventSequence: 0, events: [], result: null, error: null, errorAt: null,
    });
    mocks.prepareGaussianPreview.mockReset();
    mocks.classifyDroppedInput.mockReset().mockResolvedValue({ inputType: "video" });
    mocks.onInputDragDrop.mockReset().mockResolvedValue(() => undefined);
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
      pipelineRunElapsedMs: 0,
      pipelineElapsedOffsetMs: 0,
      previewProjectId: useGaussianTransformStore.getState().descriptor?.projectId ?? null,
      taskAcceleration: null,
    }));
    mocks.exportPly.mockReset().mockResolvedValue("E:\\Exports\\final.ply");
    mocks.revealFile.mockReset().mockResolvedValue(undefined);
    mocks.revealProject.mockReset().mockResolvedValue(undefined);
    mocks.revealProjectLogs.mockReset().mockResolvedValue(undefined);
    mocks.prepareErrorReport.mockReset();
    mocks.sendErrorReport.mockReset();
    mocks.resumePipeline.mockReset().mockResolvedValue({
      projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly,
      fileSize: project.fileSize, splatCount: project.splatCount, inputImages: 100,
      registeredImages: 90, registeredRatio: 0.9, points3d: 10_000,
      durationMs: project.durationMs, completedAt: project.completedAt, warning: null,
      logsDirectory: `${project.projectPath}\\logs`,
    });
    mocks.getProjectOverview.mockReset().mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [project] });
    mocks.getProjectTaskDetail.mockReset().mockResolvedValue({
      project,
      inputType: "video",
      sourcePath: "E:\\Capture\\input.mp4",
      projectsRoot: "E:\\Projects",
      plannerEnabled: true,
      estimatedFrames: 320,
      stage: "completed",
      progress: 100,
      inputImages: 100,
      registeredImages: 90,
      video: null,
      imageSequence: null,
      sourceProjectId: null,
      logs: [],
    });
    mocks.initializeTelemetry.mockReset().mockResolvedValue({ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" });
    mocks.inspectReshootSource.mockReset().mockResolvedValue({
      projectId: project.id,
      projectName: project.name,
      quality: project.quality,
      plannerEnabled: true,
      cameraId: 1,
      cameraModel: "SIMPLE_RADIAL",
      width: 1920,
      height: 1080,
      sourceImageCount: 100,
      eligible: true,
      reason: null,
    });
    mocks.probeReshootInput.mockReset();
    mocks.startReshootPipeline.mockReset();
    mocks.setTelemetryConsent.mockReset().mockResolvedValue({ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" });
    mocks.selectVideo.mockReset().mockResolvedValue(null);
    mocks.selectImageSequence.mockReset().mockResolvedValue(null);
    mocks.probeAndPlan.mockReset();
    mocks.confirmLargeImageSequence.mockReset().mockResolvedValue(true);
    mocks.confirmSmallImageSequence.mockReset().mockResolvedValue(true);
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
    expect(container.querySelector(".version-tag")?.textContent).toBe("LOCAL / 0.6.0");
    const startButton = container.querySelector(".primary-action");
    expect(startButton?.textContent?.trim()).toBe("开始生成");
    expect(startButton?.querySelectorAll("svg")).toHaveLength(1);
  });

  it("shows shared capture advice beside the material label", () => {
    const trigger = container.querySelector<HTMLButtonElement>(".capture-help-trigger");
    const tooltip = container.querySelector<HTMLElement>(".capture-help-tooltip");
    expect(trigger?.getAttribute("aria-label")).toBe("拍摄建议");
    expect(trigger?.getAttribute("aria-describedby")).toBe(tooltip?.id);
    expect(tooltip?.getAttribute("role")).toBe("tooltip");
    expect(tooltip?.textContent).toContain("正面、侧面、背面以及高低视角");
    expect(tooltip?.textContent).toContain("60%–80% 重叠");
    expect(tooltip?.textContent).toContain("固定镜头、焦距、分辨率和曝光");
    expect(tooltip?.textContent).toContain("AI 生成图片容易出现");
  });

  it("keeps settings available and persists the runtime monitor preference locally", async () => {
    const settingsButton = container.querySelector<HTMLButtonElement>(".settings-action:not(.language-action)")!;
    expect(settingsButton).not.toBeNull();
    await act(async () => settingsButton.click());
    expect(container.querySelector(".settings-dialog")).not.toBeNull();
    expect(container.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toContain("界面");
    await act(async () => container.querySelector<HTMLButtonElement>(".settings-switch")!.click());
    expect(window.localStorage.getItem("ooo-splat-ui-preferences-v1")).toContain('"showRuntimePanel":true');
  });

  it("adds, selects, and deletes independent standby drafts while retaining one", async () => {
    expect(Array.from(container.querySelectorAll(".group-toggle > span"), (node) => node.textContent)).toEqual(["新任务", "已完成", "未完成"]);
    const add = container.querySelector<HTMLButtonElement>(".group-add-action");
    const heading = container.querySelector<HTMLElement>(".new-task-group .group-heading")!;
    const newTasksToggle = heading.querySelector<HTMLButtonElement>(".group-toggle")!;
    const queueToggle = heading.querySelector<HTMLButtonElement>(".queue-toggle")!;
    const projectCount = heading.querySelector<HTMLElement>(".group-count")!;
    expect(projectCount.textContent).toBe("1 个项目");
    expect(heading.children[0]).toBe(newTasksToggle);
    expect(heading.children[1]).toBe(add);
    expect(heading.children[2]).toBe(queueToggle);
    expect(heading.children[3]).toBe(projectCount);
    expect(queueToggle.getAttribute("aria-checked")).toBe("false");
    expect(container.querySelectorAll(".draft-row")).toHaveLength(1);
    await act(async () => add?.click());
    expect(container.querySelectorAll(".draft-row")).toHaveLength(2);
    expect(projectCount.textContent).toBe("2 个项目");
    expect(container.querySelectorAll(".draft-row.selected")).toHaveLength(1);

    const remove = container.querySelectorAll<HTMLButtonElement>(".draft-row .danger-link");
    await act(async () => remove[1].click());
    await act(async () => container.querySelector<HTMLButtonElement>(".draft-row .danger-link")?.click());
    expect(container.querySelectorAll(".draft-row")).toHaveLength(1);
  });

  it("keeps auto-run session-only and uses a non-button drag indicator", async () => {
    const toggle = container.querySelector<HTMLButtonElement>(".queue-toggle")!;
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    await act(async () => toggle.click());
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(window.localStorage.getItem("ooo-splat-task-workspace-v1")).not.toContain("autoRunNext");

    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    const before = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    expect(container.querySelector(".draft-drag-handle")).toBeNull();
    expect(container.querySelector(".draft-drag-indicator")).not.toBeNull();
    expect(container.querySelector(".draft-drag-indicator")?.tagName).toBe("SPAN");
    const rows = container.querySelectorAll<HTMLElement>(".draft-row");
    await act(async () => rows[1].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", altKey: true, bubbles: true })));
    const after = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    expect(after).toEqual([before[1], before[0]]);
    await flush();
    const persisted = JSON.parse(window.localStorage.getItem("ooo-splat-task-workspace-v1") ?? "{}");
    expect(persisted.drafts.map((draft: { ordinal: number }) => draft.ordinal)).toEqual([2, 1]);
  });

  it("starts pointer reordering as soon as a held task card moves beyond the drag threshold", async () => {
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    const before = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    const rows = container.querySelectorAll<HTMLElement>(".draft-row");
    rows.forEach((row, index) => {
      let captured = false;
      row.setPointerCapture = vi.fn(() => { captured = true; });
      row.hasPointerCapture = vi.fn(() => captured);
      row.releasePointerCapture = vi.fn(() => { captured = false; });
      row.getBoundingClientRect = () => ({ x: 0, y: index * 100, left: 0, top: index * 100, right: 400, bottom: index * 100 + 100, width: 400, height: 100, toJSON: () => ({}) });
    });
    const pointer = (type: string, x: number, y: number) => {
      const event = new MouseEvent(type, { bubbles: true, button: 0, clientX: x, clientY: y });
      Object.defineProperty(event, "pointerId", { value: 7 });
      return event;
    };

    await act(async () => rows[1].dispatchEvent(pointer("pointerdown", 200, 150)));
    expect(container.querySelector(".draft-row.dragging")).toBeNull();
    await act(async () => rows[1].dispatchEvent(pointer("pointermove", 202, 152)));
    expect(container.querySelector(".draft-row.dragging")).toBeNull();
    await act(async () => rows[1].dispatchEvent(pointer("pointermove", 200, 20)));
    expect(rows[1].classList.contains("dragging")).toBe(true);
    await act(async () => rows[1].dispatchEvent(pointer("pointerup", 200, 20)));

    const after = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    expect(after).toEqual([before[1], before[0]]);
  });

  it("also starts pointer reordering after a stationary 200ms hold", async () => {
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    const before = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    const rows = container.querySelectorAll<HTMLElement>(".draft-row");
    rows.forEach((row, index) => {
      let captured = false;
      row.setPointerCapture = vi.fn(() => { captured = true; });
      row.hasPointerCapture = vi.fn(() => captured);
      row.releasePointerCapture = vi.fn(() => { captured = false; });
      row.getBoundingClientRect = () => ({ x: 0, y: index * 100, left: 0, top: index * 100, right: 400, bottom: index * 100 + 100, width: 400, height: 100, toJSON: () => ({}) });
    });
    const pointer = (type: string, x: number, y: number) => {
      const event = new MouseEvent(type, { bubbles: true, button: 0, clientX: x, clientY: y });
      Object.defineProperty(event, "pointerId", { value: 8 });
      return event;
    };

    vi.useFakeTimers();
    try {
      await act(async () => rows[1].dispatchEvent(pointer("pointerdown", 200, 150)));
      await act(async () => { vi.advanceTimersByTime(199); });
      expect(container.querySelector(".draft-row.dragging")).toBeNull();
      await act(async () => { vi.advanceTimersByTime(1); });
      expect(rows[1].classList.contains("dragging")).toBe(true);
      await act(async () => {
        rows[1].dispatchEvent(pointer("pointermove", 200, 20));
        rows[1].dispatchEvent(pointer("pointerup", 200, 20));
      });
    } finally {
      vi.useRealTimers();
    }

    const after = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    expect(after).toEqual([before[1], before[0]]);
  });

  it("accepts one native dropped video inside the material picker and reuses analysis", async () => {
    mocks.classifyDroppedInput.mockResolvedValueOnce({ inputType: "video" });
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 12, width: 1920, height: 1080, fps: 30, estimatedFrames: 360, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "drop" },
    });
    const zone = container.querySelector<HTMLElement>(".input-picker")!;
    zone.getBoundingClientRect = () => ({ x: 10, y: 10, left: 10, top: 10, right: 410, bottom: 90, width: 400, height: 80, toJSON: () => ({}) });
    const handler = mocks.onInputDragDrop.mock.calls.at(-1)?.[0] as (event: { type: "drop"; paths: string[]; x: number; y: number }) => void;

    await act(async () => handler({ type: "drop", paths: ["E:\\素材\\测试 clip.mov"], x: 20, y: 20 }));
    await flush();

    expect(mocks.classifyDroppedInput).toHaveBeenCalledWith("E:\\素材\\测试 clip.mov");
    expect(mocks.probeAndPlan).toHaveBeenCalledWith("E:\\素材\\测试 clip.mov", "balanced", true);
    expect(container.querySelector(".draft-row .project-path")?.textContent).toContain("测试 clip.mov");
  });

  it("serializes rapid starts and automatically runs only the immediate next ready draft", async () => {
    const probe = {
      inputType: "video" as const,
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low" as const, sampleCount: 0, basis: "queue" },
    };
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\first.mp4");
    mocks.selectImageSequence.mockResolvedValueOnce("E:\\Media\\second-images");
    mocks.probeAndPlan.mockImplementation(async (path: string) => path.endsWith("second-images") ? {
      ...probe,
      inputType: "images",
      video: null,
      imageSequence: { imageCount: 29, width: 1920, height: 1080, hasAlpha: false, requiresLargeSequenceConfirmation: false },
      plan: { retentionRatio: 1, samplingFps: 0, estimatedFrames: 29 },
    } : probe);
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker-toggle")?.click());
    const imageOption = [...container.querySelectorAll(".input-picker-menu button")].find((button) => button.textContent?.includes("图片"));
    await act(async () => imageOption?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click());
    await flush();

    const completedTaskIds: string[] = [];
    let activeStarts = 0;
    let maximumActiveStarts = 0;
    mocks.startPipeline.mockImplementation(async (_path, _quality, _root, _planner, workspaceTaskId: string) => {
      activeStarts += 1;
      maximumActiveStarts = Math.max(maximumActiveStarts, activeStarts);
      await Promise.resolve();
      activeStarts -= 1;
      completedTaskIds.push(workspaceTaskId);
      const suffix = completedTaskIds.length === 1 ? "111111111111" : "222222222222";
      return {
        projectId: `22222222-2222-4222-8222-${suffix}`,
        projectPath: `E:\\Projects\\${suffix}`,
        finalPly: `E:\\Projects\\${suffix}\\final.ply`,
        fileSize: 10, splatCount: 10, inputImages: 24, registeredImages: 24,
        registeredRatio: 1, points3d: 10, durationMs: 10, completedAt: "2026-10-04T00:00:00Z",
        warning: null, logsDirectory: `E:\\Projects\\${suffix}\\logs`,
      };
    });
    mocks.getProjectOverview.mockImplementation(async () => ({
      projectsRoot: "E:\\Projects",
      projects: completedTaskIds.map((workspaceTaskId, index) => ({
        ...project,
        id: index === 0 ? "22222222-2222-4222-8222-111111111111" : "22222222-2222-4222-8222-222222222222",
        name: `queued-${index}`,
        workspaceTaskId,
      })),
    }));

    await act(async () => container.querySelectorAll<HTMLElement>(".draft-row")[0].click());
    await act(async () => container.querySelector<HTMLButtonElement>(".queue-toggle")?.click());
    const start = container.querySelector<HTMLButtonElement>(".primary-action")!;
    await act(async () => {
      start.click();
      start.click();
    });
    await flush();
    await flush();
    await flush();

    expect(mocks.startPipeline).toHaveBeenCalledTimes(2);
    expect(mocks.startPipeline.mock.calls.map((call) => call[0])).toEqual(["E:\\Media\\first.mp4", "E:\\Media\\second-images"]);
    expect(mocks.confirmSmallImageSequence).not.toHaveBeenCalled();
    expect(maximumActiveStarts).toBe(1);
    expect(container.querySelector<HTMLButtonElement>(".queue-toggle")?.getAttribute("aria-checked")).toBe("false");
  });

  it("keeps another draft's media summary and planner toggle independent while a task is running", async () => {
    const probe = {
      inputType: "video" as const,
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low" as const, sampleCount: 0, basis: "draft isolation" },
    };
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\running.mp4").mockResolvedValueOnce("E:\\Media\\next.mp4");
    mocks.probeAndPlan.mockResolvedValue(probe);
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    await act(async () => container.querySelectorAll<HTMLElement>(".draft-row")[0]?.click());

    let finishRun!: (value: Awaited<ReturnType<typeof mocks.startPipeline>>) => void;
    mocks.startPipeline.mockImplementationOnce(() => new Promise((resolve) => { finishRun = resolve; }));
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await act(async () => Promise.resolve());

    await act(async () => container.querySelectorAll<HTMLElement>(".draft-row")[1]?.click());
    const planner = container.querySelector<HTMLButtonElement>(".planner-switch")!;
    expect(planner.disabled).toBe(false);
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click());
    await flush();
    expect(container.querySelector(".source-metrics")?.textContent).toContain("1920 × 1080");
    await act(async () => planner.click());
    await flush();
    expect(planner.getAttribute("aria-checked")).toBe("false");
    expect(mocks.probeAndPlan).toHaveBeenLastCalledWith("E:\\Media\\next.mp4", "balanced", false);
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    expect(container.querySelector<HTMLButtonElement>(".planner-switch")?.getAttribute("aria-checked")).toBe("true");

    await act(async () => finishRun({
      projectId: project.id,
      projectPath: project.projectPath,
      finalPly: project.finalPly!,
      fileSize: project.fileSize!,
      splatCount: project.splatCount!,
      inputImages: 24,
      registeredImages: 24,
      registeredRatio: 1,
      points3d: 10,
      durationMs: 10,
      completedAt: project.completedAt!,
      warning: null,
      logsDirectory: `${project.projectPath}\\logs`,
    }));
    await flush();
  });

  it("keeps another project's historical detail isolated from the active draft", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\running.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "detail isolation" },
    });
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click());
    await flush();

    let finishRun!: (value: Awaited<ReturnType<typeof mocks.startPipeline>>) => void;
    mocks.startPipeline.mockImplementationOnce(() => new Promise((resolve) => { finishRun = resolve; }));
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await act(async () => Promise.resolve());
    await act(async () => container.querySelector<HTMLElement>("#completed-task-group-content .project-row")?.click());
    await flush();

    expect(container.querySelector(".project-detail-stats")).not.toBeNull();
    expect(container.querySelector(".cancel-action")).toBeNull();
    expect(container.querySelector(".live-process:not(.historical)")).toBeNull();

    await act(async () => finishRun({
      projectId: project.id,
      projectPath: project.projectPath,
      finalPly: project.finalPly!,
      fileSize: project.fileSize!,
      splatCount: project.splatCount!,
      inputImages: 24,
      registeredImages: 24,
      registeredRatio: 1,
      points3d: 10,
      durationMs: 10,
      completedAt: project.completedAt!,
      warning: null,
      logsDirectory: `${project.projectPath}\\logs`,
    }));
    await flush();
  });

  it("promotes a successful task even when the project overview refresh fails", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\successful.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "refresh failure" },
    });
    const completedResult = {
      projectId: "33333333-3333-3333-3333-333333333333",
      projectPath: "E:\\Projects\\新完成项目",
      finalPly: "E:\\Projects\\新完成项目\\final.ply",
      fileSize: 12_345,
      splatCount: 678,
      inputImages: 24,
      registeredImages: 23,
      registeredRatio: 23 / 24,
      points3d: 456,
      durationMs: 10,
      completedAt: "2026-10-06T01:00:00Z",
      warning: null,
      logsDirectory: "E:\\Projects\\新完成项目\\logs",
    };
    mocks.startPipeline.mockResolvedValueOnce(completedResult);
    mocks.getProjectOverview.mockRejectedValueOnce(new Error("project-index busy"));

    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await flush();

    expect(useAppStore.getState().phase).toBe("completed");
    expect(useAppStore.getState().projects.some((item) => item.id === completedResult.projectId)).toBe(true);
    expect(container.querySelector(".pane-header h1")?.textContent).toBe("新完成项目");
    expect(container.querySelector(".project-detail-page .inline-error")?.textContent).toContain("任务已完成，但刷新项目列表失败");
    expect(container.querySelector(".project-detail-page .inline-error")?.textContent).toContain("project-index busy");
    expect(mocks.cancelPipeline).not.toHaveBeenCalled();
  });

  it("ignores a late historical detail response after the active task completes", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\running-with-history.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "late history" },
    });
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click());
    await flush();

    let finishRun!: (value: unknown) => void;
    mocks.startPipeline.mockImplementationOnce(() => new Promise((resolve) => { finishRun = resolve; }));
    let finishDetail!: (value: unknown) => void;
    mocks.getProjectTaskDetail.mockImplementationOnce(() => new Promise((resolve) => { finishDetail = resolve; }));
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await act(async () => Promise.resolve());
    await act(async () => container.querySelector<HTMLElement>("#completed-task-group-content .project-row")?.click());
    expect(container.querySelector(".task-detail-loading")).not.toBeNull();

    const completedResult = {
      projectId: "44444444-4444-4444-4444-444444444444",
      projectPath: "E:\\Projects\\并发完成项目",
      finalPly: "E:\\Projects\\并发完成项目\\final.ply",
      fileSize: 42_000,
      splatCount: 1_234,
      inputImages: 24,
      registeredImages: 24,
      registeredRatio: 1,
      points3d: 789,
      durationMs: 20,
      completedAt: "2026-10-06T01:30:00Z",
      warning: null,
      logsDirectory: "E:\\Projects\\并发完成项目\\logs",
    };
    await act(async () => finishRun(completedResult));
    await flush();
    expect(container.querySelector(".pane-header h1")?.textContent).toBe("并发完成项目");

    await act(async () => finishDetail({
      project,
      inputType: "video",
      sourcePath: "E:\\Capture\\input.mp4",
      projectsRoot: "E:\\Projects",
      plannerEnabled: true,
      estimatedFrames: 320,
      stage: "completed",
      progress: 100,
      inputImages: 100,
      registeredImages: 90,
      video: null,
      imageSequence: null,
      sourceProjectId: null,
      logs: [],
    }));
    await flush();

    expect(container.querySelector(".pane-header h1")?.textContent).toBe("并发完成项目");
    expect(useAppStore.getState().phase).toBe("completed");
    expect(mocks.cancelPipeline).not.toHaveBeenCalled();
  });

  it("collapses task groups, persists the preference, and expands new tasks when adding one", async () => {
    const newTasksToggle = container.querySelector<HTMLButtonElement>('[aria-controls="new-task-group-content"]')!;
    await act(async () => newTasksToggle.click());

    expect(newTasksToggle.getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector<HTMLElement>("#new-task-group-content")?.hidden).toBe(true);
    expect(JSON.parse(window.localStorage.getItem("ooo-splat-task-groups-v1") ?? "{}").new).toBe(false);

    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    expect(newTasksToggle.getAttribute("aria-expanded")).toBe("true");
    expect(container.querySelector<HTMLElement>("#new-task-group-content")?.hidden).toBe(false);
  });

  it("offers a reshoot entry on a completed project and creates an incremental reshoot task", async () => {
    // Selected by class rather than by label: the row is translated, so asserting
    // on text would couple this test to the active interface language.
    const reshootButton = container.querySelector<HTMLButtonElement>(".reshoot-link");
    expect(reshootButton).not.toBeNull();

    await act(async () => { reshootButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(mocks.inspectReshootSource).toHaveBeenCalledWith(project.id);
    expect(mocks.prepareGaussianPreview).not.toHaveBeenCalled();
    expect(container.querySelector(".reshoot-workflow-dialog")).toBeNull();
    expect(container.querySelector(".reshoot-task-page")).not.toBeNull();
    expect(container.querySelector(".reshoot-source-summary")).toBeNull();
    expect(container.querySelectorAll(".reshoot-task-page .quality-option:disabled")).toHaveLength(3);
    expect(container.querySelector(".reshoot-task-page .locked-setting")?.textContent).toContain("继承源任务 · 不可修改");
    expect(container.querySelector(".reshoot-task-page .locked-quality-settings .quality-option.selected")).not.toBeNull();
    expect(container.querySelector<HTMLButtonElement>(".reshoot-task-page .planner-switch")?.disabled).toBe(true);
    expect(container.querySelector<HTMLButtonElement>(".reshoot-task-page .planner-switch")?.getAttribute("aria-checked")).toBe("true");
    expect(container.querySelector(".reshoot-task-page .planner-switch em")).toBeNull();
  });

  it("shows automatic optimization as locked off when the source task disabled it", async () => {
    mocks.inspectReshootSource.mockResolvedValueOnce({
      projectId: project.id,
      projectName: project.name,
      quality: project.quality,
      plannerEnabled: false,
      cameraId: 1,
      cameraModel: "SIMPLE_RADIAL",
      width: 1920,
      height: 1080,
      sourceImageCount: 100,
      eligible: true,
      reason: null,
    });

    await act(async () => container.querySelector<HTMLButtonElement>(".reshoot-link")?.click());
    await flush();

    const planner = container.querySelector<HTMLButtonElement>(".reshoot-task-page .planner-switch");
    expect(planner?.disabled).toBe(true);
    expect(planner?.getAttribute("aria-checked")).toBe("false");
    expect(planner?.textContent).not.toContain("继承源任务");
  });

  it("probes Alpha reshoot media and blocks a mismatched camera resolution", async () => {
    mocks.selectVideo.mockResolvedValue("E:\\Capture\\reshoot.mov");
    mocks.probeReshootInput.mockResolvedValue({
      inputType: "video",
      imageCount: null,
      duration: 12,
      preparedWidth: 1080,
      preparedHeight: 1920,
      estimatedFrames: 180,
      hasAlpha: true,
      maskCount: 180,
      compatible: false,
      incompatibilityReason: "补拍画面必须与原项目保持相同分辨率",
      estimate: { estimatedMs: 120_000, lowerBoundMs: 90_000, upperBoundMs: 180_000, confidence: "low", sampleCount: 0, basis: "素材规模" },
    });
    const reshootButton = container.querySelector<HTMLButtonElement>(".reshoot-link");
    await act(async () => { reshootButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();
    const videoButton = container.querySelector<HTMLButtonElement>(".reshoot-task-page .input-picker > .path-picker");
    await act(async () => { videoButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(mocks.probeReshootInput).toHaveBeenCalledWith(project.id, "E:\\Capture\\reshoot.mov", "video");
    expect(container.querySelector(".reshoot-plan.incompatible")?.textContent).toContain("1080 × 1920");
    expect(container.querySelector<HTMLButtonElement>(".reshoot-task-page .primary-action")?.disabled).toBe(true);
  });

  it("keeps an ineligible reshoot source disabled without rendering zero camera metrics", async () => {
    mocks.inspectReshootSource.mockResolvedValue({
      projectId: project.id,
      projectName: project.name,
      quality: project.quality,
      plannerEnabled: false,
      cameraId: 0,
      cameraModel: "",
      width: 0,
      height: 0,
      sourceImageCount: 0,
      eligible: false,
      reason: "The source reconstruction is unavailable",
    });

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".reshoot-link")?.click();
    });
    await flush();

    expect(container.querySelector(".reshoot-source-summary")).toBeNull();
    expect(container.querySelector(".reshoot-task-page .inline-error")).not.toBeNull();
    expect(
      Array.from(container.querySelectorAll<HTMLButtonElement>(".reshoot-task-page .input-picker button"))
        .every((button) => button.disabled),
    ).toBe(true);
  });

  it("shows the original-media rejection inside the reshoot task", async () => {
    mocks.selectVideo.mockResolvedValue("E:\\Capture\\original.mov");
    mocks.probeReshootInput.mockRejectedValue(
      new Error("补拍时不能再次使用原素材，请选择新拍摄的视频或图片序列。"),
    );
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".reshoot-link")?.click();
    });
    await flush();
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".reshoot-task-page .input-picker > .path-picker")?.click();
    });
    await flush();

    expect(container.querySelector(".reshoot-task-page .inline-error")?.textContent).toContain(
      "补拍时不能再次使用原素材",
    );
    expect(container.querySelector<HTMLButtonElement>(".reshoot-task-page .primary-action")?.disabled).toBe(true);
  });

  it("opens a plain preview without the reshoot workflow", async () => {
    const previewButton = container.querySelector<HTMLButtonElement>(".preview-link");

    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.querySelector(".preview-workspace")).not.toBeNull();
  });

  it("keeps the reshoot entry away from unfinished projects", async () => {
    const unfinished = { ...project, status: "cancelled" as const, finalPly: null, completedAt: null };
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });

    expect(container.querySelector(".reshoot-link")).toBeNull();
    expect(container.querySelector(".resume-link")).not.toBeNull();
  });

  it("switches the complete task workspace to English without reloading", async () => {
    const languageButton = container.querySelector<HTMLButtonElement>(".language-action");
    expect(languageButton?.textContent).toContain("EN");
    expect(languageButton?.title).toBe("中英文切换 / Switch language");

    await act(async () => languageButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    expect(container.textContent).toContain("Create New Task");
    expect(container.textContent).toContain("Task List");
    expect(container.textContent).toContain("Start Generation");
    expect(container.textContent).toContain("Checking bundled engines");
    expect(languageButton?.textContent).toContain("中文");
    expect(languageButton?.title).toBe("中英文切换 / Switch language");
    expect(window.localStorage.getItem("ooo-splat-language")).toBe("en");
  });

  it("shows automatic mask extraction when the selected video has alpha", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\alpha.mov");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
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
      imageSequence: null,
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
      estimate: { estimatedMs: 120_000, lowerBoundMs: 60_000, upperBoundMs: 180_000, confidence: "low", sampleCount: 0, basis: "video" },
    });
    await act(async () => { container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click(); });
    await flush();

    expect(container.textContent).toContain("检测到 Alpha 通道");
    expect(container.textContent).toContain("将自动提取透明画面和 COLMAP Mask");

    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\opaque.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
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
      imageSequence: null,
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
      estimate: { estimatedMs: 120_000, lowerBoundMs: 60_000, upperBoundMs: 180_000, confidence: "low", sampleCount: 0, basis: "video" },
    });
    await act(async () => { container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click(); });
    await flush();
    expect(container.textContent).not.toContain("将自动提取透明画面和 COLMAP Mask");
  });

  it("hides the previous live process after selecting new media but keeps new analysis notices", async () => {
    mocks.getAppRuntimeStatus.mockResolvedValueOnce({ pipelineRunning: true, pipelineProjectId: null, pipelineWorkspaceTaskId: null, previewProjectId: null, taskAcceleration: null });
    act(() => window.dispatchEvent(new Event("focus")));
    await flush();
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
    mocks.getAppRuntimeStatus.mockResolvedValueOnce({ pipelineRunning: false, pipelineProjectId: null, pipelineWorkspaceTaskId: null, previewProjectId: null, taskAcceleration: null });
    act(() => window.dispatchEvent(new Event("focus")));
    await flush();
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

  it("keeps a resumed project in the completed list when the first catalog refresh misses it", async () => {
    const unfinished = { ...project, status: "failed" as const, finalPly: null, completedAt: null };
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });
    mocks.getProjectOverview.mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [] });

    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续任务");
    await act(async () => { resumeButton?.click(); });
    await flush();

    const completed = useAppStore.getState().projects.find((item) => item.id === project.id);
    expect(completed).toMatchObject({ status: "completed", finalPly: project.finalPly });
    expect(container.querySelector(`#completed-task-group-content .project-row`)).not.toBeNull();
    expect(container.querySelector(`#unfinished-task-group-content .project-row`)).toBeNull();
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

  it("offers voluntary reporting for a generic failure even with analytics off", async () => {
    mocks.initializeTelemetry.mockResolvedValue({ analyticsEnabled: false, consentDecided: true, deliveryStatus: "configured" });
    await act(async () => { root.render(<LanguageProvider><App key="analytics-disabled" /></LanguageProvider>); }); await flush();
    mocks.resumePipeline.mockRejectedValueOnce({ code: "pipeline_failed", message: "engine failed", failedStage: "matching", engine: "colmap", projectId: project.id, failureId: "failure-token" });
    await act(async () => { useAppStore.setState({ projects: [{ ...project, status: "failed", finalPly: null }] }); });
    await act(async () => { [...container.querySelectorAll("button")].find(button => button.textContent === "继续任务")?.click(); }); await flush();
    const report = [...container.querySelectorAll("button")].find(button => button.textContent === "发送错误报告");
    expect(report?.disabled).toBe(false); expect(mocks.sendErrorReport).not.toHaveBeenCalled();
    expect(container.querySelector(".failure-guidance-dialog h2")?.textContent).toBe("任务未能完成");
    expect(mocks.setTelemetryConsent).not.toHaveBeenCalled();
  });

  it("blocks the interface while a slow cancellation is still terminating processes", async () => {
    mocks.getAppRuntimeStatus.mockResolvedValueOnce({ pipelineRunning: true, pipelineProjectId: null, pipelineWorkspaceTaskId: null, previewProjectId: null, taskAcceleration: null });
    act(() => useAppStore.setState({ phase: "running" }));
    act(() => window.dispatchEvent(new Event("focus")));
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
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\estimate.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
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
      imageSequence: null,
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
      estimate: {
        estimatedMs: 120_000,
        lowerBoundMs: 60_000,
        upperBoundMs: 180_000,
        confidence: "medium",
        sampleCount: 3,
        basis: "本机历史任务校准",
      },
    });
    await act(async () => { container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click(); });
    await flush();

    expect(container.textContent).toContain("预计时长");
    expect(container.textContent).toContain("约 2 分 0 秒");
    expect(container.querySelector('[title="本机历史任务校准"]')).not.toBeNull();
  });

  it("does not turn a Brush heartbeat into completed training steps", async () => {
    mocks.getAppRuntimeStatus.mockResolvedValueOnce({ pipelineRunning: true, pipelineProjectId: null, pipelineWorkspaceTaskId: null, previewProjectId: null, taskAcceleration: null });
    act(() => useAppStore.setState({
      phase: "running",
      progress: 79,
      progressMessage: "Brush 训练进程仍在运行",
      latestEvent: {
        sequence: 7,
        timestamp: new Date().toISOString(),
        kind: "heartbeat",
        level: "info",
        stage: "trainingSplats",
        engine: "brush",
        progress: 79,
        stageProgress: 50,
        indeterminate: false,
        message: "Brush 训练进程仍在运行",
        current: null,
        total: 15_000,
        unit: null,
        elapsedMs: 120_000,
        acceleration: null,
      },
    }));
    act(() => window.dispatchEvent(new Event("focus")));
    await flush();

    expect(container.querySelector(".current-message")?.textContent).toBe("正在生成高斯泼溅…。此步骤可能耗时较长，请耐心等待");
    expect(container.textContent).not.toContain("7,500/15,000");
  });

  it("shows only the task panes until a completed project is opened", async () => {
    expect(container.textContent).toContain("创建新任务");
    expect(container.textContent).toContain("任务列表");
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
    expect(container.textContent).not.toContain("创建新任务");

    const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
    await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain(project.name);
    expect(container.textContent).toContain("任务列表");
    expect(container.querySelector<HTMLElement>(".control-pane")?.scrollTop).toBe(48);
    expect(container.querySelector<HTMLElement>(".projects-pane")?.scrollTop).toBe(96);
    expect(mocks.releaseGaussianPreview).toHaveBeenCalledWith(project.id);
  });

  it("opens another completed project while a generation task keeps running", async () => {
    await publishTask(sharedTask({
      task_id: "running-task",
      project_id: null,
      project_path: null,
      status: "running",
      stage: "trainingSplats",
      estimated_progress: 45,
      result: null,
    }));
    const runtimeReads = mocks.getAppRuntimeStatus.mock.calls.length;
    mocks.getAppRuntimeStatus.mockImplementation(() => new Promise(() => undefined));

    const previewButton = container.querySelector<HTMLButtonElement>("#completed-task-group-content .preview-link");
    expect(previewButton?.disabled).toBe(false);
    await act(async () => previewButton?.click());
    await flush();

    expect(mocks.prepareGaussianPreview).toHaveBeenCalledWith(project.id);
    expect(container.querySelector(".preview-workspace")).not.toBeNull();
    expect(mocks.getAppRuntimeStatus).toHaveBeenCalledTimes(runtimeReads);
    expect(useAppStore.getState().phase).toBe("running");
  });

  it("keeps the task workspace visible when preview preparation fails", async () => {
    mocks.prepareGaussianPreview.mockRejectedValueOnce(new Error("PLY 无法读取"));
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain(project.name);
    expect(container.textContent).toContain("任务列表");
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

    expect(container.textContent).toContain(project.name);
    expect(container.textContent).toContain("任务列表");
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
    expect(container.textContent).toContain(project.name);
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

    expect(container.textContent).toContain(project.name);
    expect(container.textContent).toContain("任务列表");
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
      expect(container.textContent).toContain(project.name);
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
    mocks.selectImageSequence.mockResolvedValueOnce("E:\\Photos\\large");
    mocks.probeAndPlan.mockResolvedValueOnce({
        inputType: "images",
        video: null,
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
    mocks.confirmLargeImageSequence.mockResolvedValueOnce(false);
    await act(async () => { container.querySelector<HTMLButtonElement>('[aria-label="选择输入素材类型"]')?.click(); });
    const imageOption = [...container.querySelectorAll(".input-picker-menu button")].find((button) => button.textContent?.includes("图片"));
    await act(async () => { imageOption?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await act(async () => { container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click(); });
    await flush();

    const generate = [...container.querySelectorAll("button")].find((button) => button.textContent?.includes("开始生成"));
    await act(async () => generate?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await flush();

    expect(mocks.confirmLargeImageSequence).toHaveBeenCalledWith(501);
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it("warns for fewer than 30 images, supports cancellation, and stops when the dialog fails", async () => {
    mocks.selectImageSequence.mockResolvedValueOnce("E:\\Photos\\small");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "images",
      video: null,
      imageSequence: { imageCount: 29, width: 1920, height: 1080, hasAlpha: false, requiresLargeSequenceConfirmation: false },
      plan: { retentionRatio: 1, samplingFps: 0, estimatedFrames: 29 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "test" },
    });
    mocks.confirmSmallImageSequence
      .mockResolvedValueOnce(false)
      .mockRejectedValueOnce(new Error("dialog failed"))
      .mockResolvedValueOnce(true);
    mocks.startPipeline.mockResolvedValueOnce({
      projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly,
      fileSize: project.fileSize, splatCount: project.splatCount, inputImages: 29,
      registeredImages: 29, registeredRatio: 1, points3d: 10, durationMs: 10,
      completedAt: project.completedAt, warning: null, logsDirectory: `${project.projectPath}\\logs`,
    });

    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker-toggle")?.click());
    const imageOption = [...container.querySelectorAll(".input-picker-menu button")].find((button) => button.textContent?.includes("图片"));
    await act(async () => imageOption?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click());
    await flush();

    await act(async () => container.querySelector<HTMLButtonElement>(".queue-toggle")?.click());
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await flush();
    expect(mocks.confirmSmallImageSequence).toHaveBeenLastCalledWith(29);
    expect(mocks.startPipeline).not.toHaveBeenCalled();
    expect(container.querySelector<HTMLButtonElement>(".queue-toggle")?.getAttribute("aria-checked")).toBe("false");

    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await flush();
    expect(container.querySelector(".inline-error")?.textContent).toContain("dialog failed");
    expect(mocks.startPipeline).not.toHaveBeenCalled();

    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await flush();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
  });

  it("starts an image task with exactly 30 images without the small-sequence warning", async () => {
    mocks.selectImageSequence.mockResolvedValueOnce("E:\\Photos\\thirty");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "images",
      video: null,
      imageSequence: { imageCount: 30, width: 1920, height: 1080, hasAlpha: false, requiresLargeSequenceConfirmation: false },
      plan: { retentionRatio: 1, samplingFps: 0, estimatedFrames: 30 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "test" },
    });
    mocks.startPipeline.mockRejectedValueOnce(new Error("stop after launch"));
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker-toggle")?.click());
    const imageOption = [...container.querySelectorAll(".input-picker-menu button")].find((button) => button.textContent?.includes("图片"));
    await act(async () => imageOption?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await act(async () => container.querySelector<HTMLButtonElement>(".input-picker > .path-picker")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await flush();
    expect(mocks.confirmSmallImageSequence).not.toHaveBeenCalled();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
  });

  it("shows completed-project statistics without a duplicate detail action bar", async () => {
    mocks.getProjectTaskDetail.mockResolvedValueOnce({
      project,
      inputType: "video",
      sourcePath: "E:\\Capture\\input.mp4",
      projectsRoot: "E:\\Projects",
      plannerEnabled: true,
      estimatedFrames: 90,
      stage: "completed",
      progress: 100,
      inputImages: 90,
      registeredImages: 90,
      video: { duration: 3, width: 2560, height: 3840, fps: 30, totalFrames: 90, codec: "h264", rotation: 0, hasAlpha: true, pixelFormat: "rgba" },
      imageSequence: null,
      sourceProjectId: null,
      logs: [],
    });
    await act(async () => useAppStore.getState().setColmapAcceleration({
      backend: "gpu",
      detectionState: "ready",
      reasonCode: "gpuReady",
      reason: "NVIDIA GeForce RTX 3060 Ti",
      device: { index: 0, name: "NVIDIA GeForce RTX 3060 Ti", driverVersion: "616.92", computeCapability: "8.6", totalMemoryMb: 8192 },
      requirements: { minimumDriverVersion: "580.00", minimumComputeCapability: "7.5" },
      detectedNvidiaDeviceCount: 1,
    }));
    await act(async () => { container.querySelector<HTMLElement>(".project-group:not(.new-task-group):not(.unfinished) .project-row")?.click(); });
    await flush();

    const detail = container.querySelector(".project-detail-page");
    expect(detail).not.toBeNull();
    expect(detail?.querySelector('.result-actions')).toBeNull();
    expect(detail?.querySelector(":scope > .project-path")).toBeNull();
    expect(detail?.querySelector(".project-configuration-detail")).not.toBeNull();
    expect(detail?.querySelector(".path-picker.readonly")?.textContent).toContain("input.mp4");
    expect(detail?.querySelector(".locked-quality-settings [aria-checked=true]")?.textContent).toContain("均衡");
    expect(detail?.querySelector<HTMLButtonElement>(".planner-switch")?.disabled).toBe(true);
    expect(detail?.querySelector(".locked-setting")).toBeNull();
    expect(detail?.textContent).toContain("COLMAP GPU 加速开启");
    expect(detail?.textContent).not.toContain("COLMAP GPU 加速已开启");
    expect(detail?.textContent).toContain("已自动提取透明画面和 COLMAP Mask · rgba");
    expect(detail?.querySelectorAll(".project-source-metrics > span")).toHaveLength(2);
    expect(detail?.querySelector(".project-source-metrics")?.textContent).not.toContain("预计帧数");
    expect(detail?.querySelector(".primary-action")).toBeNull();
    expect(container.querySelector(".project-detail-actions")).toBeNull();
    expect(container.querySelector("#completed-task-group-content .preview-link")).not.toBeNull();
    expect(container.querySelector("#completed-task-group-content .reshoot-link")).not.toBeNull();
    expect(container.querySelectorAll("#completed-task-group-content .project-actions button")).toHaveLength(4);
  });

  it("removes detail actions for unfinished projects while retaining their card actions", async () => {
    const failed = { ...project, id: "22222222-2222-2222-2222-222222222222", status: "failed" as const, finalPly: null };
    await act(async () => useAppStore.getState().setProjects([failed]));
    await flush();
    await act(async () => container.querySelector<HTMLElement>("#unfinished-task-group-content .project-row")?.click());
    await flush();

    expect(container.querySelector(".project-detail-actions")).toBeNull();
    expect(container.querySelector(".unfinished-task-detail")).not.toBeNull();
    expect(container.querySelector(".locked-quality-settings [aria-checked=true]")?.textContent).toContain("均衡");
    expect(container.querySelector<HTMLButtonElement>(".unfinished-task-detail .planner-switch")?.disabled).toBe(true);
    expect(container.querySelector(".unfinished-task-detail .primary-action")?.textContent).toContain("继续生成");
    expect(container.querySelector("#unfinished-task-group-content .resume-link")).not.toBeNull();
    expect(container.querySelectorAll("#unfinished-task-group-content .project-actions button")).toHaveLength(3);
  });

  it("keeps a task-detail load error scoped to that project", async () => {
    mocks.getProjectTaskDetail.mockRejectedValueOnce(new Error("找不到所需文件"));
    await act(async () => container.querySelector<HTMLElement>("#completed-task-group-content .project-row")?.click());
    await flush();

    expect(container.querySelector(".project-detail-page .inline-error")?.textContent).toContain("找不到所需文件");
    expect(useAppStore.getState().error).toBeNull();

    await act(async () => container.querySelector<HTMLElement>("#new-task-group-content .draft-row")?.click());
    expect(container.querySelector(".inline-error")).toBeNull();
  });

  it("shows only the latest unfinished-project error without a close action", async () => {
    const persisted = { ...project, status: "failed" as const, finalPly: null, failureMessage: "older persisted failure" };
    await act(async () => useAppStore.getState().setProjects([persisted]));
    mocks.revealProject.mockRejectedValueOnce(new Error("newest action failure"));
    await flush();
    await act(async () => container.querySelector<HTMLElement>("#unfinished-task-group-content .project-row")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>("#unfinished-task-group-content .project-actions button:not(.resume-link):not(.danger-link)")?.click());
    await flush();

    const errors = container.querySelectorAll(".project-detail-page .inline-error");
    expect(errors).toHaveLength(1);
    expect(errors[0].textContent).toContain("newest action failure");
    expect(errors[0].textContent).not.toContain("older persisted failure");
    expect(errors[0].querySelector("button")).toBeNull();
  });

  it("restores the low-registration warning from historical project data", async () => {
    await act(async () => {
      useAppStore.getState().setProjects([{ ...project, registeredRatio: 0.62 }]);
    });
    await flush();
    expect(container.querySelector(".project-quality-warning")?.textContent).toContain("62.0%");
  });

  it("summarizes long errors in both task panes and reveals their full detail on hover", async () => {
    const detail = "Brush exited with code 1\n" + "Detailed engine output\n".repeat(100);
    await act(async () => {
      useAppStore.setState({ projects: [{ ...project, status: "failed", finalPly: null, failureMessage: detail }] });
      useAppStore.getState().setError(detail);
    });
    for (const selector of [".inline-error .compact-error", ".project-failure .compact-error"]) {
      const error = container.querySelector<HTMLElement>(selector)!;
      expect(error.textContent).toBe("模型训练失败");
      expect(container.textContent).not.toContain("Detailed engine output");
      await act(async () => error.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })));
      expect(document.querySelector(".error-detail-tooltip[role=tooltip]")?.textContent).toBe(detail);
      await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
    }
    await act(async () => container.querySelector<HTMLButtonElement>(".inline-error button")!.click());
    expect(container.querySelector(".inline-error")).toBeNull();
    expect(container.querySelector(".project-failure")).not.toBeNull();
  });
  const sharedTask = (overrides: Partial<import("../types/tasks").SharedTask> = {}): import("../types/tasks").SharedTask => ({
    task_id: "shared-1", run_id: "run-1", project_id: project.id, project_path: project.projectPath,
    input_path: "E:\\Media\\orbit.mov", input_type: "video", quality: "balanced", source: "mcp", task_kind: "generation",
    planner_enabled: false, projects_root: "E:\\Projects", status: "completed", stage: "completed",
    revision: 1, sequence: 0, created_at: project.createdAt, updated_at: project.completedAt!, elapsed_ms: 60000,
    progress: 100, estimated_progress: 100, current: null, total: null, eta_seconds: null,
    error: null, result: null, recent_events: [], ...overrides,
  });
  const publishTask = async (task: import("../types/tasks").SharedTask) => {
    const handler = mocks.onTaskUpdate.mock.calls.at(-1)![0];
    await act(async () => handler({ task, event: null }));
    await flush();
  };

  it("merges a shared task and its old project into one original project row with direct actions", async () => {
    await publishTask(sharedTask());
    const rows = container.querySelectorAll(".project-row:not(.draft-row)");
    expect(rows).toHaveLength(1);
    const row = rows[0];
    expect(row.querySelector(".project-stats")?.textContent).toContain("PLY");
    expect(row.querySelector(".project-path")?.textContent).toBe(project.projectPath);
    expect(row.querySelector(".task-origin")?.textContent).toContain("由 AI Agent 创建");
    expect(row.querySelector(".preview-link")).not.toBeNull();
    expect(row.querySelector(".reshoot-link")).not.toBeNull();
    expect(container.textContent).not.toContain("项目操作");
    expect(container.querySelector(".shared-task-detail")).toBeNull();
    await act(async () => row.querySelector<HTMLButtonElement>(".preview-link")!.click());
    expect(mocks.prepareGaussianPreview).toHaveBeenCalledWith(project.id);
  });

  it("updates completion and project metadata without switching the selected task or starting GUI drafts", async () => {
    await publishTask(sharedTask({ status: "running", stage: "trainingSplats", estimated_progress: 65 }));
    const row = container.querySelector<HTMLElement>('[data-task-id="shared-1"]')!;
    await act(async () => row.click());
    expect(container.querySelector(".shared-task-detail .stage-timeline .active")).not.toBeNull();
    expect(container.querySelector(".pane-header .run-state.active")).not.toBeNull();
    const reads = mocks.getProjectOverview.mock.calls.length;
    await publishTask(sharedTask({ revision: 2 }));
    expect(mocks.getProjectOverview.mock.calls.length).toBeGreaterThan(reads);
    expect(container.querySelector(".shared-task-detail .project-detail-stats")).not.toBeNull();
    expect(container.querySelectorAll(".shared-task-detail .stage-timeline .done")).toHaveLength(7);
    expect(container.querySelector(".shared-task-detail .cancel-action")).toBeNull();
    expect(container.querySelector('#completed-task-group-content [data-task-id="shared-1"].selected')).not.toBeNull();
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it("uses the interrupted task state over a stale running project after reopening", async () => {
    mocks.getProjectOverview.mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [{ ...project, status: "running" }] });
    await publishTask(sharedTask({ status: "interrupted", stage: "trainingSplats", estimated_progress: 65 }));
    await act(async () => container.querySelector<HTMLElement>('[data-task-id="shared-1"]')!.click());
    expect(container.querySelector(".pane-header .run-state")?.textContent).toBe("已中断");
    expect(container.querySelector('.unfinished [data-task-id="shared-1"] .status-copy')?.textContent).toBe("已中断");
    expect(container.querySelector(".shared-task-detail .stage-timeline .interrupted")).not.toBeNull();
    expect(container.querySelector(".shared-task-detail .cancel-action")).toBeNull();
  });

  it('keeps the original new-task layout and disabled grip after submission without enabling sorting or deletion', async () => {
    await publishTask(sharedTask({ status: 'running', stage: 'trainingSplats', estimated_progress: 65 }));
    const row = container.querySelector<HTMLElement>('[data-task-id="shared-1"]')!;
    expect(row.classList.contains('draft-row')).toBe(true);
    expect(row.classList.contains('drag-disabled')).toBe(true);
    expect(row.querySelector('.draft-drag-indicator svg')).not.toBeNull();
    expect(row.querySelector('.project-title-line strong')?.textContent).toBe('orbit.mov');
    expect(row.querySelector('.project-path')?.textContent).toBe('E:\\Media\\orbit.mov');
    expect(row.querySelector('.project-stats')).toBeNull();
    expect(row.querySelector<HTMLButtonElement>('.danger-link')?.disabled).toBe(true);
    expect(row.hasAttribute('data-draft-id')).toBe(false);
    await act(async () => { row.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true })); });
    expect(container.querySelector('[data-task-id="shared-1"]')).toBe(row);
    await act(async () => row.click());
    expect(container.querySelector('.shared-task-detail .locked-setting')).toBeNull();
    expect(container.querySelector<HTMLButtonElement>('.shared-task-detail .planner-switch')?.disabled).toBe(true);
  });

  it('reorders GUI drafts while ignoring a submitted new-task row in pointer hit testing', async () => {
    await act(async () => container.querySelector<HTMLButtonElement>('.group-add-action')!.click());
    await publishTask(sharedTask({ status: 'created', run_id: null, project_id: null, project_path: null, stage: null }));
    const submitted = container.querySelector<HTMLElement>('[data-task-id="shared-1"]')!;
    const drafts = [...container.querySelectorAll<HTMLElement>('.new-task-group [data-draft-id]')];
    expect(drafts).toHaveLength(2);
    const ids = drafts.map(row => row.dataset.draftId);
    submitted.getBoundingClientRect = () => ({ top: 0, height: 100 } as DOMRect);
    drafts.forEach((row, index) => { row.getBoundingClientRect = () => ({ top: 200 + index * 100, height: 100 } as DOMRect); });
    const held = drafts[0]; let captured = false;
    held.setPointerCapture = () => { captured = true; }; held.hasPointerCapture = () => captured; held.releasePointerCapture = () => { captured = false; };
    const pointer = (type: string, y: number) => {
      const event = new MouseEvent(type, { bubbles: true, button: 0, clientX: 100, clientY: y });
      Object.defineProperty(event, 'pointerId', { value: 9 }); return event;
    };
    await act(async () => {
      held.dispatchEvent(pointer('pointerdown', 220));
      held.dispatchEvent(pointer('pointermove', 450));
      held.dispatchEvent(pointer('pointerup', 450));
    });
    expect([...container.querySelectorAll<HTMLElement>('.new-task-group [data-draft-id]')].map(row => row.dataset.draftId)).toEqual([ids[1], ids[0]]);
    expect(submitted.classList.contains('drag-disabled')).toBe(true);
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it('retains the source-project name and reshoot label after a GUI reshoot is submitted', async () => {
    await publishTask(sharedTask({ status: 'running', stage: 'trainingSplats', source: 'gui', task_kind: 'reshoot', source_project_id: project.id, project_id: null, project_path: null }));
    expect(container.querySelector('[data-task-id="shared-1"] .project-title-line strong')?.textContent).toBe(`${project.name} · 补拍`);
    expect(container.querySelector('[data-task-id="shared-1"] .draft-drag-indicator')).not.toBeNull();
  });

});
