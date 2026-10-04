// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider } from "../i18n";
import { SettingsDialog } from "./SettingsDialog";

describe("SettingsDialog", () => {
  const mounted: Array<{ root: ReturnType<typeof createRoot>; container: HTMLDivElement }> = [];
  beforeEach(() => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    window.localStorage.setItem("ooo-splat-language", "zh-CN");
  });
  afterEach(async () => {
    for (const item of mounted.splice(0)) {
      await act(async () => item.root.unmount());
      item.container.remove();
    }
    window.localStorage.clear();
  });

  it("opens on interface settings and toggles runtime monitoring", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    mounted.push({ root, container });
    const onRuntimePanelChange = vi.fn();
    await act(async () => root.render(<LanguageProvider><SettingsDialog
      preferences={{ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" }}
      telemetryBusy={false}
      showRuntimePanel={false}
      onTelemetryChange={() => undefined}
      onRuntimePanelChange={onRuntimePanelChange}
      onClose={() => undefined}
    /></LanguageProvider>));

    const runtimeToggle = container.querySelector<HTMLButtonElement>('[role="switch"]')!;
    expect(container.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toContain("界面");
    expect(runtimeToggle.getAttribute("aria-checked")).toBe("false");
    await act(async () => runtimeToggle.click());
    expect(onRuntimePanelChange).toHaveBeenCalledWith(true);
  });

  it("supports keyboard sidebar navigation and preserves the privacy control", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    mounted.push({ root, container });
    const onTelemetryChange = vi.fn();
    await act(async () => root.render(<LanguageProvider><SettingsDialog
      preferences={{ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" }}
      telemetryBusy={false}
      showRuntimePanel={false}
      onTelemetryChange={onTelemetryChange}
      onRuntimePanelChange={() => undefined}
      onClose={() => undefined}
    /></LanguageProvider>));

    const tabs = container.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    await act(async () => tabs[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })));
    expect(tabs[1].getAttribute("aria-selected")).toBe("true");
    const privacyToggle = container.querySelector<HTMLButtonElement>('[role="switch"]')!;
    await act(async () => privacyToggle.click());
    expect(onTelemetryChange).toHaveBeenCalledWith(false);
  });
});
