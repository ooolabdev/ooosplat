// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { consumeMouseLookEscape, consumeMouseLookPointer } from "./GaussianViewer";

describe("preview mouse-look editor boundaries", () => {
  let root: Root;
  let host: HTMLDivElement;

  beforeEach(() => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
  });
  afterEach(async () => {
    await act(async () => root.unmount());
    host.remove();
  });

  it.each([0, 1, 2])("blocks mouse button %s before rectangle and native gizmo capture handlers", async button => {
    let active = true;
    const navigation = { isMouseLookActive: () => active };
    const rectangle = vi.fn();
    const gizmo = vi.fn();
    await act(async () => root.render(<div onPointerDownCapture={event => {
      if (consumeMouseLookPointer(event, navigation)) return;
      rectangle();
    }}><canvas /></div>));
    const canvas = host.querySelector("canvas")!;
    canvas.addEventListener("pointerdown", gizmo, true);
    const locked = new MouseEvent("pointerdown", { bubbles: true, cancelable: true, button });
    await act(async () => canvas.dispatchEvent(locked));
    expect(locked.defaultPrevented).toBe(true);
    expect(rectangle).not.toHaveBeenCalled();
    expect(gizmo).not.toHaveBeenCalled();

    active = false;
    const unlocked = new MouseEvent("pointerdown", { bubbles: true, cancelable: true, button });
    await act(async () => canvas.dispatchEvent(unlocked));
    expect(unlocked.defaultPrevented).toBe(false);
    expect(rectangle).toHaveBeenCalledOnce();
    expect(gizmo).toHaveBeenCalledOnce();
  });

  it("uses Escape only to stop mouse-look even if releasing the lock is synchronous", async () => {
    let active = true;
    const selection = vi.fn();
    const stopNavigation = vi.fn(() => { active = false; });
    const navigation = { isMouseLookActive: () => active, stopNavigation };
    const capture = (event: KeyboardEvent) => { consumeMouseLookEscape(event, navigation); };
    window.addEventListener("keydown", capture, true);
    window.addEventListener("keydown", selection);
    try {
      const locked = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
      window.dispatchEvent(locked);
      expect(locked.defaultPrevented).toBe(true);
      expect(stopNavigation).toHaveBeenCalledOnce();
      expect(selection).not.toHaveBeenCalled();
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      expect(selection).toHaveBeenCalledOnce();
    } finally {
      window.removeEventListener("keydown", capture, true);
      window.removeEventListener("keydown", selection);
    }
  });

  it("does not consume ordinary editor keys or events with no active scene", () => {
    const stopNavigation = vi.fn();
    const navigation = { isMouseLookActive: () => true, stopNavigation };
    const key = new KeyboardEvent("keydown", { key: "Delete", cancelable: true });
    expect(consumeMouseLookEscape(key, navigation)).toBe(false);
    expect(key.defaultPrevented).toBe(false);
    expect(stopNavigation).not.toHaveBeenCalled();
    const pointer = { preventDefault: vi.fn(), stopPropagation: vi.fn() };
    expect(consumeMouseLookPointer(pointer, null)).toBe(false);
    expect(pointer.preventDefault).not.toHaveBeenCalled();
  });
});
