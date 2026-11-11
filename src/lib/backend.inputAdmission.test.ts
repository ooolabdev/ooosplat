// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ open: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: vi.fn(), invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open, confirm: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: vi.fn() }));
import { selectMaterialPaths } from "./backend";

beforeEach(() => {
  (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  mocks.open.mockReset(); mocks.invoke.mockReset();
});

describe("new material chooser admission", () => {
  it("only advertises supported video and image formats, not COLMAP files", async () => {
    mocks.open.mockResolvedValue(["E:/clip.mov", "E:/photo.png"]);
    await expect(selectMaterialPaths(false)).resolves.toEqual(["E:/clip.mov", "E:/photo.png"]);
    expect(mocks.open).toHaveBeenCalledOnce();
    expect(mocks.open.mock.calls[0][0]).toMatchObject({ multiple: true, directory: false,
      filters: [{ extensions: ["mp4", "mov", "jpg", "jpeg", "png"] }] });
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("keeps folder selection and cancellation without starting inspection", async () => {
    mocks.open.mockResolvedValue(null);
    await expect(selectMaterialPaths(true)).resolves.toEqual([]);
    expect(mocks.open).toHaveBeenCalledWith({ multiple: true, directory: true });
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("does not open a native chooser outside Tauri", async () => {
    delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    await expect(selectMaterialPaths(false)).resolves.toEqual([]);
    expect(mocks.open).not.toHaveBeenCalled();
  });
});
