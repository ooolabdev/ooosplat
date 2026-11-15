// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider } from "../i18n";
import type { GpuInventory } from "../types/pipeline";
import { GpuSelectionDialog } from "./GpuSelectionDialog";

const inventory: GpuInventory = {
  recommendedDeviceId: "nvidia",
  devices: [
    {
      localDeviceId: "nvidia", name: "NVIDIA RTX", vendor: "NVIDIA", deviceKind: "discreteGpu",
      adapterIndex: 0, totalMemoryMb: 8192, driver: "1", colmapCompatible: true,
      colmapGpuIndex: 0, recommended: true, selectionConfidence: "uniqueModel", duplicateModel: false,
    },
    {
      localDeviceId: "amd", name: "AMD Radeon", vendor: "AMD", deviceKind: "integratedGpu",
      adapterIndex: 0, totalMemoryMb: null, driver: null, colmapCompatible: false,
      colmapGpuIndex: null, recommended: false, selectionConfidence: "uniqueModel", duplicateModel: false,
    },
  ],
};

describe("GpuSelectionDialog", () => {
  const mounted: Array<{ root: ReturnType<typeof createRoot>; container: HTMLDivElement }> = [];
  beforeEach(() => localStorage.setItem("ooo-splat-language", "en"));
  afterEach(async () => {
    for (const item of mounted.splice(0)) {
      await act(async () => item.root.unmount());
      item.container.remove();
    }
    localStorage.clear();
  });

  it("preselects the recommendation and returns one concrete device", async () => {
    const container = document.createElement("div"); document.body.appendChild(container);
    const root = createRoot(container); mounted.push({ root, container });
    const finish = vi.fn();
    await act(async () => root.render(<LanguageProvider><GpuSelectionDialog inventory={inventory} onFinish={finish} /></LanguageProvider>));
    const radios = container.querySelectorAll<HTMLInputElement>('input[type="radio"]');
    expect(radios[0].checked).toBe(true);
    expect(container.textContent).toContain("Training uses this device; reconstruction uses the CPU.");
    await act(async () => { radios[1].click(); container.querySelector<HTMLInputElement>('.gpu-remember input')!.click(); });
    await act(async () => container.querySelector<HTMLButtonElement>('.primary-action')!.click());
    expect(finish).toHaveBeenCalledWith({
      selection: { localDeviceId: "amd", deviceKind: "integratedGpu", selectionConfidence: "uniqueModel" },
      remember: true,
    });
  });

  it("focuses cancel and closes on Escape", async () => {
    const container = document.createElement("div"); document.body.appendChild(container);
    const root = createRoot(container); mounted.push({ root, container });
    const finish = vi.fn();
    await act(async () => root.render(<LanguageProvider><GpuSelectionDialog inventory={inventory} onFinish={finish} /></LanguageProvider>));
    expect(document.activeElement?.textContent).toBe("Cancel");
    await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
    expect(finish).toHaveBeenCalledWith(null);
  });
});
