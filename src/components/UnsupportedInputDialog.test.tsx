// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider } from "../i18n";
import type { InputAdmissionReason } from "../lib/inputAdmission";
import { UnsupportedInputDialog } from "./UnsupportedInputDialog";

let container: HTMLDivElement, root: Root, previous: HTMLButtonElement;
const close = vi.fn();
let mounted = false;

async function render(reason: InputAdmissionReason = "colmap_disabled") {
  await act(async () => { root.render(<LanguageProvider><UnsupportedInputDialog reason={reason} onClose={close} /></LanguageProvider>); });
  mounted = true;
}
async function unmount() { if (mounted) { await act(async () => root.unmount()); mounted = false; } }
function buttons() { return [...container.querySelectorAll("button")]; }
async function key(target: HTMLElement, value: string, shiftKey = false) {
  await act(async () => { target.dispatchEvent(new KeyboardEvent("keydown", { key: value, shiftKey, bubbles: true, cancelable: true })); });
}

beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear(); localStorage.setItem("ooo-splat-language", "zh-CN"); close.mockReset();
  previous = document.createElement("button"); previous.textContent = "previous"; document.body.append(previous); previous.focus();
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await unmount(); container.remove(); previous.remove(); });

describe("unsupported input dialog", () => {
  it("uses the confirmed Chinese message, labels the dialog, and initially focuses Close", async () => {
    await render();
    const dialog = container.querySelector<HTMLElement>('[role="alertdialog"]')!;
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(document.getElementById(dialog.getAttribute("aria-labelledby")!)?.textContent).toBe("不支持的输入");
    expect(document.getElementById(dialog.getAttribute("aria-describedby")!)?.textContent).toBe("暂不支持导入 COLMAP 文件或文件夹。");
    expect(document.activeElement).toBe(buttons().at(-1));
    expect(buttons().at(-1)?.textContent).toBe("关闭");
    await act(async () => buttons().at(-1)!.click());
    expect(close).toHaveBeenCalledOnce();
  });

  it("supports the English unsupported-format message", async () => {
    localStorage.setItem("ooo-splat-language", "en");
    await render("unsupported_input");
    expect(container.textContent).toContain("Unsupported input");
    expect(container.textContent).toContain("The selected file or folder is unsupported. Choose a file type supported by this input.");
    expect(buttons().at(-1)?.textContent).toBe("Close");
  });

  it("traps Tab in both directions, closes on Escape, and restores the originating focus", async () => {
    await render();
    const [first, last] = buttons();
    await key(last, "Tab"); expect(document.activeElement).toBe(first);
    await key(first, "Tab", true); expect(document.activeElement).toBe(last);
    await key(last, "Escape"); expect(close).toHaveBeenCalledOnce();
    await unmount(); expect(document.activeElement).toBe(previous);
  });

  it("does not steal focus from a later input", async () => {
    await render();
    const input = document.createElement("input"); document.body.append(input); input.focus();
    await unmount(); expect(document.activeElement).toBe(input);
    input.remove();
  });

  it("does not restore focus across a newly opened modal", async () => {
    await render();
    const next = document.createElement("section"); next.setAttribute("aria-modal", "true"); document.body.append(next);
    await unmount(); expect(document.activeElement).not.toBe(previous);
    next.remove();
  });
});
