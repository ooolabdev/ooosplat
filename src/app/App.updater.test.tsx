// @vitest-environment jsdom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../stores/appStore";
import { useGaussianTransformStore } from "../stores/gaussianTransformStore";

const mocks = vi.hoisted(() => ({
  downloadAppUpdate: vi.fn(),
  installAppUpdate: vi.fn(),
  discardAppUpdate: vi.fn(),
  checkForAppUpdate: vi.fn(),
  updaterEnabled: true,
  pluginMissing: false,
}));

vi.mock("../lib/backend", () => ({
  cancelPipeline: vi.fn().mockResolvedValue(undefined),
  checkEngines: vi.fn().mockResolvedValue([]),
  confirmAndDeleteProject: vi.fn().mockResolvedValue(false),
  confirmLargeImageSequence: vi.fn().mockResolvedValue(true),
  estimateProjectRuntime: vi.fn().mockResolvedValue(null),
  getProjectOverview: vi.fn().mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [] }),
  initializeTelemetry: vi.fn().mockResolvedValue({ analyticsEnabled: false, consentDecided: true, deliveryStatus: "disabled" }),
  onPipelineEvent: vi.fn().mockResolvedValue(() => undefined),
  prepareGaussianPreview: vi.fn(),
  probeAndPlan: vi.fn(),
  releaseGaussianPreview: vi.fn().mockResolvedValue(undefined),
  resumePipeline: vi.fn(),
  revealProject: vi.fn(),
  selectProjectsRoot: vi.fn(),
  selectImageSequence: vi.fn().mockResolvedValue(null),
  selectVideo: vi.fn().mockResolvedValue(null),
  setProjectsRoot: vi.fn(),
  setTelemetryConsent: vi.fn(),
  startPipeline: vi.fn(),
  startReshootPipeline: vi.fn(),
}));

vi.mock("../components/GaussianViewer", () => ({
  GaussianViewer: () => <section className="preview-workspace"><h1>高斯泼溅预览</h1></section>,
}));

vi.mock("../lib/updater", () => ({
  checkForAppUpdate: mocks.checkForAppUpdate,
  downloadAppUpdate: mocks.downloadAppUpdate,
  installAppUpdate: mocks.installAppUpdate,
  discardAppUpdate: mocks.discardAppUpdate,
  isUpdaterEnabled: () => mocks.updaterEnabled,
  isUpdaterPluginMissing: () => mocks.pluginMissing,
}));

import { App } from "./App";

const flush = async () => {
  await act(async () => {
    await Promise.resolve();
    await new Promise((resolve) => window.setTimeout(resolve, 0));
  });
};

const buttonWithText = (container: HTMLElement, text: string) =>
  [...container.querySelectorAll("button")].find((button) => button.textContent?.includes(text));

