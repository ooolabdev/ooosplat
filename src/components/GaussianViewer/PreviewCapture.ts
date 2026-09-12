import { getCurrentLocale, translate } from "../../i18n";

export interface NormalizedCaptureRegion {
  x: number;
  y: number;
  width: number;
  height: number;
}

const clamp = (value: number, minimum: number, maximum: number) =>
  Math.min(maximum, Math.max(minimum, value));

export function normalizedCaptureRegion(
  canvas: Pick<DOMRect, "left" | "top" | "width" | "height">,
  guide: Pick<DOMRect, "left" | "top" | "width" | "height">,
): NormalizedCaptureRegion {
  if (canvas.width <= 0 || canvas.height <= 0 || guide.width <= 0 || guide.height <= 0) {
    throw new Error(translate(getCurrentLocale(), "viewer.captureSize"));
  }

  const left = clamp(guide.left, canvas.left, canvas.left + canvas.width);
  const top = clamp(guide.top, canvas.top, canvas.top + canvas.height);
  const right = clamp(guide.left + guide.width, canvas.left, canvas.left + canvas.width);
  const bottom = clamp(guide.top + guide.height, canvas.top, canvas.top + canvas.height);
  if (right <= left || bottom <= top) {
    throw new Error(translate(getCurrentLocale(), "viewer.captureOutside"));
  }

  return {
    x: (left - canvas.left) / canvas.width,
    y: (top - canvas.top) / canvas.height,
    width: (right - left) / canvas.width,
    height: (bottom - top) / canvas.height,
  };
}

export function verticalFovForCapture(sourceFovDegrees: number, regionHeight: number) {
  if (
    !Number.isFinite(sourceFovDegrees)
    || sourceFovDegrees <= 0
    || sourceFovDegrees >= 180
    || !Number.isFinite(regionHeight)
    || regionHeight <= 0
    || regionHeight > 1
  ) {
    throw new Error(translate(getCurrentLocale(), "viewer.captureFov"));
  }
  const sourceHalfFov = sourceFovDegrees * Math.PI / 360;
  return Math.atan(Math.tan(sourceHalfFov) * regionHeight) * 360 / Math.PI;
}

export function copyFlippedRgbaRows(
  source: Uint8Array | Uint8ClampedArray,
  target: Uint8Array | Uint8ClampedArray,
  width: number,
  height: number,
) {
  const rowBytes = width * 4;
  const requiredBytes = rowBytes * height;
  if (source.byteLength < requiredBytes || target.byteLength < requiredBytes) {
    throw new Error(translate(getCurrentLocale(), "viewer.framePixels"));
  }
  for (let sourceRow = 0; sourceRow < height; sourceRow += 1) {
    const sourceOffset = sourceRow * rowBytes;
    const targetOffset = (height - sourceRow - 1) * rowBytes;
    target.set(source.subarray(sourceOffset, sourceOffset + rowBytes), targetOffset);
  }
}
