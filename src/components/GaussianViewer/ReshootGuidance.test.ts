import { describe, expect, it } from "vitest";
import {
  findReshootRegion,
  GUIDE_ARROW_MARGIN,
  GUIDE_IMAGE_SIZE,
  guideCaption,
  guideMarkerInViewport,
  guideMarkerRadius,
  guideViewport,
  guideViewportPoint,
  isSameReshootRegion,
  regionGeometryLabel,
  regionKey,
  reshootGuideArrows,
  reshootGuidance,
  reshootRegionsGuidance,
  shootingDirections,
  type ReshootRegion,
} from "./ReshootGuidance";

const sphere = (center: [number, number, number], radius = 0.5): ReshootRegion => ({ kind: "sphere", center, radius });
const box = (center: [number, number, number], size: [number, number, number] = [1, 1, 1]): ReshootRegion =>
  ({ kind: "box", center, size });

describe("reshoot region identity", () => {
  it("treats an identical selection as the same region", () => {
    expect(isSameReshootRegion(sphere([1, 2, 3]), sphere([1, 2, 3]))).toBe(true);
    expect(regionKey(sphere([1, 2, 3]))).toBe(regionKey(sphere([1, 2, 3])));
  });

  it("ignores sub-millimetre jitter so a repeated click is still a duplicate", () => {
    expect(isSameReshootRegion(sphere([1, 2, 3], 0.5), sphere([1.0004, 2, 3], 0.5002))).toBe(true);
  });

  it("separates regions that differ in position, size, or kind", () => {
    expect(isSameReshootRegion(sphere([1, 2, 3]), sphere([1, 2, 3.5]))).toBe(false);
    expect(isSameReshootRegion(sphere([1, 2, 3], 0.5), sphere([1, 2, 3], 0.9))).toBe(false);
    expect(isSameReshootRegion(sphere([1, 2, 3]), box([1, 2, 3]))).toBe(false);
    expect(isSameReshootRegion(box([0, 0, 0], [1, 2, 3]), box([0, 0, 0], [1, 2, 4]))).toBe(false);
  });

  it("finds an existing duplicate before it is added again", () => {
    const regions = [sphere([0, 0, 0]), box([4, 0, 1])];
    expect(findReshootRegion(regions, box([4, 0, 1]))).toBe(1);
    expect(findReshootRegion(regions, sphere([0, 0, 0]))).toBe(0);
    expect(findReshootRegion(regions, sphere([9, 9, 9]))).toBe(-1);
  });
});

describe("reshoot guidance text", () => {
  it("describes the selection geometry so the shooter knows which spot it means", () => {
    expect(regionGeometryLabel(sphere([1.5, -2, 0.25], 0.75))).toBe("球选 · 中心 (1.50, -2.00, 0.25) · 半径 0.75");
    expect(regionGeometryLabel(box([0, 0, 0], [2, 3, 4]))).toBe("盒选 · 中心 (0.00, 0.00, 0.00) · 尺寸 (2.00, 3.00, 4.00)");
  });

  it("never repeats the same wording for two different regions", () => {
    const regions = [sphere([0, 0, 0]), sphere([0, 0, 0.4]), sphere([1, 0, 0])];
    const guidance = reshootRegionsGuidance(regions);

    expect(new Set(guidance).size).toBe(regions.length);
    expect(guidance[0]).toContain("区域 1");
    expect(guidance[1]).toContain("区域 2");
    expect(guidance[2]).toContain("区域 3");
    expect(guidance[0]).toContain("中心 (0.00, 0.00, 0.00)");
    expect(guidance[1]).toContain("中心 (0.00, 0.00, 0.40)");
  });

  it("differs between a sphere and a box selection at the same spot", () => {
    expect(reshootGuidance(sphere([0, 0, 0]), 0)).not.toBe(reshootGuidance(box([0, 0, 0]), 0));
    expect(reshootGuidance(box([0, 0, 0]), 0)).toContain("正面、左侧、右侧与上方");
  });
});

describe("reshoot shooting directions", () => {
  it("gives a sphere region two rings of positions", () => {
    const directions = shootingDirections(sphere([0, 0, 0]));
    expect(directions.length).toBe(12);
    expect(directions.filter((item) => item.label.includes("抬高 30°"))).toHaveLength(4);
  });

  it("gives a box region the documented sides", () => {
    expect(shootingDirections(box([0, 0, 0])).map((item) => item.label)).toEqual(["正面", "左侧", "右侧", "右前 30°", "左前 30°"]);
  });
});

