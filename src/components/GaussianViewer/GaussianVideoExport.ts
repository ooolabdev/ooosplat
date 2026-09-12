import {
  BufferTarget,
  CanvasSource,
  Mp4OutputFormat,
  Output,
  canEncodeVideo,
} from "mediabunny";
import {
  VIDEO_DURATION_SECONDS,
} from "./PreviewAnimation";
import { getCurrentLocale, translate } from "../../i18n";

export const GAUSSIAN_VIDEO_WIDTH = 1080;
export const GAUSSIAN_VIDEO_HEIGHT = 1920;
export const GAUSSIAN_VIDEO_FPS = 30;
export const GAUSSIAN_VIDEO_BITRATE = 12_000_000;
export const GAUSSIAN_VIDEO_FRAME_COUNT = VIDEO_DURATION_SECONDS * GAUSSIAN_VIDEO_FPS;

export type GaussianVideoEncodingPhase = "rendering" | "finalizing";

export interface GaussianVideoEncodingProgress {
  phase: GaussianVideoEncodingPhase;
  currentFrame: number;
  totalFrames: number;
  progress: number;
}

export interface GaussianVideoCapability {
  supported: boolean;
  reason: string | null;
}

export async function checkGaussianVideoCapability(): Promise<GaussianVideoCapability> {
  const locale = getCurrentLocale();
  if (!window.isSecureContext) {
    return { supported: false, reason: translate(locale, "video.insecure") };
  }
  if (typeof VideoEncoder === "undefined") {
    return { supported: false, reason: translate(locale, "video.noWebCodecs") };
  }
  try {
    const supported = await canEncodeVideo("avc", {
      bitrate: GAUSSIAN_VIDEO_BITRATE,
      width: GAUSSIAN_VIDEO_WIDTH,
      height: GAUSSIAN_VIDEO_HEIGHT,
    });
    return supported
      ? { supported: true, reason: null }
      : { supported: false, reason: translate(locale, "video.noAvc") };
  } catch (error) {
    return {
      supported: false,
      reason: translate(locale, "video.capabilityError", { detail: error instanceof Error ? error.message : String(error) }),
    };
  }
}

export function drawOoosplatWatermark(
  context: CanvasRenderingContext2D,
  logo: CanvasImageSource,
) {
  const margin = 48;
  const logoSize = 70;
  const gap = 18;
  const label = "OOOSplat";
  context.save();
  context.font = '700 40px Arial, "Segoe UI", sans-serif';
  context.textBaseline = "middle";
  const textWidth = context.measureText(label).width;
  const width = logoSize + gap + textWidth + 34;
  const height = 94;
  const x = GAUSSIAN_VIDEO_WIDTH - margin - width;
  const y = GAUSSIAN_VIDEO_HEIGHT - margin - height;

  context.globalAlpha = 0.82;
  context.fillStyle = "rgba(8, 14, 25, 0.62)";
  context.beginPath();
  context.roundRect(x, y, width, height, 16);
  context.fill();

  context.globalAlpha = 0.94;
  context.drawImage(logo, x + 12, y + 12, logoSize, logoSize);
  context.shadowColor = "rgba(0, 0, 0, 0.55)";
  context.shadowBlur = 8;
  context.fillStyle = "#ffffff";
  context.fillText(label, x + 12 + logoSize + gap, y + height / 2 + 1);
  context.restore();
}

export async function encodeGaussianVideo({
  canvas,
  logo,
  renderFrameAt,
  signal,
  onProgress,
}: {
  canvas: HTMLCanvasElement;
  logo: CanvasImageSource;
  renderFrameAt: (timeSeconds: number, context: CanvasRenderingContext2D) => Promise<void>;
  signal: AbortSignal;
  onProgress?: (progress: GaussianVideoEncodingProgress) => void;
}) {
  if (canvas.width !== GAUSSIAN_VIDEO_WIDTH || canvas.height !== GAUSSIAN_VIDEO_HEIGHT) {
    throw new Error(translate(getCurrentLocale(), "video.invalidCanvas"));
  }
  const context = canvas.getContext("2d", { alpha: false });
  if (!context) throw new Error(translate(getCurrentLocale(), "video.noCanvas"));
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";

  const target = new BufferTarget();
  const output = new Output({
    format: new Mp4OutputFormat({ fastStart: "in-memory" }),
    target,
  });
  const source = new CanvasSource(canvas, {
    bitrate: GAUSSIAN_VIDEO_BITRATE,
    codec: "avc",
    keyFrameInterval: 2,
  });
  output.addVideoTrack(source, { frameRate: GAUSSIAN_VIDEO_FPS });

  const cancelOutput = () => {
    if (output.state !== "canceled" && output.state !== "finalized") void output.cancel();
  };
  signal.addEventListener("abort", cancelOutput, { once: true });

  try {
    await output.start();
    const frameDuration = 1 / GAUSSIAN_VIDEO_FPS;
    for (let frame = 0; frame < GAUSSIAN_VIDEO_FRAME_COUNT; frame += 1) {
      signal.throwIfAborted();
      const time = frame * frameDuration;
      await renderFrameAt(time, context);
      drawOoosplatWatermark(context, logo);
      signal.throwIfAborted();
      await source.add(time, frameDuration);
      onProgress?.({
        phase: "rendering",
        currentFrame: frame + 1,
        totalFrames: GAUSSIAN_VIDEO_FRAME_COUNT,
        progress: (frame + 1) / GAUSSIAN_VIDEO_FRAME_COUNT,
      });
    }
    signal.throwIfAborted();
    source.close();
    onProgress?.({
      phase: "finalizing",
      currentFrame: GAUSSIAN_VIDEO_FRAME_COUNT,
      totalFrames: GAUSSIAN_VIDEO_FRAME_COUNT,
      progress: 1,
    });
    await output.finalize();
    signal.throwIfAborted();
    if (!target.buffer || target.buffer.byteLength === 0) {
      throw new Error(translate(getCurrentLocale(), "video.empty"));
    }
    return new Uint8Array(target.buffer);
  } catch (error) {
    if (output.state !== "canceled" && output.state !== "finalized") {
      await output.cancel().catch(() => undefined);
    }
    throw error;
  } finally {
    signal.removeEventListener("abort", cancelOutput);
  }
}

export async function loadWatermarkLogo(source: string) {
  return new Promise<HTMLImageElement>((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(translate(getCurrentLocale(), "viewer.watermarkLoad")));
    image.src = source;
  });
}
