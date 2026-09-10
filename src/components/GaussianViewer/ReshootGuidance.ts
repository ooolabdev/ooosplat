import type { GaussianCrop } from "../../types/pipeline";

export type ReshootRegion = Exclude<GaussianCrop, null>;

/** Two selections closer than this describe the same spot, not a new region. */
const REGION_PRECISION = 3;

export interface ReshootEntry {
  region: ReshootRegion;
  guidance: string;
  /** Annotated screenshot: the circled area plus arrows showing where to stand. */
  guideImage: string | null;
}

export interface GuideMarker {
  /** Screen position of the region centre in image pixels. */
  x: number;
  y: number;
  radius: number;
}

export interface GuideArrow {
  fromX: number;
  fromY: number;
  toX: number;
  toY: number;
  label: string;
  /** Where the label sits relative to the arrow tail. */
  labelX: number;
  labelY: number;
}

export interface GuideLabel {
  text: string;
  x: number;
  y: number;
}

const round = (value: number) => Math.round(value * 10 ** REGION_PRECISION) / 10 ** REGION_PRECISION;

const triple = (values: [number, number, number]) => values.map(round).join(",");

/** Stable identity for a selection, so an identical region cannot be added twice. */
export function regionKey(region: ReshootRegion): string {
  return region.kind === "sphere"
    ? `sphere:${triple(region.center)}:${round(region.radius)}`
    : `box:${triple(region.center)}:${triple(region.size)}`;
}

export function isSameReshootRegion(left: ReshootRegion, right: ReshootRegion): boolean {
  return regionKey(left) === regionKey(right);
}

export function findReshootRegion(regions: ReshootRegion[], region: ReshootRegion): number {
  const key = regionKey(region);
  return regions.findIndex((item) => regionKey(item) === key);
}

const format = (value: number) => value.toFixed(2);

export function regionGeometryLabel(region: ReshootRegion): string {
  const center = `中心 (${region.center.map(format).join(", ")})`;
  return region.kind === "sphere"
    ? `球选 · ${center} · 半径 ${format(region.radius)}`
    : `盒选 · ${center} · 尺寸 (${region.size.map(format).join(", ")})`;
}

/**
 * Guidance must never read the same for two different regions, so every line
 * carries the selection geometry that tells the shooter which spot it means.
 */
export function reshootGuidance(region: ReshootRegion, index: number): string {
  const geometry = regionGeometryLabel(region);
  return region.kind === "sphere"
    ? `区域 ${index + 1}｜${geometry}｜绕该区域缓慢环拍两周：第一周低角度、第二周抬高约 30°，每圈至少 12 个机位，始终让该区域位于画面中央并保留前景与背景两层视差。`
    : `区域 ${index + 1}｜${geometry}｜从正面、左侧、右侧与上方各补拍一组高清画面，相邻机位间隔约 30°；避免仅原地变焦或只沿一个方向平移。`;
}

export function reshootRegionsGuidance(regions: ReshootRegion[]): string[] {
  return regions.map((region, index) => reshootGuidance(region, index));
}

/** Shooting positions the arrows point from, described in the guide image. */
export function shootingDirections(region: ReshootRegion): Array<{ angleDeg: number; label: string }> {
  if (region.kind === "sphere") {
    return [
      { angleDeg: -90, label: "正前低角度" },
      { angleDeg: -45, label: "右前低角度" },
      { angleDeg: 0, label: "右侧低角度" },
      { angleDeg: 45, label: "右后低角度" },
      { angleDeg: 90, label: "正后低角度" },
      { angleDeg: 135, label: "左后低角度" },
      { angleDeg: 180, label: "左侧低角度" },
      { angleDeg: -135, label: "左前低角度" },
      { angleDeg: -90, label: "正前抬高 30°" },
      { angleDeg: 0, label: "右侧抬高 30°" },
      { angleDeg: 90, label: "正后抬高 30°" },
      { angleDeg: 180, label: "左侧抬高 30°" },
    ];
  }
  return [
    { angleDeg: -90, label: "正面" },
    { angleDeg: 180, label: "左侧" },
    { angleDeg: 0, label: "右侧" },
    { angleDeg: -45, label: "右前 30°" },
    { angleDeg: -135, label: "左前 30°" },
  ];
}

/**
 * Places an arrow for every shooting direction on a ring around the circled
 * area. Arrows point inward, so the tail marks where the camera should stand.
 */
