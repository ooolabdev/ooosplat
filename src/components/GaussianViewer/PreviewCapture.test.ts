import { describe, expect, it } from "vitest";
import {
  copyFlippedRgbaRows,
  copyRgbaReadbackRows,
  normalizedCaptureRegion,
  verticalFovForCapture,
} from "./PreviewCapture";

describe("preview video capture", () => {
  it("maps the visible portrait guide into normalized canvas coordinates", () => {
    const region = normalizedCaptureRegion(
      { left: 100, top: 50, width: 1200, height: 800 },
      { left: 482.875, top: 64, width: 434.25, height: 772 },
    );
    expect(region).toEqual({ x: 0.3190625, y: 0.0175, width: 0.361875, height: 0.965 });
  });

  it("matches the portrait camera FOV to the visible guide height", () => {
    expect(verticalFovForCapture(52, 1)).toBeCloseTo(52);
    expect(verticalFovForCapture(52, 0.965)).toBeCloseTo(50.36, 1);
    expect(() => verticalFovForCapture(52, 0)).toThrow("相机视野");
  });

  it("flips WebGL RGBA rows through both the direct and shared readback paths", () => {
    const bottomRow = [1, 2, 3, 255, 4, 5, 6, 255];
    const topRow = [7, 8, 9, 255, 10, 11, 12, 255];
    const target = new Uint8ClampedArray(16);
    copyFlippedRgbaRows(new Uint8Array([...bottomRow, ...topRow]), target, 2, 2);
    expect([...target]).toEqual([...topRow, ...bottomRow]);
    target.fill(0);
    copyRgbaReadbackRows(new Uint8Array([...bottomRow, ...topRow]), target, 2, 2, true);
    expect([...target]).toEqual([...topRow, ...bottomRow]);
  });

  it("keeps WebGPU top-down RGBA rows in their original order", () => {
    const topRow = [1, 2, 3, 255, 4, 5, 6, 255];
    const bottomRow = [7, 8, 9, 255, 10, 11, 12, 255];
    const target = new Uint8ClampedArray(16);
    copyRgbaReadbackRows(new Uint8Array([...topRow, ...bottomRow]), target, 2, 2, false);
    expect([...target]).toEqual([...topRow, ...bottomRow]);
  });
});
