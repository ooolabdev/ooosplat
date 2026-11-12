// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider, useI18n } from "../i18n";
import type { RuntimeSnapshot } from "../types/pipeline";
import { RuntimePanel } from "./RuntimePanel";
import { TaskProgress } from './TaskProgress';

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

async function render(value: RuntimeSnapshot | null, running = true) {
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
  it.each(['zh-CN', 'en'])('does not retain the finishing monitor after a durable completion (%s)', async locale => {
    localStorage.setItem('ooo-splat-language', locale);
    const value = { ...snapshot(), phase: 'finishing', training: { ...snapshot().training!, iteration: 100 } };
    const show = async (status: string) => act(async () => root.render(<LanguageProvider><TaskProgress status={status} stage={status === 'completed' ? 'completed' : 'trainingSplats'} progress={100} runtime={value} logs={[{ key: 'published', text: '已发布 200 个 Splat', source: 'system' }]} /></LanguageProvider>));
    await show('running');
    expect(container.querySelector('.runtime-panel')).not.toBeNull();
    expect(container.querySelector('.live-heading .mono')?.textContent).toBe('99.9%');
    await show('completed');
    expect(container.querySelector('.runtime-panel')).toBeNull();
    expect(container.querySelector('.live-heading .mono')?.textContent).toBe('100.0%');
    expect(container.querySelector('.live-log')?.textContent).toContain('200');
  });
  it('keeps the same monitor container across preparation, samples and a true operation transition', async () => {
    const renderTask = async (runtime: RuntimeSnapshot | null, substage: string) => act(async () => root.render(<LanguageProvider><TaskProgress status="running" stage="trainingSplats" substage={substage} progress={99.99} runtime={runtime} logs={[]} message="正在准备训练图片" /></LanguageProvider>));
    await renderTask(null, 'panoramaTrainingPreparation');
    const panel = container.querySelector('.runtime-panel');
    expect(panel).not.toBeNull();
    expect(container.querySelector('.stage-timeline li.active b')?.textContent).toBe('Splat 训练');
    expect(container.querySelector('.process-metrics b')?.textContent).toBe('Splat 训练');
    expect(container.querySelector('.live-heading .mono')?.textContent).toBe('99.9%');
    for (let index = 0; index < 10; index++) {
      await renderTask(snapshot(), 'training');
      expect(container.querySelector('.runtime-panel')).toBe(panel);
    }
    await renderTask(null, 'training');
    expect(container.querySelector('.runtime-panel')).toBe(panel);
    expect(panel?.textContent).not.toContain('42');
    expect(panel?.querySelector('.runtime-step-heading')).toBeNull();
  });
  it("shows actual steps, resources and translated details", async () => {
    await render(snapshot());
    expect(container.textContent).toContain("50 / 100");
    expect(container.textContent).toContain("10 步/秒");
    expect(container.textContent).toContain("整卡占用，包含其他程序");
    expect(container.textContent).toContain("100 MiB");
    expect(container.querySelector("progress")?.value).toBe(50);
    expect(container.querySelector(".runtime-metrics > div:nth-child(2) small")).toBeNull();
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

  it('keeps the resource and GPU containers while a real process switches without borrowing its old samples', async () => {
    const first = { ...snapshot(), phase: 'running', training: null, config: {}, device: null, backend: null };
    await render(first);
    const panel = container.querySelector('.runtime-panel');
    const resources = container.querySelector('.runtime-resources');
    const gpu = container.querySelector('.runtime-gpus');
    expect(resources?.textContent).toContain('100 MiB');
    await render(null);
    expect(container.querySelector('.runtime-panel')).toBe(panel);
    expect(container.querySelector('.runtime-resources')).toBe(resources);
    expect(container.querySelector('.runtime-gpus')).toBe(gpu);
    expect(container.querySelector('.runtime-heading')?.textContent).not.toContain('42');
    expect(resources?.textContent).toContain('采样中');
    expect(gpu?.textContent).not.toContain('RTX');
    await render({ ...first, processId: 43 }); // stale sample is still owned by PID 42
    expect(resources?.textContent).not.toContain('100 MiB');
    expect(gpu?.textContent).not.toContain('80%');
    const second = { ...first, processId: 43, resources: { ...first.resources!, processId: 43, memoryBytes: 50 * 1024 * 1024 } };
    await render(second);
    expect(container.querySelector('.runtime-resources')).toBe(resources);
    expect(resources?.textContent).toContain('50 MiB');
  });

  it('does not show an exited process as active while the task is still saving or validating', async () => {
    const stopped = { ...snapshot(), phase: 'stopped' };
    await render(stopped, true);
    const before = container.querySelector('.runtime-output')?.textContent;
    expect(container.querySelector('.runtime-heading')?.textContent).toContain('已停止更新');
    expect(container.querySelector('.runtime-heading')?.textContent).not.toContain('PID');
    expect(container.querySelector('.runtime-panel')?.getAttribute('aria-busy')).toBe('false');
    expect(container.querySelector('.runtime-resources')?.textContent).not.toContain('100 MiB');
    expect(container.querySelector('.runtime-gpus')?.textContent).not.toContain('80%');
    expect(container.textContent).toContain('50 / 100');
    expect(container.textContent).not.toContain('10 步/秒');
    expect(container.querySelector('.runtime-output')?.textContent).toBe(before);
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
