// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider, useI18n } from "../i18n";
import { CompactError } from "./CompactError";

let container: HTMLDivElement;
let root: Root;
const detail = "Brush: IO error while loading dataset: early eof\n" + "a long detail line\n".repeat(200);
const tooltip = () => document.querySelector<HTMLElement>("[role=tooltip]");
const anchor = () => container.querySelector<HTMLElement>(".compact-error")!;
function Toggle() { const { toggleLocale } = useI18n(); return <button onClick={toggleLocale}>switch</button>; }
beforeEach(async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.setItem("ooo-splat-language", "zh-CN");
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
  await act(async () => root.render(<LanguageProvider><Toggle /><CompactError message={detail} /></LanguageProvider>));
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); localStorage.clear(); vi.useRealTimers(); });

describe("compact error hover details", () => {
  it("keeps a short summary in the layout and reveals complete, copyable details on hover", async () => {
    expect(anchor().textContent).toBe("生成所需图片读取不完整"); expect(tooltip()).toBeNull();
    await act(async () => anchor().dispatchEvent(new MouseEvent("mouseover", { bubbles: true })));
    expect(tooltip()?.textContent).toBe(detail);
    expect(container.contains(tooltip())).toBe(false);
    expect(anchor().getAttribute("aria-describedby")).toBe(tooltip()?.id);
    expect(Number.parseInt(tooltip()!.style.maxHeight)).toBeLessThanOrEqual(320);
  });
  it("shows details with keyboard focus and dismisses them with Esc", async () => {
    await act(async () => anchor().focus()); expect(tooltip()?.textContent).toBe(detail);
    await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
    expect(tooltip()).toBeNull();
  });
  it("lets the pointer enter the scrollable tooltip without closing it", async () => {
    vi.useFakeTimers();
    await act(async () => anchor().dispatchEvent(new MouseEvent("mouseover", { bubbles: true })));
    await act(async () => anchor().dispatchEvent(new MouseEvent("mouseout", { bubbles: true })));
    await act(async () => tooltip()!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })));
    await act(async () => vi.advanceTimersByTime(200)); expect(tooltip()).not.toBeNull();
    await act(async () => tooltip()!.dispatchEvent(new Event("scroll"))); expect(tooltip()).not.toBeNull();
    await act(async () => tooltip()!.dispatchEvent(new MouseEvent("mouseout", { bubbles: true })));
    await act(async () => vi.advanceTimersByTime(200)); expect(tooltip()).toBeNull();
  });
  it("translates summaries without changing the original detail", async () => {
    await act(async () => container.querySelector<HTMLButtonElement>("button")!.click());
    expect(anchor().textContent).toBe("Some required images could not be read");
    await act(async () => anchor().focus()); expect(tooltip()?.textContent).toBe(detail);
    await act(async () => window.dispatchEvent(new Event("resize"))); expect(tooltip()).toBeNull();
  });
});