export function reshootGuideArrows(marker: GuideMarker, directions: Array<{ angleDeg: number; label: string }>, arrowLength = 46): GuideArrow[] {
  const ringRadius = Math.max(marker.radius + 18, 42);
  return directions.map(({ angleDeg, label }) => {
    const radians = (angleDeg * Math.PI) / 180;
    const dirX = Math.cos(radians);
    const dirY = Math.sin(radians);
    const tailDistance = ringRadius + arrowLength;
    return {
      fromX: marker.x + dirX * tailDistance,
      fromY: marker.y + dirY * tailDistance,
      toX: marker.x + dirX * ringRadius,
      toY: marker.y + dirY * ringRadius,
      label,
      labelX: marker.x + dirX * (tailDistance + 10),
      labelY: marker.y + dirY * (tailDistance + 10),
    };
  });
}

/** Radius the highlighted circle uses for a region, in screen pixels. */
export function guideMarkerRadius(region: ReshootRegion, pixelsPerUnit: number): number {
  const base = region.kind === "sphere" ? region.radius : Math.max(...region.size) / 2;
  return Math.max(24, Math.min(420, Math.abs(base) * pixelsPerUnit));
}

/**
 * Space the arrows, their labels, and a margin need beyond the highlight. The
 * guide crops to this so a small selection is never a speck in a full screenshot.
 */
export const GUIDE_ARROW_MARGIN = 96;

/** Fixed output size keeps labels and arrows readable on any window size. */
export const GUIDE_IMAGE_SIZE = 900;

/** A tiny marker is magnified until the crop is at most this share of the frame. */
const GUIDE_MIN_VIEW_FRACTION = 0.35;

export interface GuideViewport {
  /** Square window taken from the capture. */
  sourceX: number;
  sourceY: number;
  sourceSize: number;
  /** Window scaled into a fixed-size square output. */
  outputSize: number;
  scale: number;
}

/**
 * Chooses the square crop drawn around the region: big enough for every arrow,
 * small enough that the selection stays legible, and always inside the frame.
 */
export function guideViewport(
  frame: { width: number; height: number },
  marker: GuideMarker,
): GuideViewport {
  const minSide = Math.min(frame.width, frame.height);
  const required = (marker.radius + GUIDE_ARROW_MARGIN) * 2;
  const sourceSize = Math.max(1, Math.min(minSide, Math.max(minSide * GUIDE_MIN_VIEW_FRACTION, required)));
  const clamp = (value: number, minimum: number, maximum: number) => Math.min(Math.max(value, minimum), Math.max(minimum, maximum));
  return {
    sourceX: clamp(marker.x - sourceSize / 2, 0, frame.width - sourceSize),
    sourceY: clamp(marker.y - sourceSize / 2, 0, frame.height - sourceSize),
    sourceSize,
    outputSize: GUIDE_IMAGE_SIZE,
    scale: GUIDE_IMAGE_SIZE / sourceSize,
  };
}

/** Maps a captured-frame point into the cropped guide image. */
export function guideViewportPoint(viewport: GuideViewport, point: { x: number; y: number }): { x: number; y: number } {
  return {
    x: (point.x - viewport.sourceX) * viewport.scale,
    y: (point.y - viewport.sourceY) * viewport.scale,
  };
}

/**
 * Keeps the highlight inside the guide image: an oversized or off-screen
 * selection is drawn at the largest size that still leaves room for the arrows.
 */
export function guideMarkerInViewport(
  viewport: GuideViewport,
  marker: GuideMarker,
): GuideMarker {
  const mapped = guideViewportPoint(viewport, marker);
  const radius = Math.min(Math.max(marker.radius * viewport.scale, viewport.outputSize * 0.08), viewport.outputSize * 0.36);
  const margin = radius + GUIDE_ARROW_MARGIN * (viewport.outputSize / GUIDE_IMAGE_SIZE);
  const clamp = (value: number) => Math.min(Math.max(value, margin), viewport.outputSize - margin);
  return { x: clamp(mapped.x), y: clamp(mapped.y), radius };
}

export function guideCaption(region: ReshootRegion, index: number): string {
  return `区域 ${index + 1} · ${region.kind === "sphere" ? "球选" : "盒选"} · 箭头 = 补拍机位（箭头指向被补拍区域）`;
}

/**
 * Draws the reshoot guide over a captured preview frame: the circled region,
 * an inward arrow for every shooting position, and its label.
 */
