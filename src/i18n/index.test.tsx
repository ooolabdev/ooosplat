// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { detectSystemLocale, LanguageProvider, readInitialLocale, useI18n } from ".";

function Harness() {
  const { locale, t, toggleLocale, formatDuration } = useI18n();
  return <div>
    <span data-locale={locale}>{t("task.create")}</span>
    <span>{formatDuration(62_000)}</span>
    <button type="button" onClick={toggleLocale}>{t("language.target")}</button>
  </div>;
}

describe("interface language", () => {
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
