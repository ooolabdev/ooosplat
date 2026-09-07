// @vitest-environment jsdom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../stores/appStore";
import type { PipelineEvent } from "../types/pipeline";

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
  let scrollIntoView: ReturnType<typeof vi.fn>;

  beforeEach(async () => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;
    useAppStore.setState({
      inputPath: null, inputType: "video", projectsRoot: "E:\\Projects", projects: [], quality: "balanced", colmapAcceleration: null,
      video: null, imageSequence: null, plan: null, estimate: null, engines: [], phase: "running", progress: 0, progressMessage: "",
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

  it("keeps following new log lines after the 500-event cap is reached", async () => {
    await pushEvents(1, 500);
    expect(useAppStore.getState().events).toHaveLength(500);

    // The store now drops one event for every event it appends, so the log length is
    // pinned at 500 while a fine-detail run keeps emitting thousands more lines.
    const callsAtCap = scrollIntoView.mock.calls.length;
    await pushEvents(501, 520);

    expect(useAppStore.getState().events).toHaveLength(500);
    expect(useAppStore.getState().events.at(-1)?.message).toBe("step 520");
    expect(scrollIntoView.mock.calls.length).toBeGreaterThan(callsAtCap);
  });
});
