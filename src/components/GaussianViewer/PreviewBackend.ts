import { DEVICETYPE_WEBGL2, DEVICETYPE_WEBGPU, GSPLAT_RENDERER_RASTER_CPU_SORT, type Application } from "playcanvas";

export const PREVIEW_DEVICE_TYPES = [DEVICETYPE_WEBGPU, DEVICETYPE_WEBGL2] as const;

export function previewDeviceTypes() {
  return [...PREVIEW_DEVICE_TYPES];
}

export function configurePreviewRenderer(app: Pick<Application, "scene">) {
  // PlayCanvas 2.21 GPU sorting produces incorrect overlap/blending on the
  // 002 model. Keep GPU rasterization, with the verified CPU depth ordering.
  app.scene.gsplat.renderer = GSPLAT_RENDERER_RASTER_CPU_SORT;
}
