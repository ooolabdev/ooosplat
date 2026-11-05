// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { detectSystemLocale, LanguageProvider, localizePipelineMessage, readInitialLocale, translate, useI18n } from ".";

it("panorama initialization limit uses approved English copy", () => {
  const message = "初始化点数超出当前全景训练预算，已停止训练以避免丢弃初始点。";
  expect(localizePipelineMessage("en", message)).toBe("Initial point count exceeds the current panorama training budget. Training stopped to avoid dropping initial points.");
  expect(localizePipelineMessage("en", `外部进程执行失败：${message}`)).toBe(localizePipelineMessage("en", message));
  expect(localizePipelineMessage("zh-CN", message)).toBe(message);
});

it('localizes the approved reshoot lifecycle logs without exposing Chinese cache labels', () => {
  for (const [message, expected] of [
    ['正在准备原项目文件：3/20', 'Preparing original project files: 3/20'],
    ['正在准备补拍画面：3/20', 'Preparing reshoot frames: 3/20'],
    ['正在整理补拍画面：3/20', 'Organizing reshoot frames: 3/20'],
    ['正在检查画面与蒙版：3/20', 'Checking images and masks: 3/20'],
    ['补拍画面准备完成：原画面 20 张，新增 3 张', 'Reshoot frames prepared: 20 original images, 3 new images'],
    ['补拍画面已加入：3/20', 'Reshoot images registered: 3/20'],
    ['补拍相机设置：沿用原设置', 'Reshoot camera settings: reusing the original settings'],
    ['补拍相机设置：独立处理', 'Reshoot camera settings: processing independently'],
    ['补拍相机设置：条件不兼容，独立处理', 'Reshoot camera settings: incompatible conditions, processing independently'],
    ['正在准备原模型训练副本', 'Preparing the original model training snapshot'],
    ['复用已完成的补拍缓存：原项目文件', 'Reusing completed reshoot cache: original project files'],
    ['补拍处理仍在进行，当前步骤可能耗时较长，请耐心等待', 'Reshoot processing is still running. This step may take a while. Please wait.'],
  ]) {
    expect(localizePipelineMessage('en', message)).toBe(expected);
    expect(localizePipelineMessage('zh-CN', message)).toBe(message);
  }
});

it('localizes Brush training start and step progress messages', () => {
  for (const [message, expected] of [
    ['Brush 训练开始（使用可用图形后端）· 15000 iterations · 最大分辨率 1920 · 预计约 00:12:34', 'Brush training started (using an available graphics backend) · 15000 iterations · max resolution 1920 · estimated about 00:12:34'],
    ['Brush 训练开始（使用可用图形后端）· 30000 iterations · 最大分辨率 3840 · 预计约 100:05:09', 'Brush training started (using an available graphics backend) · 30000 iterations · max resolution 3840 · estimated about 100:05:09'],
    ['Brush 训练 · 1200/15000 步', 'Brush training · 1200/15000 steps'],
  ]) {
    expect(localizePipelineMessage('en', message)).toBe(expected);
    expect(localizePipelineMessage('zh-CN', message)).toBe(message);
  }
});

function Harness() {
  const { locale, t, toggleLocale, formatDuration } = useI18n();
  return <div>
    <span data-locale={locale}>{t("task.create")}</span>
    <span>{formatDuration(62_000)}</span>
    <button type="button" onClick={toggleLocale}>{t("language.target")}</button>
  </div>;
}

