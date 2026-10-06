// @vitest-environment jsdom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider } from "../i18n";
import { useAppStore } from "../stores/appStore";
import type { PipelineEvent } from "../types/pipeline";

const mocks = vi.hoisted(() => ({
  initializeTelemetry: vi.fn(),
  getProjectOverview: vi.fn(),
  getSharedTasks: vi.fn(async () => []),
  onTaskUpdate: vi.fn(async () => () => undefined),
  getAppRuntimeStatus: vi.fn(),
}));

vi.mock("../lib/backend", () => ({
  cancelPipeline: vi.fn(),
  checkEngines: vi.fn().mockResolvedValue([]),
  checkColmapAcceleration: vi.fn().mockResolvedValue(null),
  confirmAndDeleteProject: vi.fn().mockResolvedValue(false),
  confirmLargeImageSequence: vi.fn().mockResolvedValue(true),
  confirmSmallImageSequence: vi.fn().mockResolvedValue(true),
  estimateProjectRuntime: vi.fn(),
  exportPly: vi.fn(),
  getSharedTasks: mocks.getSharedTasks,
  onTaskUpdate: mocks.onTaskUpdate,
  getAppRuntimeStatus: mocks.getAppRuntimeStatus,
  getProjectOverview: mocks.getProjectOverview,
  initializeTelemetry: mocks.initializeTelemetry,
  classifyDroppedInput: vi.fn(),
  onInputDragDrop: vi.fn().mockResolvedValue(() => undefined),
  onPipelineEvent: vi.fn().mockResolvedValue(() => undefined),
  prepareGaussianPreview: vi.fn(),
  probeAndPlan: vi.fn(),
  releaseGaussianPreview: vi.fn(),
  resumePipeline: vi.fn(),
  revealProject: vi.fn(),
  revealProjectLogs: vi.fn(),
  revealFile: vi.fn(),
  selectImageSequence: vi.fn().mockResolvedValue(null),
  selectProjectsRoot: vi.fn(),
  selectVideo: vi.fn().mockResolvedValue(null),
  setProjectsRoot: vi.fn(),
  setTelemetryConsent: vi.fn(),
  startPipeline: vi.fn(),
}));

vi.mock("../components/GaussianViewer", () => ({
  GaussianViewer: () => <section className="preview-workspace" />,
}));

import { App } from "./App";

const event = (sequence: number): PipelineEvent => ({
  sequence,
  timestamp: "2026-08-22T14:00:00Z",
  kind: "progress",
  level: "info",
  stage: "trainingSplats",
  engine: "brush",
  progress: 50,
  stageProgress: 50,
  indeterminate: false,
  message: `step ${sequence}`,
  current: sequence,
  total: 30_000,
  unit: "步",
  elapsedMs: sequence * 10,
  acceleration: null,
});

const flush = async () => {
  await act(async () => {
    await Promise.resolve();
    await new Promise((resolve) => window.setTimeout(resolve, 0));
  });
};

const pushEvents = async (from: number, to: number) => {
  await act(async () => {
    for (let sequence = from; sequence <= to; sequence += 1) {
      useAppStore.getState().receiveEvent(event(sequence));
    }
  });
  await flush();
};