describe("reshoot guide layout", () => {
  it("points every arrow at the circled region from its shooting position", () => {
    const marker = { x: 400, y: 300, radius: 80 };
    const arrows = reshootGuideArrows(marker, shootingDirections(sphere([0, 0, 0])));

    expect(arrows).toHaveLength(12);
    for (const arrow of arrows) {
      const tipDistance = Math.hypot(arrow.toX - marker.x, arrow.toY - marker.y);
      const tailDistance = Math.hypot(arrow.fromX - marker.x, arrow.fromY - marker.y);
      expect(tipDistance).toBeCloseTo(Math.max(marker.radius + 18, 42), 6);
      expect(tailDistance).toBeGreaterThan(tipDistance);
      expect(arrow.label).not.toBe("");
    }
  });

  it("keeps arrows outside the highlight even for a tiny marker", () => {
    const arrows = reshootGuideArrows({ x: 0, y: 0, radius: 1 }, shootingDirections(box([0, 0, 0])));
    expect(arrows.every((arrow) => Math.hypot(arrow.toX, arrow.toY) >= 42)).toBe(true);
  });

  it("scales the highlight with the selection and stays within a usable range", () => {
    expect(guideMarkerRadius(sphere([0, 0, 0], 0.5), 100)).toBe(50);
    expect(guideMarkerRadius(sphere([0, 0, 0], 0.5), 0.001)).toBe(24);
    expect(guideMarkerRadius(box([0, 0, 0], [2, 4, 1]), 1000)).toBe(420);
  });

  it("captions the guide image with the region and what the arrows mean", () => {
    expect(guideCaption(box([0, 0, 0]), 2)).toBe("区域 3 · 盒选 · 箭头 = 补拍机位（箭头指向被补拍区域）");
  });
});

describe("reshoot guide crop", () => {
  const frame = { width: 1600, height: 900 };

  it("magnifies a small selection so it is not a speck in the screenshot", () => {
    const viewport = guideViewport(frame, { x: 800, y: 450, radius: 20 });

    expect(viewport.sourceSize).toBeLessThanOrEqual(Math.min(frame.width, frame.height) * 0.35 + 1e-6);
    expect(viewport.scale).toBeGreaterThan(2);
    expect(viewport.outputSize).toBe(GUIDE_IMAGE_SIZE);
    expect(guideMarkerInViewport(viewport, { x: 800, y: 450, radius: 20 }).radius).toBeGreaterThanOrEqual(GUIDE_IMAGE_SIZE * 0.08);
  });

  it("widens the crop until it fits the arrows of a larger selection", () => {
    const marker = { x: 800, y: 450, radius: 200 };
    const viewport = guideViewport(frame, marker);

    expect(viewport.sourceSize).toBe((marker.radius + GUIDE_ARROW_MARGIN) * 2);
    expect(viewport.scale).toBeLessThan(2);
  });

  it("stops at the frame instead of cropping past its edge", () => {
    const viewport = guideViewport(frame, { x: 800, y: 450, radius: 400 });

    expect(viewport.sourceSize).toBe(Math.min(frame.width, frame.height));
    expect(viewport.sourceX).toBeGreaterThanOrEqual(0);
    expect(viewport.sourceX + viewport.sourceSize).toBeLessThanOrEqual(frame.width);
  });

  it("never crops outside the captured frame", () => {
    for (const marker of [
      { x: 0, y: 0, radius: 30 },
      { x: 1600, y: 900, radius: 30 },
      { x: -500, y: 1200, radius: 60 },
    ]) {
      const viewport = guideViewport(frame, marker);
      expect(viewport.sourceX).toBeGreaterThanOrEqual(0);
      expect(viewport.sourceY).toBeGreaterThanOrEqual(0);
      expect(viewport.sourceX + viewport.sourceSize).toBeLessThanOrEqual(frame.width + 1e-6);
      expect(viewport.sourceY + viewport.sourceSize).toBeLessThanOrEqual(frame.height + 1e-6);
    }
  });

  it("maps a captured point into the cropped image", () => {
    const viewport = { sourceX: 100, sourceY: 50, sourceSize: 450, outputSize: GUIDE_IMAGE_SIZE, scale: 2 };
    expect(guideViewportPoint(viewport, { x: 100, y: 50 })).toEqual({ x: 0, y: 0 });
    expect(guideViewportPoint(viewport, { x: 325, y: 275 })).toEqual({ x: 450, y: 450 });
  });

  it("keeps the highlight and its arrows inside the image", () => {
    const viewport = guideViewport(frame, { x: 1600, y: 900, radius: 1200 });
    const marker = guideMarkerInViewport(viewport, { x: 1600, y: 900, radius: 1200 });
    const arrows = reshootGuideArrows(marker, shootingDirections(sphere([0, 0, 0])));

    expect(marker.radius).toBeLessThanOrEqual(GUIDE_IMAGE_SIZE * 0.36);
    for (const arrow of arrows) {
      expect(Number.isFinite(arrow.fromX)).toBe(true);
      expect(arrow.fromX).toBeGreaterThanOrEqual(0);
      expect(arrow.fromX).toBeLessThanOrEqual(GUIDE_IMAGE_SIZE);
      expect(arrow.fromY).toBeGreaterThanOrEqual(0);
      expect(arrow.fromY).toBeLessThanOrEqual(GUIDE_IMAGE_SIZE);
    }
  });
});