describe("interface language", () => {
  it.each([
    'source.unsupportedImages', 'source.panoramaImagesBudget',
    'source.panoramaReshoot', 'source.ambiguousImages',
    'reshoot.fallbackRunning', 'reshoot.multiCameraHint', 'reshoot.panoramaHint',
  ] as const)("preserves source/reshoot translations and diagnostic suffixes (%s)", key => {
    const message = translate('zh-CN', key);
    const expected = translate('en', key);
    expect(expected).not.toBe(message);
    for (const suffix of [
      '',
      '\nDiagnostic log: E:/logs/ffprobe-probe-latest.log',
      '\nCould not save diagnostic log: access denied',
    ]) {
      expect(localizePipelineMessage('en', message + suffix)).toBe(expected + suffix);
      expect(localizePipelineMessage('zh-CN', message + suffix)).toBe(message + suffix);
    }
  });
  it("preserves the log location without preventing error localization", () => {
    expect(localizePipelineMessage("en", "找不到本地处理引擎：ffprobe.exe\nDiagnostic log: E:/logs/probe.log"))
      .toBe("Local processing engine not found: ffprobe.exe\nDiagnostic log: E:/logs/probe.log");
  });
  it("explains a missing FFprobe DLL in English", () => {
    const message = "本地处理引擎无法启动：ffprobe.exe（缺少运行所需的 DLL（退出码 0xC0000135）\nno diagnostic output）\nDiagnostic log: E:/logs/probe.log";
    expect(localizePipelineMessage("en", message)).toBe("Local processing engine could not start: ffprobe.exe (required DLL missing, exit code 0xC0000135)\nno diagnostic output\nDiagnostic log: E:/logs/probe.log");
  });
  it("localizes engine health failures while keeping their diagnostic output", () => {
    expect(localizePipelineMessage("en", "帮助命令退出码：1\nmissing dll"))
      .toBe("Engine check exit code: 1\nmissing dll");
    expect(localizePipelineMessage("en", "未找到 E:/engines/ffprobe.exe"))
      .toBe("Not found: E:/engines/ffprobe.exe");
  });
  it("localizes FFprobe failures while preserving multiline diagnostics", () => {
    const message = "外部进程执行失败：FFprobe 视频分析失败，退出码 1，引擎 engines/ffprobe.exe\nheader\nmoov atom not found";
    expect(localizePipelineMessage("zh-CN", message)).toBe(message);
    expect(localizePipelineMessage("en", message)).toBe(
      "FFprobe video analysis failed, exit code 1, executable engines/ffprobe.exe\nheader\nmoov atom not found",
    );
  });
  it("uses compact navigation labels without the removed speed/limitation copy", () => {
    expect(translate('zh-CN', 'viewer.roamHint')).toContain('自动定位视角后');
    expect(translate('en', 'viewer.roamHint')).toContain('Auto locate view');
    expect(translate('zh-CN', 'viewer.roam')).toBe('漫游');
    expect(translate('en', 'viewer.roam')).toBe('Fly');
    expect(translate('zh-CN', 'viewer.rotate')).toBe('旋转');
    expect(translate('en', 'viewer.rotate')).toBe('Rotate');
    expect(translate('zh-CN', 'viewer.locateHint')).toContain('切换到该透视视角');
    expect(translate('en', 'viewer.locateHint')).toContain('back to perspective');
    for (const locale of ['zh-CN', 'en'] as const) {
      expect(translate(locale, 'viewer.roamHint')).not.toMatch(/Shift|仅透视|perspective only/i);
    }
  });
  it("localizes matching silence notices as system status, not invented engine progress", () => {
    const message = '图像匹配进程仍在运行，最近 15 秒无新输出。此步骤可能耗时较长，请耐心等待';
    expect(localizePipelineMessage('en', message)).toBe('The image-matching process is still running, with no new output for 15 seconds. This step may take a while. Please wait.');
    expect(localizePipelineMessage('zh-CN', message)).toBe(message);
    expect(translate('en', 'viewer.locate')).toBe('Auto locate view');
  });
  it("localizes optional loop detection messages without relabeling exhaustive matching", () => {
    expect(localizePipelineMessage("en", "COLMAP 正在进行 GPU 顺序匹配（已开启闭环检测）")).toBe("COLMAP is running sequential matching with GPU (loop detection enabled)");
    expect(localizePipelineMessage("en", "顺序匹配（已开启闭环检测）完成")).toBe("Sequential matching (loop detection enabled) completed");
    expect(localizePipelineMessage("en", "COLMAP 正在进行 CPU 穷举匹配")).toBe("COLMAP is running exhaustive matching with CPU");
    expect(localizePipelineMessage("en", "闭环检测所需的辅助文件缺失或损坏。请重新安装 OOOSplat，或关闭自动优化后重试。\nSHA256 mismatch")).toContain("Reinstall OOOSplat, or turn off Auto Optimize");
    expect(localizePipelineMessage("en", "外部进程执行失败：闭环检测所需的辅助文件缺失或损坏。请重新安装 OOOSplat，或关闭自动优化后重试。\nSHA256 mismatch")).toContain("Reinstall OOOSplat, or turn off Auto Optimize");
    expect(localizePipelineMessage("zh-CN", "COLMAP 正在进行 GPU 顺序匹配（已开启闭环检测）")).toContain("已开启闭环检测");
  });
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    window.localStorage.clear();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    window.localStorage.clear();
  });

  it("maps every Chinese system locale to Simplified Chinese and all others to English", () => {
    expect(detectSystemLocale("zh-CN")).toBe("zh-CN");
    expect(detectSystemLocale("zh_TW")).toBe("zh-CN");
    expect(detectSystemLocale("en-US")).toBe("en");
    expect(detectSystemLocale("de-DE")).toBe("en");
  });

  it("uses a valid saved choice and ignores invalid storage", () => {
    window.localStorage.setItem("ooo-splat-language", "en");
    expect(readInitialLocale()).toBe("en");
    window.localStorage.setItem("ooo-splat-language", "broken");
    expect(["zh-CN", "en"]).toContain(readInitialLocale());
  });

  it("uses backend-neutral renderer status messages", () => {
    expect(translate("zh-CN", "viewer.initializing")).toBe("正在初始化图形渲染器");
    expect(translate("en", "viewer.initializing")).toBe("Initializing graphics renderer");
    expect(translate("zh-CN", "viewer.contextLost")).not.toContain("WebGL2");
    expect(translate("en", "viewer.contextLost")).not.toContain("WebGL2");
  });

  it("localizes image import and alpha preparation progress", () => {
    expect(localizePipelineMessage("en", "正在导入图片 12/30 张")).toBe("Importing images 12/30");
    expect(localizePipelineMessage("en", "正在导入补拍图片 3/8 张")).toBe("Importing reshoot images 3/8");
    expect(localizePipelineMessage("en", "正在检测 Alpha 并生成 Mask 2/5 张")).toBe("Checking alpha and generating masks 2/5");
    expect(localizePipelineMessage("en", "图片序列 30 张 · 1920×1080 · 检测到 Alpha 通道")).toBe("Image sequence 30 images · 1920×1080 · alpha channel detected");
  });

  it("localizes Brush device loss without exposing raw driver details", () => {
    const message = "Splat 训练期间显卡设备连接中断。请接通电源、关闭占用显卡的软件，并在 Windows 图形设置中将 brush_app.exe 设为高性能后重试。\nParent device is lost";
    expect(localizePipelineMessage("en", message)).toBe(
      "The GPU device disconnected during Splat training. Connect the computer to power, close other GPU-heavy apps, set brush_app.exe to High performance in Windows Graphics settings, and try again.",
    );
    expect(localizePipelineMessage("zh-CN", message)).toBe(message);
  });

  it("switches immediately, localizes formatting, and persists the explicit choice", async () => {
    window.localStorage.setItem("ooo-splat-language", "zh-CN");
    await act(async () => root.render(<LanguageProvider><Harness /></LanguageProvider>));
    expect(container.textContent).toContain("创建新任务");
    expect(container.textContent).toContain("1 分 2 秒");

    const button = container.querySelector("button")!;
    await act(async () => button.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    expect(container.textContent).toContain("Create New Task");
    expect(container.textContent).toContain("1m 2s");
    expect(window.localStorage.getItem("ooo-splat-language")).toBe("en");
    expect(document.documentElement.lang).toBe("en");
  });
});
