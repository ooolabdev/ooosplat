// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TelemetryPreferences } from "./TelemetryPreferences";

describe("TelemetryPreferences", () => {
  const roots: Array<{ root: ReturnType<typeof createRoot>; container: HTMLDivElement }> = [];

  beforeEach(() => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  });

  afterEach(async () => {
    for (const item of roots.splice(0)) {
      await act(async () => item.root.unmount());
      item.container.remove();
    }
  });

  it("keeps the legacy undecided-state notice transparent and dismissible", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    roots.push({ root, container });
    const onChange = vi.fn();
    await act(async () => root.render(<TelemetryPreferences
      mode="consent"
      preferences={{ analyticsEnabled: true, consentDecided: false, deliveryStatus: "notConfigured" }}
      busy={false}
      onChange={onChange}
    />));

    expect(container.textContent).toContain("绝不收集");
    expect(container.textContent).toContain("文件名、路径和项目名称");
    expect(container.textContent).toContain("不会产生遥测网络请求");
    const decline = [...container.querySelectorAll("button")].find((button) => button.textContent === "不用了");
    await act(async () => decline?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onChange).toHaveBeenCalledWith(false);
  });

  it("exposes an accessible settings switch", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    roots.push({ root, container });
    const onChange = vi.fn();
    await act(async () => root.render(<TelemetryPreferences
      mode="settings"
      preferences={{ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" }}
      busy={false}
      onChange={onChange}
      onClose={() => undefined}
    />));

    const toggle = container.querySelector<HTMLButtonElement>('[role="switch"]');
    expect(toggle?.getAttribute("aria-checked")).toBe("true");
    await act(async () => toggle?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onChange).toHaveBeenCalledWith(false);
  });

  it("shows a local settings failure while keeping the switch off and usable", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    roots.push({ root, container });
    const onChange = vi.fn();
    await act(async () => root.render(<TelemetryPreferences
      mode="settings"
      preferences={{ analyticsEnabled: false, consentDecided: true, deliveryStatus: "configured", settingsError: "settings write denied" }}
      busy={false}
      onChange={onChange}
    />));
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("settings write denied");
    const toggle = container.querySelector<HTMLButtonElement>('[role="switch"]')!;
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(toggle.disabled).toBe(false);
    await act(async () => toggle.click());
    expect(onChange).toHaveBeenCalledWith(true);
  });
});
