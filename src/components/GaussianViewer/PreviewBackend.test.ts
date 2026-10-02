import { describe, expect, it } from "vitest";
import { DEVICETYPE_WEBGL2, DEVICETYPE_WEBGPU } from "playcanvas";
import { PREVIEW_DEVICE_TYPES, previewDeviceTypes } from "./PreviewBackend";

describe("preview graphics backend", () => {
  it("provides WebGPU-first preferences in a fresh array for each application mount", () => {
    expect(PREVIEW_DEVICE_TYPES).toEqual([DEVICETYPE_WEBGPU, DEVICETYPE_WEBGL2]);
    expect(previewDeviceTypes()).toEqual([DEVICETYPE_WEBGPU, DEVICETYPE_WEBGL2]);
    expect(previewDeviceTypes()).not.toBe(previewDeviceTypes());
  });
});
