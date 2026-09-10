// @vitest-environment jsdom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../stores/appStore";
import type { PipelineResult } from "../types/pipeline";

const mocks = vi.hoisted(() => ({
  initializeTelemetry: vi.fn(),
  getProjectOverview: vi.fn(),
}));

vi.mock("../lib/backend", () => ({
  cancelPipeline: vi.fn(),
  checkEngines: vi.fn().mockResolvedValue([]),
  confirmAndDeleteProject: vi.fn().mockResolvedValue(false),
  confirmLargeImageSequence: vi.fn().mockResolvedValue(true),
  estimateProjectRuntime: vi.fn(),
  getProjectOverview: mocks.getProjectOverview,
  initializeTelemetry: mocks.initializeTelemetry,
  onPipelineEvent: vi.fn().mockResolvedValue(() => undefined),
  prepareGaussianPreview: vi.fn(),
  probeAndPlan: vi.fn(),
  releaseGaussianPreview: vi.fn(),
  resumePipeline: vi.fn(),
  revealProject: vi.fn(),
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

const result = (warning: string | null): PipelineResult => ({
  projectId: "11111111-1111-1111-1111-111111111111",
  projectPath: "E:\\Projects\\示例项目",
  finalPly: "E:\\Projects\\示例项目\\final.ply",
  fileSize: 73_729_603,
  splatCount: 312_407,
  inputImages: 200,
  registeredImages: 110,
  registeredRatio: 0.55,
  points3d: 10_000,
  durationMs: 3_600_000,
  completedAt: "2026-08-22T15:00:00Z",
  warning,
  logsDirectory: "E:\\Projects\\示例项目\\logs",
});

const flush = async () => {
  await act(async () => {
    await Promise.resolve();
    await new Promise((resolve) => window.setTimeout(resolve, 0));
  });
};

describe("App reconstruction warning", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(async () => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    // jsdom does not implement scrollIntoView, and starting a run renders the live log,
    // whose auto-scroll effect then calls it.
    Element.prototype.scrollIntoView = vi.fn();
    useAppStore.setState({
      inputPath: null, inputType: "video", projectsRoot: "E:\\Projects", projects: [], quality: "balanced", colmapAcceleration: null,
      video: null, imageSequence: null, plan: null, estimate: null, engines: [], phase: "idle", progress: 0, progressMessage: "",
      latestEvent: null, events: [], result: null, error: null,
    });
    mocks.getProjectOverview.mockReset().mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [] });
    mocks.initializeTelemetry.mockReset().mockResolvedValue({
      analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured",
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

  it("shows the reconstruction quality warning the backend returns", async () => {
    expect(container.querySelector(".inline-warning")).toBeNull();

    const warning = "注册率 55.0%：低于 80%，将继续训练，但结果质量可能受影响";
    await act(async () => {
      useAppStore.setState({ phase: "completed" });
      useAppStore.getState().setResult(result(warning));
    });
    await flush();

    expect(container.querySelector(".inline-warning")?.textContent).toContain(warning);
  });

  it("stays hidden for a run the backend did not flag", async () => {
    await act(async () => {
      useAppStore.setState({ phase: "completed" });
      useAppStore.getState().setResult(result(null));
    });
    await flush();

    expect(container.querySelector(".inline-warning")).toBeNull();
  });

  it("clears the warning when the next run starts", async () => {
    await act(async () => {
      useAppStore.setState({ phase: "completed" });
      useAppStore.getState().setResult(result("注册率 55.0%：低于 80%，将继续训练，但结果质量可能受影响"));
    });
    await flush();
    expect(container.querySelector(".inline-warning")).not.toBeNull();

    await act(async () => { useAppStore.getState().beginRun(); });
    await flush();

    expect(container.querySelector(".inline-warning")).toBeNull();
  });
});
