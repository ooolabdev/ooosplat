import { FILLMODE_FILL_WINDOW, RESOLUTION_AUTO, type Application } from "playcanvas";

export function viewerPixelRatio(
  width: number,
  height: number,
  devicePixelRatio: number,
  maxTextureSize: number,
  maxRenderBufferSize?: number,
): number {
  const requested = Number.isFinite(devicePixelRatio) && devicePixelRatio > 0 ? devicePixelRatio : 1;
  const limits = [maxTextureSize, maxRenderBufferSize].filter(
    (value): value is number => typeof value === "number" && Number.isFinite(value) && value > 0,
  );
  const limit = Math.min(...limits);
  return Math.min(requested, limit / Math.max(1, width), limit / Math.max(1, height));
}

/** Keep the drawing buffer in physical pixels, not the default fixed 300 x 150 canvas. */
export function configureViewerResolution(app: Application): () => void {
  let pixelRatioQuery: MediaQueryList | undefined;
  const device = app.graphicsDevice;
  const bufferLimit = (device as typeof device & { maxRenderBufferSize?: number }).maxRenderBufferSize;
  const resize = () => {
    const width = Math.max(1, window.innerWidth);
    const height = Math.max(1, window.innerHeight);
    device.maxPixelRatio = viewerPixelRatio(width, height, window.devicePixelRatio, device.maxTextureSize, bufferLimit);
    app.setCanvasResolution(RESOLUTION_AUTO, width, height);
    app.resizeCanvas(width, height);
    app.renderNextFrame = true;
  };
  const watchPixelRatio = () => {
    pixelRatioQuery?.removeEventListener("change", onPixelRatioChange);
    pixelRatioQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    pixelRatioQuery.addEventListener("change", onPixelRatioChange);
  };
  const onPixelRatioChange = () => {
    resize();
    watchPixelRatio();
  };

  app.setCanvasFillMode(FILLMODE_FILL_WINDOW, window.innerWidth, window.innerHeight);
  resize();
  window.addEventListener("resize", resize);
  watchPixelRatio();

  return () => {
    window.removeEventListener("resize", resize);
    pixelRatioQuery?.removeEventListener("change", onPixelRatioChange);
    pixelRatioQuery = undefined;
  };
}