describe("App live log", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(async () => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    window.localStorage.setItem("ooo-splat-language", "zh-CN");
    useAppStore.setState({
      inputPath: null, inputType: "video", projectsRoot: "E:\\Projects", plannerEnabled: true, projects: [], quality: "balanced", colmapAcceleration: null, taskColmapAcceleration: null,
      video: null, imageSequence: null, plan: null, estimate: null, engines: [], phase: "running", progress: 0, progressMessage: "",
      latestEvent: null, latestRuntime: null, lastEventSequence: 0, events: [], result: null, error: null, errorAt: null,
    });
    mocks.getSharedTasks.mockReset().mockResolvedValue([]);
    mocks.onTaskUpdate.mockClear();
    mocks.getProjectOverview.mockReset().mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [] });
    mocks.getAppRuntimeStatus.mockReset().mockResolvedValue({ pipelineRunning: true, pipelineRunElapsedMs: 5_000, pipelineElapsedOffsetMs: 10_000, previewProjectId: null, taskAcceleration: null });
    mocks.initializeTelemetry.mockReset().mockResolvedValue({
      analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured",
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

  const mockLogViewport = () => {
    const log = container.querySelector<HTMLElement>(".live-log")!;
    let scrollTop = 0;
    let scrollHeight = 1_000;
    Object.defineProperties(log, {
      clientHeight: { configurable: true, get: () => 220 },
      scrollHeight: { configurable: true, get: () => scrollHeight },
      scrollTop: {
        configurable: true,
        get: () => scrollTop,
        set: (value: number) => { scrollTop = value; },
      },
    });
    return {
      log,
      getScrollTop: () => scrollTop,
      setScrollTop: (value: number) => { scrollTop = value; },
      setScrollHeight: (value: number) => { scrollHeight = value; },
    };
  };

  it("shows only the current stage and total elapsed metrics", () => {
    const labels = Array.from(container.querySelectorAll(".process-metrics small"), (node) => node.textContent);
    expect(labels).toEqual(["当前阶段", "总耗时"]);
  });

  it("continues total elapsed time after reconnecting to an active run", async () => {
    const elapsed = () => container.querySelectorAll(".process-metrics b")[1]?.textContent;
    const before = elapsed();
    await act(async () => new Promise((resolve) => window.setTimeout(resolve, 1_100)));
    expect(elapsed()).not.toBe(before);
  });

  it("shows the current stage percentage separately from overall progress", async () => {
    await act(async () => useAppStore.getState().receiveEvent({
      ...event(1),
      stage: "reconstructing",
      progress: 63.8,
      stageProgress: 47.8,
      indeterminate: false,
      current: null,
      total: null,
    }));
    expect(container.querySelector(".live-heading > .mono")?.textContent).toBe("63.8%");
    expect(container.querySelector(".stage-timeline li.active small")?.textContent).toBe("47.8%");
  });

  it("does not substitute overall progress when the current stage percentage is unavailable", async () => {
    await act(async () => useAppStore.getState().receiveEvent({
      ...event(1),
      stage: "reconstructing",
      progress: 47.3,
      stageProgress: null,
      indeterminate: true,
      current: null,
      total: null,
    }));
    expect(container.querySelector(".stage-timeline li.active small")).toBeNull();
  });

  it("shows the live training estimate below the process message without enabling the runtime panel", async () => {
    await act(async () => {
      useAppStore.getState().receiveEvent(event(1));
      useAppStore.getState().receiveEvent({
        ...event(2),
        kind: "runtime",
        runtime: {
          processId: 42,
          phase: "training",
          updatedAt: new Date().toISOString(),
          lastOutputAgeMs: 0,
          training: { iteration: 500, total: 8_000, startIter: 0, lod: 0, stepsPerSecond: 10, remainingSeconds: 90, splatCount: null, psnr: null, ssim: null },
          device: null,
          backend: null,
          config: {},
          resources: null,
        },
      });
    });
    expect(container.querySelector(".training-remaining")?.textContent).toContain("预计训练剩余时长");
    expect(container.querySelector(".training-remaining strong")).toBeNull();
    expect(container.querySelector(".training-remaining small")).toBeNull();
    expect(container.querySelector(".training-remaining")?.textContent).not.toContain("不含后续导出耗时");
    expect(container.querySelector(".runtime-panel")).toBeNull();
  });

  it("integrates automatic optimization into the quality card with matching typography copy", () => {
    const settings = container.querySelector(".quality-settings")!;
    const qualityList = settings.querySelector(".quality-list");
    const plannerSwitch = settings.querySelector<HTMLButtonElement>(".planner-switch");

    expect(qualityList).not.toBeNull();
    expect(plannerSwitch?.getAttribute("aria-checked")).toBe("true");
    expect(plannerSwitch?.disabled).toBe(true);
    expect(plannerSwitch?.textContent).toContain("自动优化（实验性）");
    expect(plannerSwitch?.textContent).toContain("开启后，将优化重建与训练参数并自动补救，通常可缩短生成时长，提升（尤其是图片素材）细节表现");
  });

  it("keeps the outer task pane fixed while following fewer than 500 log lines", async () => {
    const viewport = mockLogViewport();
    const controlPane = container.querySelector<HTMLElement>(".control-pane")!;
    controlPane.scrollTop = 137;

    await pushEvents(1, 20);

    expect(viewport.getScrollTop()).toBe(1_000);
    expect(controlPane.scrollTop).toBe(137);
  });

  it("keeps following new log lines after the 500-event cap is reached", async () => {
    const viewport = mockLogViewport();
    await pushEvents(1, 500);
    expect(useAppStore.getState().events).toHaveLength(500);
    expect(viewport.getScrollTop()).toBe(1_000);

    // The store now drops one event for every event it appends, so the log length is
    // pinned at 500 while a fine-detail run keeps emitting thousands more lines.
    viewport.setScrollHeight(1_400);
    await pushEvents(501, 520);

    expect(useAppStore.getState().events).toHaveLength(500);
    expect(useAppStore.getState().events.at(-1)?.message).toBe("step 520");
    expect(viewport.getScrollTop()).toBe(1_400);
  });

  it("pauses auto-follow while the user reads older logs and resumes at the bottom", async () => {
    const viewport = mockLogViewport();
    await pushEvents(1, 10);

    viewport.setScrollTop(120);
    await act(async () => { viewport.log.dispatchEvent(new Event("scroll", { bubbles: true })); });
    viewport.setScrollHeight(1_200);
    await pushEvents(11, 11);
    expect(viewport.getScrollTop()).toBe(120);

    viewport.setScrollTop(980);
    await act(async () => { viewport.log.dispatchEvent(new Event("scroll", { bubbles: true })); });
    viewport.setScrollHeight(1_500);
    await pushEvents(12, 12);
    expect(viewport.getScrollTop()).toBe(1_500);
  });

  it("shows plain-language progress with counts for the four processing stages", async () => {
    const cases = [
      ["extractingFeatures", "正在分析画面（8/20）"],
      ["matching", "正在寻找画面之间的联系（9/20）"],
      ["reconstructing", "正在还原拍摄场景（10/20）。此步骤可能耗时较长，请耐心等待"],
      ["trainingSplats", "正在生成高斯泼溅（11/20）。此步骤可能耗时较长，请耐心等待"],
    ] as const;

    for (const [index, [stage, expected]] of cases.entries()) {
      await act(async () => {
        useAppStore.getState().receiveEvent({
          ...event(index + 1),
          stage,
          message: `technical detail for ${stage}`,
          current: index + 8,
          total: 20,
        });
      });
      await flush();
      expect(container.querySelector(".current-message")?.textContent).toBe(expected);
    }
  });

  it("keeps the latest count when a later technical log has no count", async () => {
    await act(async () => {
      useAppStore.getState().receiveEvent({
        ...event(1), stage: "matching", message: "Matching pair batch", current: 12, total: 40,
      });
      useAppStore.getState().receiveEvent({
        ...event(2), stage: "matching", kind: "log", message: "Technical matcher detail", current: null, total: null,
      });
    });
    await flush();

    expect(container.querySelector(".current-message")?.textContent).toBe("正在寻找画面之间的联系（12/40）");
    expect(container.querySelector(".live-log")?.textContent).toContain("Technical matcher detail");
  });

  it("omits the count until progress data is available", async () => {
    await act(async () => {
      useAppStore.getState().receiveEvent({
        ...event(1), stage: "extractingFeatures", message: "COLMAP startup", current: null, total: null,
      });
    });
    await flush();

    expect(container.querySelector(".current-message")?.textContent).toBe("正在分析画面…");
    expect(container.querySelector(".live-log")?.textContent).toContain("COLMAP startup");
  });

  it("does not invent a Splat training step from stage progress", async () => {
    await act(async () => {
      useAppStore.getState().receiveEvent({
        ...event(1), stage: "trainingSplats", kind: "heartbeat", message: "Brush estimate", current: null, total: 30_000, stageProgress: 50,
      });
    });
    await flush();

    expect(container.querySelector(".current-message")?.textContent).toBe("正在生成高斯泼溅…。此步骤可能耗时较长，请耐心等待");

    await act(async () => { container.querySelector<HTMLButtonElement>(".language-action")!.click(); });
    expect(container.querySelector(".current-message")?.textContent).toBe("Generating Gaussian splats…. This step may take a while. Please wait.");
  });

  it("shows the actual Brush step reported by the runtime snapshot", async () => {
    await act(async () => {
      useAppStore.getState().receiveEvent({
        ...event(1),
        kind: "runtime",
        current: null,
        runtime: {
          processId: 42,
          phase: "training",
          updatedAt: new Date().toISOString(),
          lastOutputAgeMs: 0,
          training: { iteration: 12_345, total: 30_000, startIter: 0, lod: 0, stepsPerSecond: 100, remainingSeconds: 176.55, splatCount: null, psnr: null, ssim: null },
          device: "RTX",
          backend: "Vulkan",
          config: {},
          resources: null,
        },
      });
      useAppStore.getState().receiveEvent({ ...event(2), current: 12_345 });
    });
    await flush();
    expect(container.querySelector(".current-message")?.textContent).toBe("正在生成高斯泼溅（12,345/30,000）。此步骤可能耗时较长，请耐心等待");
    expect(container.querySelector(".runtime-panel")).toBeNull();
    await act(async () => container.querySelector<HTMLButtonElement>(".settings-action:not(.language-action)")?.click());
    await act(async () => container.querySelector<HTMLButtonElement>(".settings-switch")?.click());
    expect(container.querySelector(".runtime-panel")?.textContent).toContain("12,345 / 30,000");
  });

  it("keeps terminal messages instead of presenting them as active work", async () => {
    await act(async () => {
      useAppStore.getState().receiveEvent({
        ...event(1), stage: "matching", message: "Technical matcher detail", current: 4, total: 20,
      });
      useAppStore.getState().receiveEvent({
        ...event(2), stage: "cancelled", message: "任务已取消", current: null, total: null,
      });
    });
    await flush();

    expect(container.querySelector(".current-message")?.textContent).toBe("任务已取消");
  });

  it("keeps the real percentage and marks the failing stage", async () => {
    await act(async () => {
      useAppStore.getState().receiveEvent(event(1));
      useAppStore.getState().receiveEvent({
        ...event(2), stage: "failed", progress: 100, stageProgress: null, level: "error",
      });
      useAppStore.getState().setPhase("failed");
    });
    await flush();

    expect(container.querySelector(".live-heading .mono")?.textContent).toBe("50.0%");
    const timeline = container.querySelectorAll(".stage-timeline li");
    expect(timeline[5].classList.contains("failed")).toBe(true);
    expect(timeline[6].className).toBe("");
  });
  it("shows Agent tasks without switching selection and synchronizes failure independently of a generation promise", async () => {
    const { onTaskUpdate } = await import("../lib/backend");
    const callback = vi.mocked(onTaskUpdate).mock.calls.at(-1)![0];
    const task = {
      task_id: "agent-task", run_id: null, project_id: null, project_path: null,
      input_path: "E:\\Agent\\orbit.mp4", quality: "balanced", source: "mcp", task_kind: "generation",
      planner_enabled: false, projects_root: "E:\\Projects", status: "created", stage: null,
      revision: 1, sequence: 0, created_at: "2026-10-06T00:00:00Z", updated_at: "2026-10-06T00:00:00Z",
      elapsed_ms: 0, progress: null, estimated_progress: null, current: null, total: null, eta_seconds: null,
      error: null, result: null, recent_events: [],
    } as import("../types/tasks").SharedTask;
    await act(async () => callback({ task, event: null }));
    expect(container.textContent).toContain("由 AI Agent 创建");
    expect(container.querySelector(".shared-task-detail")).toBeNull();
    await act(async () => container.querySelector<HTMLElement>('[data-task-id="agent-task"]')!.click());
    expect(container.querySelector(".shared-task-detail")?.textContent).toContain("orbit.mp4");
    await act(async () => callback({ task: { ...task, run_id: "agent-run", revision: 2, status: "running" }, event: null }));
    await act(async () => callback({ task: { ...task, run_id: "agent-run", revision: 3, status: "failed", stage: "failed", error: { code: "pipeline_failed", message: "模型校验失败", failed_stage: "trainingSplats", engine: "brush", exit_code: 2, failure_id: null, classification: "brush_gpu", classification_is_heuristic: true } }, event: null }));
    expect(container.querySelector(".shared-task-detail")?.textContent).toContain("失败");
    expect(container.querySelector(".shared-task-detail")?.textContent).toContain("模型校验失败");
    expect(container.querySelector('.unfinished [data-task-id="agent-task"]')?.textContent).toContain("失败");
    expect(useAppStore.getState().phase).toBe("failed");
  });

});
