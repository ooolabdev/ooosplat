// @vitest-environment jsdom

import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider } from "../../i18n";
import type { GaussianCrop, GaussianTransform } from "../../types/pipeline";
import { SelectionPanel } from "./SelectionPanel";
import { TransformPanel } from "./TransformPanel";

function pointerEvent(type: string, x: number) {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, buttons: type === "pointerup" ? 0 : 1, clientX: x });
  Object.defineProperty(event, "pointerId", { value: 9 });
  return event;
}

describe("TransformPanel", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    window.localStorage.setItem("ooo-splat-language", "zh-CN");
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    window.localStorage.clear();
  });

  it("scrubs a numeric value with a left-button horizontal drag", async () => {
    const begin = vi.fn();
    const commit = vi.fn();
    function Harness() {
      const [transform, setTransform] = useState<GaussianTransform>({ position: [0, 0, 0], rotation: [0, 0, 0], scale: 1 });
      return <TransformPanel transform={transform} onBegin={begin} onChange={setTransform} onCommit={commit} />;
    }
    await act(async () => root.render(<Harness />));

    const scrubber = container.querySelector<HTMLButtonElement>('[aria-label="位置 X拖动调整"]');
    await act(async () => {
      scrubber?.dispatchEvent(pointerEvent("pointerdown", 100));
      scrubber?.dispatchEvent(pointerEvent("pointermove", 120));
      scrubber?.dispatchEvent(pointerEvent("pointerup", 120));
    });

    expect(begin).toHaveBeenCalledTimes(1);
    expect(commit).toHaveBeenCalledTimes(1);
    expect(container.querySelector<HTMLInputElement>('[aria-label="位置 X"]')?.value).toBe("0.2");
  });

  it("reserves a wider label column for English scale controls", async () => {
    window.localStorage.setItem("ooo-splat-language", "en");
    const transform: GaussianTransform = { position: [0, 0, 0], rotation: [0, 0, 0], scale: 1 };
    await act(async () => root.render(<LanguageProvider><TransformPanel transform={transform} onBegin={() => {}} onChange={() => {}} onCommit={() => {}} /></LanguageProvider>));

    const uniform = Array.from(container.querySelectorAll<HTMLButtonElement>(".transform-scrubber")).find((button) => button.textContent === "Uniform");
    expect(uniform?.classList.contains("long-label")).toBe(true);
    expect(uniform?.closest(".transform-field")?.classList.contains("long-label")).toBe(true);
  });

  it("allows a uniform scale of 10000 and rejects values above it", async () => {
    window.localStorage.setItem("ooo-splat-language", "en");
    function Harness() {
      const [transform, setTransform] = useState<GaussianTransform>({ position: [0, 0, 0], rotation: [0, 0, 0], scale: 1 });
      return <LanguageProvider><TransformPanel transform={transform} onBegin={() => {}} onChange={setTransform} onCommit={() => {}} /></LanguageProvider>;
    }
    await act(async () => root.render(<Harness />));

    const input = container.querySelector<HTMLInputElement>(".transform-panel section:last-of-type input")!;
    await act(async () => { input.focus(); });
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, "10000");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => { input.blur(); });
    expect(input.value).toBe("10000");
    expect(input.getAttribute("aria-invalid")).toBe("false");

    await act(async () => { input.focus(); });
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, "10001");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => { input.blur(); });
    expect(input.value).toBe("10000");
    expect(input.getAttribute("aria-invalid")).toBe("true");
  });

  it("scrubs crop extents above 1000 without snapping to the transform scale limit", async () => {
    function Harness() {
      const [crop, setCrop] = useState<GaussianCrop>({ kind: "sphere", center: [0, 0, 0], radius: 1500 });
      return <SelectionPanel crop={crop} kind="sphere" onBegin={() => {}} onChange={setCrop} onCommit={() => {}} onEnable={() => {}} />;
    }
    await act(async () => root.render(<LanguageProvider><Harness /></LanguageProvider>));

    const scrubber = Array.from(container.querySelectorAll<HTMLButtonElement>(".transform-scrubber"))
      .find((button) => button.textContent === "R");
    expect(scrubber).toBeDefined();
    await act(async () => { scrubber?.dispatchEvent(pointerEvent("pointerdown", 100)); });
    await act(async () => { scrubber?.dispatchEvent(pointerEvent("pointermove", 110)); });
    await act(async () => { scrubber?.dispatchEvent(pointerEvent("pointerup", 110)); });

    const value = Number(container.querySelector<HTMLInputElement>('[aria-label="球形半径"]')?.value);
    expect(value).toBeGreaterThan(1500);
    expect(value).toBeLessThan(1e12);
  });

  it("rejects crop extents outside the backend range and restores the last value", async () => {
    const crop: GaussianCrop = { kind: "box", center: [0, 0, 0], size: [1500, 2000, 2500] };
    await act(async () => root.render(<LanguageProvider><SelectionPanel crop={crop} kind="box" onBegin={() => {}} onChange={() => {}} onCommit={() => {}} onEnable={() => {}} /></LanguageProvider>));

    const input = container.querySelector<HTMLInputElement>('[aria-label="盒形尺寸 X"]')!;
    await act(async () => { input.focus(); });
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, "10000000000000");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => { input.blur(); });

    expect(input.value).toBe("1500");
    expect(input.getAttribute("aria-invalid")).toBe("true");
  });
});
