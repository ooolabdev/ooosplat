// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider, useI18n } from "../i18n";
import type { RuntimeSnapshot } from "../types/pipeline";
import { RuntimePanel } from "./RuntimePanel";

let container: HTMLDivElement;
let root: Root;

const snapshot = (): RuntimeSnapshot => ({
  processId: 42,
  phase: "training",
  updatedAt: new Date().toISOString(),
  lastOutputAgeMs: 0,
  training: { iteration: 50, total: 100, startIter: 0, lod: 0, stepsPerSecond: 10, remainingSeconds: 5, splatCount: 200, psnr: 25, ssim: 0.9 },
  device: "RTX 3060 Ti",
  backend: "Vulkan",
  config: { seed: "42", max_resolution: "50" },
  resources: {
    processId: 42,
    sampledAt: new Date().toISOString(),
    cpuPercent: 25,
    memoryBytes: 104_857_600,
    gpuStatus: "available",
    gpus: [{ uuid: "GPU-a", name: "RTX 3060 Ti", utilizationPercent: 80, memoryUsedMib: 1000, memoryTotalMib: 8192 }],
  },
});

function SwitchLanguage() {
  const { toggleLocale } = useI18n();
  return <button onClick={toggleLocale}>switch</button>;
}

async function render(value: RuntimeSnapshot, running = true) {
  await act(async () => root.render(<LanguageProvider><SwitchLanguage /><RuntimePanel snapshot={value} running={running} /></LanguageProvider>));
}

beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear();
  localStorage.setItem("ooo-splat-language", "zh-CN");
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.useRealTimers();
});

describe("runtime panel", () => {
  it("shows actual steps, resources and translated details", async () => {
    await render(snapshot());
    expect(container.textContent).toContain("50 / 100");
    expect(container.textContent).toContain("10 步/秒");
    expect(container.textContent).toContain("整卡占用，包含其他程序");
    expect(container.textContent).toContain("100 MiB");
    expect(container.querySelector("progress")?.value).toBe(50);
    await act(async () => container.querySelector("button")!.click());
    expect(container.textContent).toContain("Actual training steps");
    expect(container.textContent).toContain("Whole-card use, including other programs");
  });

  it("shows output gaps and stale samples without claiming a hang", async () => {
    const value = snapshot();
    value.lastOutputAgeMs = 11_000;
    value.resources = { ...value.resources!, sampledAt: new Date(Date.now() - 7_000).toISOString(), cpuPercent: null, memoryBytes: null, gpus: [], gpuStatus: "unsupported" };
    await render(value);
    expect(container.textContent).toContain("进程仍运行，暂未收到新输出");
    expect(container.textContent).toContain("数据已过期");
    expect(container.textContent).toContain("暂不支持");
    expect(container.textContent).not.toContain("10 步/秒");
  });

  it("freezes the output timer and rate when a task stops", async () => {
    vi.useFakeTimers();
    await render(snapshot(), false);
    const before = container.textContent;
    await act(async () => vi.advanceTimersByTime(12_000));
    expect(container.textContent).toBe(before);
    expect(before).toContain("已停止更新");
    expect(before).not.toContain("10 步/秒");
  });
});