export function drawReshootGuide(
  context: CanvasRenderingContext2D,
  options: {
    width: number;
    height: number;
    marker: GuideMarker;
    region: ReshootRegion;
    index: number;
    directions: Array<{ angleDeg: number; label: string }>;
  },
): void {
  const { width, height, marker, region, index, directions } = options;
  const arrows = reshootGuideArrows(marker, directions);
  const scale = width / GUIDE_IMAGE_SIZE;
  const fontSize = Math.round(15 * scale);
  const captionSize = Math.round(18 * scale);

  context.save();
  context.lineWidth = 3 * scale;
  context.strokeStyle = "#ffd166";
  context.fillStyle = "rgba(255, 209, 102, .18)";
  context.beginPath();
  context.arc(marker.x, marker.y, marker.radius, 0, Math.PI * 2);
  context.fill();
  context.stroke();

  context.strokeStyle = "#ff5d73";
  context.fillStyle = "#ff5d73";
  for (const arrow of arrows) {
    const angle = Math.atan2(arrow.toY - arrow.fromY, arrow.toX - arrow.fromX);
    const headLength = 14 * scale;
    context.beginPath();
    context.moveTo(arrow.fromX, arrow.fromY);
    context.lineTo(arrow.toX, arrow.toY);
    context.stroke();
    context.beginPath();
    context.moveTo(arrow.toX, arrow.toY);
    context.lineTo(
      arrow.toX - headLength * Math.cos(angle - Math.PI / 7),
      arrow.toY - headLength * Math.sin(angle - Math.PI / 7),
    );
    context.lineTo(
      arrow.toX - headLength * Math.cos(angle + Math.PI / 7),
      arrow.toY - headLength * Math.sin(angle + Math.PI / 7),
    );
    context.closePath();
    context.fill();
  }

  context.font = `600 ${fontSize}px 'Microsoft YaHei UI', 'Segoe UI', sans-serif`;
  context.textAlign = "center";
  context.textBaseline = "middle";
  context.fillStyle = "#ffffff";
  context.strokeStyle = "rgba(0, 0, 0, .72)";
  context.lineWidth = 4 * scale;
  for (const arrow of arrows) {
    // Keep a label inside the picture even when its arrow touches an edge.
    const halfWidth = context.measureText(arrow.label).width / 2 + 6 * scale;
    const labelX = Math.min(Math.max(arrow.labelX, halfWidth), width - halfWidth);
    const labelY = Math.min(Math.max(arrow.labelY, fontSize), height - captionSize * 1.6);
    context.strokeText(arrow.label, labelX, labelY);
    context.fillText(arrow.label, labelX, labelY);
  }

  const caption = guideCaption(region, index);
  context.textAlign = "left";
  context.font = `700 ${captionSize}px 'Microsoft YaHei UI', 'Segoe UI', sans-serif`;
  context.strokeStyle = "rgba(0, 0, 0, .78)";
  context.lineWidth = 5 * scale;
  context.strokeText(caption, 18 * scale, height - 24 * scale);
  context.fillText(caption, 18 * scale, height - 24 * scale);
  context.restore();
}

/** Composes a full guide image from a captured frame and returns a PNG data URL. */
export function composeReshootGuideImage(options: {
  frame: { width: number; height: number; rgba: Uint8ClampedArray | Uint8Array };
  marker: GuideMarker;
  region: ReshootRegion;
  index: number;
  directions: Array<{ angleDeg: number; label: string }>;
}): string | null {
  const { frame, marker, region, index, directions } = options;
  if (frame.width <= 0 || frame.height <= 0) return null;

  // Draw the capture first, then crop and magnify the area around the region so
  // a small selection is never a speck inside a full screenshot.
  const source = document.createElement("canvas");
  source.width = frame.width;
  source.height = frame.height;
  const sourceContext = source.getContext("2d");
  if (!sourceContext) return null;
  const image = sourceContext.createImageData(frame.width, frame.height);
  image.data.set(frame.rgba);
  sourceContext.putImageData(image, 0, 0);

  const viewport = guideViewport(frame, marker);
  const canvas = document.createElement("canvas");
  canvas.width = viewport.outputSize;
  canvas.height = viewport.outputSize;
  const context = canvas.getContext("2d");
  if (!context) return null;
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  context.drawImage(
    source,
    viewport.sourceX,
    viewport.sourceY,
    viewport.sourceSize,
    viewport.sourceSize,
    0,
    0,
    viewport.outputSize,
    viewport.outputSize,
  );
  // Slightly darken the frame so the highlight and arrows stand out.
  context.fillStyle = "rgba(6, 10, 18, .22)";
  context.fillRect(0, 0, viewport.outputSize, viewport.outputSize);
  drawReshootGuide(context, {
    width: viewport.outputSize,
    height: viewport.outputSize,
    marker: guideMarkerInViewport(viewport, marker),
    region,
    index,
    directions,
  });
  return canvas.toDataURL("image/png");
}