describe("App in-app updates", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(async () => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    if (!window.requestAnimationFrame) {
      window.requestAnimationFrame = (callback) => window.setTimeout(() => callback(performance.now()), 0);
      window.cancelAnimationFrame = (handle) => window.clearTimeout(handle);
    }
    // jsdom has no layout, so the live log's auto-scroll must be stubbed.
    Element.prototype.scrollIntoView = vi.fn();
    useGaussianTransformStore.getState().close();
    useAppStore.setState({
      inputPath: null, inputType: "video", projectsRoot: "E:\\Projects", projects: [], quality: "balanced", colmapAcceleration: null,
      video: null, imageSequence: null, plan: null, estimate: null, engines: [], phase: "idle", progress: 0, progressMessage: "",
      latestEvent: null, events: [], result: null, error: null,
    });
    mocks.updaterEnabled = true;
    mocks.pluginMissing = false;
    mocks.checkForAppUpdate.mockReset().mockResolvedValue(null);
    mocks.downloadAppUpdate.mockReset().mockResolvedValue(undefined);
    mocks.installAppUpdate.mockReset().mockResolvedValue(undefined);
    mocks.discardAppUpdate.mockReset().mockResolvedValue(undefined);
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => { root.unmount(); });
    container.remove();
  });

  const render = async () => {
    root = createRoot(container);
    await act(async () => { root.render(<App />); });
    await flush();
  };

  const renderWithUpdate = async () => {
    mocks.checkForAppUpdate.mockResolvedValue({ version: "0.5.0" });
    await render();
    return container.querySelector<HTMLButtonElement>(".update-action");
  };

  it("hides every update control in a build that is not an official release", async () => {
    mocks.updaterEnabled = false;
    await render();

    expect(mocks.checkForAppUpdate).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain("检查更新");
    expect(container.textContent).not.toContain("已是最新");
    expect(container.querySelector(".update-action")).toBeNull();
  });

  it("hides the updater instead of failing forever when the binary lacks the plugin", async () => {
    mocks.checkForAppUpdate.mockRejectedValue(new Error("Command plugin:updater|check not found"));
    mocks.pluginMissing = true;
    await render();

    expect(container.textContent).not.toContain("检查更新失败");
    expect(container.querySelector(".update-action")).toBeNull();
    expect(container.querySelector(".update-progress")).toBeNull();
  });

  it("reports no failure when an official build is already current", async () => {
    mocks.checkForAppUpdate.mockResolvedValue(null);
    await render();

    expect(mocks.checkForAppUpdate).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("已是最新");
    expect(container.textContent).not.toContain("检查更新失败");
  });

  it("offers installation when a signed update is available", async () => {
    const updateButton = await renderWithUpdate();

    expect(updateButton?.textContent).toContain("更新至 0.5.0");
    expect(updateButton?.disabled).toBe(false);

    await act(async () => { updateButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();
    expect(mocks.downloadAppUpdate).toHaveBeenCalledTimes(1);
    expect(mocks.installAppUpdate).toHaveBeenCalledTimes(1);
  });

  it("refuses to install an update while a task is running", async () => {
    mocks.checkForAppUpdate.mockResolvedValue({ version: "0.5.0" });
    useAppStore.setState({ phase: "running" });
    await render();

    const blockedButton = container.querySelector<HTMLButtonElement>(".update-action");
    expect(blockedButton?.textContent).toContain("任务完成后可更新");
    expect(blockedButton?.disabled).toBe(true);

    await act(async () => { blockedButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();
    expect(mocks.downloadAppUpdate).not.toHaveBeenCalled();
    expect(mocks.installAppUpdate).not.toHaveBeenCalled();
  });

  it("does not restart the app when a task starts during the download", async () => {
    let finishDownload: (() => void) | undefined;
    mocks.downloadAppUpdate.mockImplementation(() => new Promise<void>((resolve) => { finishDownload = resolve; }));
    const updateButton = await renderWithUpdate();

    await act(async () => { updateButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();
    expect(mocks.downloadAppUpdate).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("正在下载更新");

    // A task begins while the package is still downloading.
    act(() => useAppStore.setState({ phase: "running" }));
    await flush();
    await act(async () => { finishDownload?.(); });
    await flush();

    expect(mocks.installAppUpdate).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain("正在下载更新");
    expect(container.querySelector(".update-action")?.textContent).toContain("任务完成后可更新");
  });

  it("blocks starting a task while an update package is downloading", async () => {
    let finishDownload: (() => void) | undefined;
    mocks.downloadAppUpdate.mockImplementation(() => new Promise<void>((resolve) => { finishDownload = resolve; }));
    useAppStore.setState({
      inputPath: "E:\\clips\\input.mp4",
      projectsRoot: "E:\\Projects",
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
    });
    const updateButton = await renderWithUpdate();

    expect(container.querySelector<HTMLButtonElement>(".primary-action")?.disabled).toBe(false);

    await act(async () => { updateButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    const startButton = container.querySelector<HTMLButtonElement>(".primary-action");
    expect(startButton?.disabled).toBe(true);
    expect(startButton?.textContent).toContain("更新下载完成后可开始");

    await act(async () => { finishDownload?.(); });
    await flush();
  });

  it("reuses the downloaded package when installation was postponed", async () => {
    mocks.checkForAppUpdate.mockResolvedValue({ version: "0.5.0" });
    useAppStore.setState({ phase: "running" });
    await render();

    // The task finishes, the user installs, and a second click must not redownload.
    act(() => useAppStore.setState({ phase: "completed" }));
    await flush();

    const updateButton = container.querySelector<HTMLButtonElement>(".update-action");
    await act(async () => { updateButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(mocks.downloadAppUpdate).toHaveBeenCalledTimes(1);
    expect(mocks.installAppUpdate).toHaveBeenCalledTimes(1);
  });

  it("surfaces a failed update check so the user can retry", async () => {
    mocks.checkForAppUpdate.mockRejectedValue(new Error("network unreachable"));
    await render();

    expect(container.textContent).toContain("检查更新失败");
    expect(buttonWithText(container, "检查更新失败")?.getAttribute("title")).toBe("network unreachable");
  });

  it("surfaces a failed download instead of claiming success", async () => {
    mocks.checkForAppUpdate.mockResolvedValue({ version: "0.5.0" });
    mocks.downloadAppUpdate.mockRejectedValue(new Error("下载中断"));
    await render();

    const updateButton = container.querySelector<HTMLButtonElement>(".update-action");
    await act(async () => { updateButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain("更新失败，重试");
    expect(container.textContent).not.toContain("检查更新失败");
    expect(buttonWithText(container, "更新失败，重试")?.getAttribute("title")).toBe("下载中断");
    expect(mocks.installAppUpdate).not.toHaveBeenCalled();

    // Retrying a failed download must not re-check the feed.
    await act(async () => { buttonWithText(container, "更新失败，重试")?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();
    expect(mocks.downloadAppUpdate).toHaveBeenCalledTimes(2);
    expect(mocks.checkForAppUpdate).toHaveBeenCalledTimes(1);
  });
});
