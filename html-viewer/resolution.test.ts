// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import { FILLMODE_FILL_WINDOW, RESOLUTION_AUTO, type Application } from "playcanvas";
import { configureViewerResolution, viewerPixelRatio } from "./resolution";

afterEach(() => vi.restoreAllMocks());

function viewport(width: number, height: number, ratio: number) {
  vi.spyOn(window, "innerWidth", "get").mockReturnValue(width);
  vi.spyOn(window, "innerHeight", "get").mockReturnValue(height);
  vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(ratio);
}

function setup(isWebGPU: boolean) {
  const canvas = document.createElement("canvas");
  let automatic = false;
  const device = { canvas, isWebGPU, maxPixelRatio: 1, maxTextureSize: 8192, maxRenderBufferSize: isWebGPU ? undefined : 4096 };
  const update = (width: number, height: number) => {
    canvas.width = Math.floor(width * Math.min(device.maxPixelRatio, window.devicePixelRatio));
    canvas.height = Math.floor(height * Math.min(device.maxPixelRatio, window.devicePixelRatio));
  };
  const app = {
    graphicsDevice: device,
    renderNextFrame: false,
    setCanvasFillMode: vi.fn(),
    setCanvasResolution: vi.fn((mode, width, height) => { automatic = mode === RESOLUTION_AUTO; update(width, height); }),
    resizeCanvas: vi.fn((width, height) => {
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
      if (automatic) update(width, height);
    }),
  };
  const queries: Array<{ query: string; listener?: () => void; removeEventListener: ReturnType<typeof vi.fn> }> = [];
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn((query: string) => {
      const result = {
        query,
        listener: undefined as (() => void) | undefined,
        addEventListener: vi.fn((_event: string, listener: () => void) => { result.listener = listener; }),
        removeEventListener: vi.fn(() => { result.listener = undefined; }),
      };
      queries.push(result);
      return result;
    }),
  });
  return { app, canvas, queries, configure: () => configureViewerResolution(app as unknown as Application) };
}

describe.each([true, false])("offline viewer resolution (WebGPU: %s)", (isWebGPU) => {
  it.each([1, 1.25, 1.5, 2])("uses physical viewport pixels at %sx pixel density", (ratio) => {
    viewport(1200, 800, ratio);
    const { app, canvas, configure } = setup(isWebGPU);
    expect([canvas.width, canvas.height]).toEqual([300, 150]);
    const dispose = configure();
    expect(app.setCanvasFillMode).toHaveBeenCalledWith(FILLMODE_FILL_WINDOW, 1200, 800);
    expect(app.setCanvasResolution).toHaveBeenCalledWith(RESOLUTION_AUTO, 1200, 800);
    expect([canvas.width, canvas.height]).toEqual([1200 * ratio, 800 * ratio]);
    expect([canvas.style.width, canvas.style.height]).toEqual(["1200px", "800px"]);
    expect(app.renderNextFrame).toBe(true);
    dispose();
  });

  it("updates both window proportions and pixel density without reloading the scene", () => {
    viewport(1200, 800, 1);
    const { app, canvas, queries, configure } = setup(isWebGPU);
    const dispose = configure();
    viewport(700, 1100, 1.5);
    window.dispatchEvent(new Event("resize"));
    expect([canvas.width, canvas.height]).toEqual([1050, 1650]);
    expect(app.setCanvasFillMode).toHaveBeenCalledTimes(1);
    viewport(700, 1100, 2);
    queries[0].listener!();
    expect([canvas.width, canvas.height]).toEqual([1400, 2200]);
    expect(queries[0].removeEventListener).toHaveBeenCalledOnce();
    expect(queries[1].query).toBe("(resolution: 2dppx)");
    dispose();
    dispose();
    app.resizeCanvas.mockClear();
    window.dispatchEvent(new Event("resize"));
    expect(app.resizeCanvas).not.toHaveBeenCalled();
    expect(queries[1].listener).toBeUndefined();
  });

  it("clamps the first allocation and later resizes to the GPU capacity", () => {
    viewport(10000, 5000, 2);
    const { app, canvas, configure } = setup(isWebGPU);
    const dispose = configure();
    const limit = isWebGPU ? 8192 : 4096;
    expect([canvas.width, canvas.height]).toEqual([limit, limit / 2]);
    expect(app.graphicsDevice.maxPixelRatio).toBe(limit / 10000);
    dispose();
  });
});

describe("viewerPixelRatio", () => {
  it("preserves native density unless a hardware dimension limit is exceeded", () => {
    expect(viewerPixelRatio(1920, 1080, 2, 8192)).toBe(2);
    expect(viewerPixelRatio(2000, 4000, 2, 8192, 4096)).toBe(1.024);
    expect(viewerPixelRatio(800, 600, NaN, 8192)).toBe(1);
  });
});
