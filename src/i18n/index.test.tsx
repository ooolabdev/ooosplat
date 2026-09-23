// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { detectSystemLocale, LanguageProvider, localizePipelineMessage, readInitialLocale, translate, useI18n } from ".";

function Harness() {
  const { locale, t, toggleLocale, formatDuration } = useI18n();
  return <div>
    <span data-locale={locale}>{t("task.create")}</span>
    <span>{formatDuration(62_000)}</span>
    <button type="button" onClick={toggleLocale}>{t("language.target")}</button>
  </div>;
}

describe("interface language", () => {
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

  it("switches immediately, localizes formatting, and persists the explicit choice", async () => {
    window.localStorage.setItem("ooo-splat-language", "zh-CN");
    await act(async () => root.render(<LanguageProvider><Harness /></LanguageProvider>));
    expect(container.textContent).toContain("01 创建新任务");
    expect(container.textContent).toContain("1 分 2 秒");

    const button = container.querySelector("button")!;
    await act(async () => button.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    expect(container.textContent).toContain("01 Create New Task");
    expect(container.textContent).toContain("1m 2s");
    expect(window.localStorage.getItem("ooo-splat-language")).toBe("en");
    expect(document.documentElement.lang).toBe("en");
  });
});
