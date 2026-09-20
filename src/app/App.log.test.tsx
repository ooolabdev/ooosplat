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
  getAppSettings: vi.fn(),
  getAppRuntimeStatus: vi.fn(),
}));

vi.mock("../lib/backend", () => ({
  cancelPipeline: vi.fn(),
  checkEngines: vi.fn().mockResolvedValue([]),
  confirmAndDeleteProject: vi.fn().mockResolvedValue(false),
  confirmLargeImageSequence: vi.fn().mockResolvedValue(true),
  estimateProjectRuntime: vi.fn(),
  exportPly: vi.fn(),
  getAppRuntimeStatus: mocks.getAppRuntimeStatus,
  getProjectOverview: mocks.getProjectOverview,
  getAppSettings: mocks.getAppSettings,
  initializeTelemetry: mocks.initializeTelemetry,
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
  setPlannerPreference: vi.fn(),
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
      inputPath: null, inputType: "video", projectsRoot: "E:\\Projects", projects: [], quality: "balanced", colmapAcceleration: null,
      video: null, imageSequence: null, plan: null, estimate: null, engines: [], phase: "running", progress: 0, progressMessage: "",
      latestEvent: null, events: [], result: null, error: null,
    });
    mocks.getProjectOverview.mockReset().mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [] });
    mocks.getAppSettings.mockReset().mockResolvedValue({ projectsRoot: "E:\\Projects", plannerEnabled: false, plannerPreference: "askEachTime" });
    mocks.getAppRuntimeStatus.mockReset().mockResolvedValue({ pipelineRunning: true, previewProjectId: null });
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

  it("shows the last registered count during mapper refinement", async () => {
    await act(async () => {
      useAppStore.getState().receiveEvent({
        ...event(1),
        stage: "reconstructing",
        engine: "colmap",
        message: "Retriangulation and Global bundle adjustment",
        current: 86,
        total: 100,
      });
    });
    await flush();

    expect(container.querySelector(".current-message")?.textContent).toBe("Retriangulation and Global bundle adjustment · 已注册 86/100");

    await act(async () => { container.querySelector<HTMLButtonElement>(".language-action")!.click(); });
    expect(container.querySelector(".current-message")?.textContent).toBe("Retriangulation and Global bundle adjustment · Registered 86/100");
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
});
