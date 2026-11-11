// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { LanguageProvider } from "../i18n";
import type { GaussianImportProbe } from "../types/pipeline";
import { GaussianImportDialog } from "./GaussianImportDialog";

const mocks = vi.hoisted(() => ({
  probe: vi.fn(), begin: vi.fn(), complete: vi.fn(), cancel: vi.fn(), convert: vi.fn(),
}));
vi.mock("../lib/backend", () => ({
  probeGaussianImport: mocks.probe,
  beginGaussianImport: mocks.begin,
  completeGaussianImport: mocks.complete,
  cancelGaussianImport: mocks.cancel,
}));
vi.mock("../lib/gaussianImport", () => ({ convertGaussianImport: mocks.convert }));

const probe: GaussianImportProbe = {
  probeId: "probe", suggestedName: "Model", defaultModelId: "model", defaultColmapId: "legacy-sidecar",
  models: [{ id: "model", name: "model.ply", format: "ply", fileSize: 2048, directPly: true, splatCount: 10 }],
  colmapModels: [{ id: "legacy-sidecar", label: "COLMAP", registeredImages: 2, points3d: 10, cameraCount: 1, reshootEligible: true, reason: null }],
};
const rejected = vi.fn(), closed = vi.fn(), imported = vi.fn();
let root: Root, container: HTMLDivElement;
const deferred = <T,>() => {
  let resolve!: (value: T) => void, reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const unsupported = (reason: "colmap_disabled" | "unsupported_input") => ({ code: "UNSUPPORTED_INPUT", reason, message: "Admission rejected" });
async function render(selectionId = "selection") {
  await act(async () => root.render(<LanguageProvider><GaussianImportDialog selectionId={selectionId} onClose={closed} onImported={imported} onUnsupportedInput={rejected} /></LanguageProvider>));
}
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem("ooo-splat-language", "zh-CN");
  vi.resetAllMocks();
  mocks.probe.mockResolvedValue(probe);
  mocks.begin.mockResolvedValue({ importId: "import", sourceName: "model.ply" });
  mocks.complete.mockResolvedValue({ id: "project" });
  mocks.cancel.mockResolvedValue(undefined);
  mocks.convert.mockReturnValue({ cancel: vi.fn(), result: Promise.resolve({ splatCount: 10, shBands: 3, model: null }) });
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); });

it("keeps the Gaussian model fields but removes new COLMAP association", async () => {
  await render();
  expect(container.querySelectorAll("select")).toHaveLength(1);
  expect(container.textContent).toContain("model.ply");
  expect(container.textContent).toContain("2 KiB");
  expect(container.textContent).not.toContain("COLMAP");
  expect(container.querySelectorAll(".import-summary-grid article")).toHaveLength(1);
  await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")!.click());
  expect(mocks.begin).toHaveBeenCalledWith({ probeId: "probe", modelId: "model", colmapId: null, projectName: "Model" });
  expect(imported).toHaveBeenCalledWith({ id: "project" });
  expect(rejected).not.toHaveBeenCalled();
});

it.each(["colmap_disabled", "unsupported_input"] as const)("forwards typed probe rejection to the shared popup (%s)", async reason => {
  mocks.probe.mockRejectedValue(unsupported(reason));
  await render();
  expect(rejected).toHaveBeenCalledExactlyOnceWith(reason);
  expect(mocks.begin).not.toHaveBeenCalled();
  expect(container.querySelector(".import-error")).toBeNull();
});

it("preserves ordinary file errors rather than calling them unsupported", async () => {
  mocks.probe.mockRejectedValue(new Error("Access denied: OS error 5"));
  await render();
  expect(container.querySelector(".import-error")?.textContent).toContain("OS error 5");
  expect(rejected).not.toHaveBeenCalled();
});

it("ignores a late rejection from a previously selected source", async () => {
  const previous = deferred<GaussianImportProbe>();
  mocks.probe.mockReturnValueOnce(previous.promise).mockResolvedValue(probe);
  await render("previous");
  await render("current");
  await act(async () => previous.reject(unsupported("colmap_disabled")));
  expect(rejected).not.toHaveBeenCalled();
  expect(container.textContent).toContain("model.ply");
});

it("ignores a probe rejection after the dialog is closed", async () => {
  const pending = deferred<GaussianImportProbe>();
  mocks.probe.mockReturnValue(pending.promise);
  await render();
  await act(async () => container.querySelector<HTMLButtonElement>("footer button")!.click());
  await act(async () => pending.reject(unsupported("unsupported_input")));
  expect(closed).toHaveBeenCalledTimes(1);
  expect(rejected).not.toHaveBeenCalled();
});

it("cancels an active session before forwarding a typed execution rejection", async () => {
  mocks.convert.mockImplementation(() => ({ cancel: vi.fn(), result: Promise.reject(unsupported("colmap_disabled")) }));
  await render();
  await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")!.click());
  expect(mocks.cancel).toHaveBeenCalledExactlyOnceWith("import");
  expect(rejected).toHaveBeenCalledExactlyOnceWith("colmap_disabled");
  expect(mocks.cancel.mock.invocationCallOrder[0]).toBeLessThan(rejected.mock.invocationCallOrder[0]);
  expect(mocks.probe).toHaveBeenCalledTimes(1);
  expect(mocks.complete).not.toHaveBeenCalled();
});

it("cleans up an import session returned after cancellation without starting conversion", async () => {
  const pending = deferred<{ importId: string; sourceName: string }>();
  mocks.begin.mockReturnValue(pending.promise);
  await render();
  await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")!.click());
  await act(async () => container.querySelector<HTMLButtonElement>("footer button")!.click());
  await act(async () => pending.resolve({ importId: "late", sourceName: "model.ply" }));
  expect(mocks.cancel).toHaveBeenCalledExactlyOnceWith("late");
  expect(mocks.convert).not.toHaveBeenCalled();
  expect(mocks.complete).not.toHaveBeenCalled();
  expect(imported).not.toHaveBeenCalled();
});

it("preserves a normal conversion error and allows another import attempt", async () => {
  mocks.convert.mockImplementation(() => ({ cancel: vi.fn(), result: Promise.reject(new Error("Invalid Gaussian payload")) }));
  await render();
  await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")!.click());
  expect(container.querySelector(".import-error")?.textContent).toContain("Invalid Gaussian payload");
  expect(mocks.probe).toHaveBeenCalledTimes(2);
  expect(container.querySelector<HTMLButtonElement>(".primary-action")?.disabled).toBe(false);
  expect(rejected).not.toHaveBeenCalled();
});

it("forwards a typed rejection from error recovery instead of stacking another dialog", async () => {
  mocks.convert.mockImplementation(() => ({ cancel: vi.fn(), result: Promise.reject(new Error("Source changed")) }));
  await render();
  mocks.probe.mockRejectedValueOnce(unsupported("unsupported_input"));
  await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")!.click());
  expect(mocks.cancel).toHaveBeenCalledExactlyOnceWith("import");
  expect(rejected).toHaveBeenCalledExactlyOnceWith("unsupported_input");
  expect(mocks.complete).not.toHaveBeenCalled();
});

it("starts a Gaussian import only once when confirm is clicked repeatedly", async () => {
  await render();
  await act(async () => {
    const button = container.querySelector<HTMLButtonElement>(".primary-action")!;
    button.click(); button.click();
  });
  expect(mocks.begin).toHaveBeenCalledTimes(1);
  expect(mocks.convert).toHaveBeenCalledTimes(1);
  expect(mocks.complete).toHaveBeenCalledTimes(1);
  expect(imported).toHaveBeenCalledTimes(1);
});
