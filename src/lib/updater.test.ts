import { afterEach, describe, expect, it, vi } from "vitest";
import type { Update } from "@tauri-apps/plugin-updater";

const check = vi.fn();

vi.mock("@tauri-apps/plugin-updater", () => ({ check }));

const importUpdater = async () => {
  vi.resetModules();
  return import("./updater");
};

type ProgressEvent = { event: "Started"; data: { contentLength: number | null } } | { event: "Progress"; data: { chunkLength: number } };

const fakeUpdate = (events: ProgressEvent[], overrides: Partial<Record<"download" | "install" | "close", unknown>> = {}) => {
  const seen: { downloadOptions?: unknown; installOptions?: unknown } = {};
  const update = {
    download: vi.fn(async (onEvent: (event: ProgressEvent) => void, options?: unknown) => {
      seen.downloadOptions = options;
      for (const event of events) onEvent(event);
    }),
    install: vi.fn(async (options?: unknown) => { seen.installOptions = options; }),
    close: vi.fn(async () => undefined),
    ...overrides,
  };
  return { update: update as unknown as Update, spy: update, seen };
};

describe("updater availability", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("never contacts the feed from a build that is not an official release", async () => {
    vi.stubEnv("VITE_UPDATER_ENABLED", "false");
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    const { checkForAppUpdate, isUpdaterEnabled } = await importUpdater();

    expect(isUpdaterEnabled()).toBe(false);
    await expect(checkForAppUpdate()).resolves.toBeNull();
    expect(check).not.toHaveBeenCalled();
  });

  it("treats a missing build flag as updater disabled", async () => {
    vi.stubEnv("VITE_UPDATER_ENABLED", "");
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    const { checkForAppUpdate, isUpdaterEnabled } = await importUpdater();

    expect(isUpdaterEnabled()).toBe(false);
    await expect(checkForAppUpdate()).resolves.toBeNull();
    expect(check).not.toHaveBeenCalled();
  });

  it("does not contact the feed from a browser development session", async () => {
    vi.stubEnv("VITE_UPDATER_ENABLED", "true");
    vi.stubGlobal("window", {});
    const { checkForAppUpdate } = await importUpdater();

    await expect(checkForAppUpdate()).resolves.toBeNull();
    expect(check).not.toHaveBeenCalled();
  });

  it("checks the signed feed only from an official Tauri build", async () => {
    vi.stubEnv("VITE_UPDATER_ENABLED", "true");
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    check.mockResolvedValue(null);
    const { checkForAppUpdate, isUpdaterEnabled } = await importUpdater();

    expect(isUpdaterEnabled()).toBe(true);
    await expect(checkForAppUpdate()).resolves.toBeNull();
    expect(check).toHaveBeenCalledWith({ timeout: 15_000 });
  });

  it("reports a check failure to the caller", async () => {
    vi.stubEnv("VITE_UPDATER_ENABLED", "true");
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    check.mockRejectedValue(new Error("network unreachable"));
    const { checkForAppUpdate } = await importUpdater();

    await expect(checkForAppUpdate()).rejects.toThrow("network unreachable");
  });

  it("recognizes a binary that was built without the updater feature", async () => {
    const { isUpdaterPluginMissing } = await importUpdater();

    expect(isUpdaterPluginMissing(new Error("Command plugin:updater|check not found"))).toBe(true);
    expect(isUpdaterPluginMissing("updater.not_allowed")).toBe(true);
    expect(isUpdaterPluginMissing(new Error("network unreachable"))).toBe(false);
    expect(isUpdaterPluginMissing(undefined)).toBe(false);
  });
});

describe("update download", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("accumulates progress and installs only when asked", async () => {
    const { update, spy, seen } = fakeUpdate([
      { event: "Started", data: { contentLength: 100 } },
      { event: "Progress", data: { chunkLength: 40 } },
      { event: "Progress", data: { chunkLength: 60 } },
    ]);
    const progress: Array<{ downloadedBytes: number; totalBytes: number | null }> = [];
    const { downloadAppUpdate, installAppUpdate } = await importUpdater();

    await downloadAppUpdate(update, (value) => progress.push(value));

    expect(progress).toEqual([
      { downloadedBytes: 0, totalBytes: 100 },
      { downloadedBytes: 40, totalBytes: 100 },
      { downloadedBytes: 100, totalBytes: 100 },
    ]);
    expect(seen.downloadOptions).toEqual({ timeout: 30 * 60 * 1000 });
    expect(spy.install).not.toHaveBeenCalled();

    await installAppUpdate(update);
    expect(seen.installOptions).toEqual({ restartAfterInstall: true });
  });

  it("reports an unknown total size without inventing a percentage", async () => {
    const { update } = fakeUpdate([{ event: "Started", data: { contentLength: null } }]);
    const progress: Array<{ downloadedBytes: number; totalBytes: number | null }> = [];
    const { downloadAppUpdate } = await importUpdater();

    await downloadAppUpdate(update, (value) => progress.push(value));

    expect(progress).toEqual([{ downloadedBytes: 0, totalBytes: null }]);
  });

  it("never throws while discarding a downloaded package", async () => {
    const { update } = fakeUpdate([], { close: vi.fn(async () => { throw new Error("gone"); }) });
    const { discardAppUpdate } = await importUpdater();

    await expect(discardAppUpdate(update)).resolves.toBeUndefined();
  });
});
