import {
  forwardRef,
  memo,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { Application, Entity } from "@playcanvas/react";
import { Camera, GSplat } from "@playcanvas/react/components";
import { useApp, useSplat } from "@playcanvas/react/hooks";
import {
  ADDRESS_CLAMP_TO_EDGE,
  BoundingBox,
  DEVICETYPE_WEBGL2,
  Entity as PcEntity,
  FILTER_LINEAR,
  GAMMA_SRGB,
  GSPLAT_STREAM_INSTANCE,
  type GSplatComponent,
  GSplatResource,
  Mat4,
  PIXELFORMAT_RGBA8,
  PIXELFORMAT_R8,
  RenderTarget,
  Texture,
  TONEMAP_LINEAR,
  Vec3,
  WORKBUFFER_UPDATE_ALWAYS,
  WORKBUFFER_UPDATE_AUTO,
  WORKBUFFER_UPDATE_ONCE,
  type Application as PcApplication,
  type CameraComponent,
  type WebglGraphicsDevice,
} from "playcanvas";
import {
  ArrowLeft,
  Film,
  FolderOpen,
  LoaderCircle,
  Minus,
  Move,
  Orbit,
  Play,
  Plus,
  Redo2,
  RotateCcw,
  Save,
  Undo2,
  X,
  ZoomIn,
  Box,
  CircleDot,
  MousePointer2,
  RectangleHorizontal,
  Trash2,
} from "lucide-react";
import appLogo from "../../../assets/app-icon.svg";
import {
  beginGaussianEditSave,
  beginGaussianVideoExport,
  cancelGaussianVideoExport,
  commitGaussianEditSave,
  commitGaussianVideoExport,
  exportTransformedGaussian,
  onGaussianExportProgress,
  revealFile,
  saveGaussianTransform,
} from "../../lib/backend";
import { previewAssetUrl as withPreviewAssetRevision } from "../../lib/previewAssetUrl";
import { cloneCrop, IDENTITY_TRANSFORM, useGaussianTransformStore } from "../../stores/gaussianTransformStore";
import type {
  GaussianCrop,
  GaussianEditorTool,
  GaussianExportProgress,
  GaussianOrthographicView,
  GaussianTransform,
  GaussianVideoExportResult,
  GaussianVideoExportSession,
} from "../../types/pipeline";
import { PLY_TO_ENGINE_ROTATION } from "./CoordinateSystem";
import {
  composeReshootGuideImage,
  findReshootRegion,
  guideMarkerRadius,
  regionGeometryLabel,
  regionKey,
  reshootGuidance,
  shootingDirections,
  type ReshootEntry,
} from "./ReshootGuidance";
import {
  copyFlippedRgbaRows,
  normalizedCaptureRegion,
  verticalFovForCapture,
  type NormalizedCaptureRegion,
} from "./PreviewCapture";
import {
  GAUSSIAN_VIDEO_FRAME_COUNT,
  GAUSSIAN_VIDEO_HEIGHT,
  GAUSSIAN_VIDEO_WIDTH,
  checkGaussianVideoCapability,
  encodeGaussianVideo,
  loadWatermarkLogo,
  type GaussianVideoCapability,
  type GaussianVideoEncodingProgress,
} from "./GaussianVideoExport";
import { GroundGrid } from "./GroundGrid";
import { CropOutline } from "./CropOutline";
import { GaussianSelectionController, type RectangleSelectionMode, type SelectionRectangle } from "./GaussianSelectionController";
import { OrbitAxisGuide } from "./OrbitAxisGuide";
import {
  ORBIT_DEGREES_PER_SECOND,
  ORBIT_START_SECONDS,
  PREVIEW_ANIMATION_GLSL,
  animationEffectsActive,
  animationPhaseAt,
  orbitDegreesAt,
  robustEffectBounds,
  type PreviewAnimationPhase,
} from "./PreviewAnimation";
import { TransformPanel } from "./TransformPanel";
import { SelectionPanel } from "./SelectionPanel";
import { EDITABLE_SPLAT_ASSET_OPTIONS, splatTextureCapacityError } from "./SplatLoadPolicy";
import { ViewerControls, type ViewerCameraState } from "./ViewerControls";

type ViewerMode = "adjust" | "preview";
type ViewportPhase = "initializing" | "loading" | "mounting" | "ready" | "error";
type ViewportStatus = {
  phase: ViewportPhase;
  progress: number;
  error: string | null;
  renderer: string;
};
type AnimationStatus = {
  phase: PreviewAnimationPhase;
  elapsedSeconds: number;
};
type VideoExportPhase = "idle" | "preparing" | "rendering" | "finalizing" | "saving" | "completed" | "error";

interface SplatSceneApi {
  replay: () => void;
  selectRectangle: (rectangle: SelectionRectangle, selectionMode: RectangleSelectionMode) => Promise<Uint8Array>;
  freezeCrop: (crop: Exclude<GaussianCrop, null>, deletedMask: Uint8Array) => Promise<Uint8Array>;
  alignView: (view: GaussianOrthographicView) => void;
  initializeCrop: (kind: "sphere" | "box", previous: GaussianCrop) => Exclude<GaussianCrop, null> | null;
  /** Current view as top-down RGBA pixels, used to draw a reshoot guide. */
  captureReshootFrame: () => Promise<{ width: number; height: number; rgba: Uint8ClampedArray } | null>;
  /** Model-space point to image pixels, so the guide can circle the region. */
  projectToScreen: (point: [number, number, number]) => { x: number; y: number; width: number; height: number; visible: boolean } | null;
  exportVideo: (options: {
    signal: AbortSignal;
    onProgress: (progress: GaussianVideoEncodingProgress) => void;
    captureRegion: NormalizedCaptureRegion;
  }) => Promise<Uint8Array>;
}

const INITIAL_VIEWPORT: ViewportStatus = {
  phase: "initializing",
  progress: 0,
  error: null,
  renderer: "WEBGL2 / UNIFIED GSPLAT",
};
const INITIAL_VIDEO_CAPABILITY: GaussianVideoCapability & { checking: boolean } = {
  supported: false,
  reason: null,
  checking: true,
};
const INITIAL_CAMERA_POSITION: [number, number, number] = [0, 0, 5];
const IDENTITY_ROTATION: [number, number, number] = [0, 0, 0];
const IDENTITY_SCALE: [number, number, number] = [1, 1, 1];
const FIT_OCCUPANCY = 0.85;
const SQRT_THREE = Math.sqrt(3);

function effectRadialLimit(fullBounds: BoundingBox, effectBounds: BoundingBox) {
  const minimum = fullBounds.getMin();
  const maximum = fullBounds.getMax();
  let limit = 1;
  for (const x of [minimum.x, maximum.x]) {
    for (const y of [minimum.y, maximum.y]) {
      for (const z of [minimum.z, maximum.z]) {
        const dx = (x - effectBounds.center.x) / Math.max(effectBounds.halfExtents.x, 0.0001);
        const dy = (y - effectBounds.center.y) / Math.max(effectBounds.halfExtents.y, 0.0001);
        const dz = (z - effectBounds.center.z) / Math.max(effectBounds.halfExtents.z, 0.0001);
        limit = Math.max(limit, Math.hypot(dx, dy, dz) / SQRT_THREE);
      }
    }
  }
  return limit;
}

const PreviewCamera = memo(forwardRef<PcEntity>(function PreviewCamera(_props, ref) {
  return <Entity ref={ref} name="OOOSplat Preview Camera" position={INITIAL_CAMERA_POSITION} rotation={IDENTITY_ROTATION} scale={IDENTITY_SCALE}>
    <Camera clearColor="#0e1117" fov={52} nearClip={0.01} farClip={10000} gammaCorrection={GAMMA_SRGB} toneMapping={TONEMAP_LINEAR} />
  </Entity>;
}));

function nextAnimationFrame(signal: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    const abort = () => {
      cancelAnimationFrame(frame);
      reject(new DOMException("视频导出已取消。", "AbortError"));
    };
    const frame = requestAnimationFrame(() => {
      signal.removeEventListener("abort", abort);
      resolve();
    });
    signal.addEventListener("abort", abort, { once: true });
  });
}

function waitForSplatFrame(
  app: PcApplication,
  camera: CameraComponent,
  signal: AbortSignal,
) {
  const gsplatSystem = app.systems.gsplat;
  if (!gsplatSystem) return Promise.reject(new Error("PlayCanvas GSplat 系统不可用。"));
  return new Promise<void>((resolve, reject) => {
    let settled = false;
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timeout);
      signal.removeEventListener("abort", abort);
      handle.off();
      if (error) reject(error);
      else resolve();
    };
    const handle = gsplatSystem.on(
      "frame:ready",
      (frameCamera: CameraComponent, _layer: unknown, ready: boolean, loadingCount: number) => {
        if (frameCamera === camera && ready && loadingCount === 0) finish();
      },
    );
    const abort = () => finish(new DOMException("视频导出已取消。", "AbortError"));
    const timeout = window.setTimeout(
      () => finish(new Error("等待高斯泼溅排序完成超时。请降低系统负载后重试。")),
      8_000,
    );
    signal.addEventListener("abort", abort, { once: true });
    app.renderNextFrame = true;
  });
}

interface SplatSceneProps {
  assetUrl: string;
  splatCount: number;
  transform: GaussianTransform;
  mode: ViewerMode;
  tool: GaussianEditorTool;
  crop: GaussianCrop;
  deletedMask: Uint8Array;
  selectionMask: Uint8Array;
  onOrthographicViewChange: (view: GaussianOrthographicView | null) => void;
  onStatus: (status: ViewportStatus) => void;
  onAnimationStatus: (status: AnimationStatus) => void;
}

const LoadedSplatScene = forwardRef<SplatSceneApi, SplatSceneProps>(function LoadedSplatScene({ assetUrl, transform, mode, tool, crop, deletedMask, selectionMask, onOrthographicViewChange, onStatus, onAnimationStatus }, ref) {
  const app = useApp();
  const cameraRef = useRef<PcEntity>(null);
  const modelRef = useRef<PcEntity>(null);
  const splatRef = useRef<PcEntity>(null);
  const controlsRef = useRef<ViewerControls | null>(null);
  const gridRef = useRef<GroundGrid | null>(null);
  const cropOutlineRef = useRef<CropOutline | null>(null);
  const orbitAxisGuideRef = useRef<OrbitAxisGuide | null>(null);
  const selectionRef = useRef<GaussianSelectionController | null>(null);
  const appDestroyedRef = useRef(false);
  const contextLostRef = useRef(false);
  const modeRef = useRef<ViewerMode>(mode);
  const exportingRef = useRef(false);
  const animationElapsedRef = useRef(0);
  const lastReportedAtRef = useRef(-1);
  const animationComponentRef = useRef<GSplatComponent | null>(null);
  const animationEffectActiveRef = useRef(false);
  const robustLocalBoundsRef = useRef<BoundingBox | null>(null);
  const editorStateRef = useRef({ crop, mode, tool, deletedMask, selectionMask });
  editorStateRef.current = { crop, mode, tool, deletedMask, selectionMask };
  app.scene.gsplatCentersEnabled = true;
  const { asset, loading, error, subscribe } = useSplat(assetUrl, EDITABLE_SPLAT_ASSET_OPTIONS);
  const preparedAsset = useMemo(() => {
    if (!asset) return null;
    const resource = asset.resource as GSplatResource;
    if (!resource.format.extraStreams.some((stream) => stream.name === "ooosplatDeleted")) {
      resource.format.addExtraStreams([
        { name: "ooosplatDeleted", format: PIXELFORMAT_R8, storage: GSPLAT_STREAM_INSTANCE },
        { name: "ooosplatSelected", format: PIXELFORMAT_R8, storage: GSPLAT_STREAM_INSTANCE },
        { name: "ooosplatScratch", format: PIXELFORMAT_R8, storage: GSPLAT_STREAM_INSTANCE },
      ]);
    }
    return asset;
  }, [asset]);
  const renderer = `${app.graphicsDevice.deviceType.toUpperCase()} / UNIFIED GSPLAT`;

  const setAnimationUniforms = useCallback((enabled: boolean, elapsedSeconds: number) => {
    const component = animationComponentRef.current;
    if (!component || appDestroyedRef.current) return;
    const effectActive = enabled && animationEffectsActive(elapsedSeconds);
    if (effectActive) {
      component.workBufferUpdate = WORKBUFFER_UPDATE_ALWAYS;
      component.setParameter("uOoosplatAnimationEnabled", 1);
      component.setParameter("uOoosplatAnimationTime", elapsedSeconds);
    } else if (animationEffectActiveRef.current || component.getParameter("uOoosplatAnimationEnabled") !== 0) {
      component.workBufferUpdate = WORKBUFFER_UPDATE_AUTO;
      component.setParameter("uOoosplatAnimationEnabled", 0);
      component.setParameter("uOoosplatAnimationTime", Math.min(elapsedSeconds, ORBIT_START_SECONDS));
    }
    animationEffectActiveRef.current = effectActive;
    app.renderNextFrame = true;
  }, [app]);

  const reportAnimation = useCallback((elapsedSeconds: number, force = false) => {
    if (!force && elapsedSeconds - lastReportedAtRef.current < 0.2) return;
    lastReportedAtRef.current = elapsedSeconds;
    onAnimationStatus({ phase: animationPhaseAt(elapsedSeconds), elapsedSeconds });
  }, [onAnimationStatus]);

  const replay = useCallback(() => {
    animationElapsedRef.current = 0;
    lastReportedAtRef.current = -1;
    setAnimationUniforms(true, 0);
    reportAnimation(0, true);
  }, [reportAnimation, setAnimationUniforms]);

  useEffect(() => {
    modeRef.current = mode;
    gridRef.current?.setVisible(mode === "adjust");
    orbitAxisGuideRef.current?.setVisible(mode === "preview" && !exportingRef.current);
    controlsRef.current?.setRectangleSelectionMode(mode === "adjust" && tool === "rectangle");
    if (mode === "preview") {
      replay();
    } else {
      setAnimationUniforms(false, animationElapsedRef.current);
    }
  }, [mode, replay, setAnimationUniforms, tool]);

  useEffect(() => {
    appDestroyedRef.current = false;
    contextLostRef.current = false;
    const handle = app.on("destroy", () => {
      appDestroyedRef.current = true;
      controlsRef.current?.destroy();
      controlsRef.current = null;
      gridRef.current?.destroy();
      gridRef.current = null;
      cropOutlineRef.current?.destroy();
      cropOutlineRef.current = null;
      orbitAxisGuideRef.current?.destroy();
      orbitAxisGuideRef.current = null;
      selectionRef.current?.destroy();
      selectionRef.current = null;
    });
    return () => { handle.off(); };
  }, [app]);

  useEffect(() => {
    const canvas = app.graphicsDevice.canvas;
    const handleContextLost = (event: Event) => {
      event.preventDefault();
      if (contextLostRef.current || appDestroyedRef.current) return;
      contextLostRef.current = true;
      if (controlsRef.current) controlsRef.current.enabled = false;
      onOrthographicViewChange(null);
      onStatus({
        phase: "error",
        progress: 0,
        error: "WebGL2 图形上下文已丢失。大模型可能超过当前显卡或驱动可分配的单次图形资源，请关闭其他图形应用后重新加载。",
        renderer,
      });
    };
    canvas.addEventListener("webglcontextlost", handleContextLost);
    return () => canvas.removeEventListener("webglcontextlost", handleContextLost);
  }, [app, onOrthographicViewChange, onStatus, renderer]);

  const applyEditorUniforms = useCallback(() => {
    const component = animationComponentRef.current;
    if (!component || appDestroyedRef.current) return;
    const { crop: currentCrop, mode: currentMode, tool: currentTool } = editorStateRef.current;
    const kind = currentCrop?.kind === "sphere" ? 1 : currentCrop?.kind === "box" ? 2 : 0;
    component.setParameter("uOoosplatCropKind", kind);
    component.setParameter("uOoosplatCropCenter", currentCrop?.center ?? [0, 0, 0]);
    component.setParameter("uOoosplatCropSize", currentCrop?.kind === "box" ? currentCrop.size : [1, 1, 1]);
    component.setParameter("uOoosplatCropRadius", currentCrop?.kind === "sphere" ? currentCrop.radius : 1);
    component.setParameter("uOoosplatShowSelection", currentMode === "adjust" && currentTool === "rectangle" ? 1 : 0);
    if (!animationEffectActiveRef.current) component.workBufferUpdate = WORKBUFFER_UPDATE_ONCE;
    app.renderNextFrame = true;
  }, [app]);

  useEffect(() => { applyEditorUniforms(); }, [applyEditorUniforms, crop, mode, tool]);
  useEffect(() => {
    cropOutlineRef.current?.setCrop(crop);
    cropOutlineRef.current?.setVisible(mode === "adjust" && (tool === "sphere" || tool === "box") && crop !== null);
    app.renderNextFrame = true;
  }, [app, crop, mode, tool]);
  useEffect(() => {
    void selectionRef.current?.applyMasks(deletedMask, selectionMask);
  }, [deletedMask, selectionMask]);
  useEffect(() => {
    const canvas = app.graphicsDevice.canvas;
    const container = canvas.parentElement ?? canvas;
    const resize = () => {
      if (appDestroyedRef.current || contextLostRef.current || exportingRef.current) return;
      app.graphicsDevice.maxPixelRatio = Math.max(1, window.devicePixelRatio || 1);
      app.resizeCanvas(Math.max(1, container.clientWidth), Math.max(1, container.clientHeight));
    };
    const observer = new ResizeObserver(resize);
    observer.observe(container);
    window.addEventListener("resize", resize);
    resize();
    onStatus({ phase: "loading", progress: 0, error: null, renderer });
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", resize);
    };
  }, [app, onStatus, renderer]);

  useEffect(() => {
    const unsubscribe = subscribe((meta) => {
      if (contextLostRef.current) return;
      onStatus({
        phase: "loading",
        progress: Math.max(0, Math.min(1, meta.progress ?? 0)),
        error: null,
        renderer,
      });
    });
    return () => { unsubscribe(); };
  }, [onStatus, renderer, subscribe]);

  useEffect(() => {
    if (contextLostRef.current) return;
    if (error) onStatus({ phase: "error", progress: 0, error, renderer });
    else if (asset) onStatus({ phase: "mounting", progress: 1, error: null, renderer });
    else if (loading) onStatus({ phase: "loading", progress: 0, error: null, renderer });
  }, [asset, error, loading, onStatus, renderer]);

  useEffect(() => {
    if (!asset) return;
    return () => {
      if (appDestroyedRef.current || !app.assets.get(asset.id)) return;
      asset.unload();
      app.assets.remove(asset);
    };
  }, [app, asset]);

  const transformedModelBounds = useCallback(() => {
    const source = asset?.resource as GSplatResource | undefined;
    const splat = splatRef.current;
    if (!source?.aabb || !splat) return null;
    const transformed = new BoundingBox();
    transformed.setFromTransformedAabb(source.aabb, splat.getWorldTransform());
    return transformed;
  }, [asset]);

  const transformedEffectBounds = useCallback(() => {
    const source = asset?.resource as GSplatResource | undefined;
    const splat = splatRef.current;
    if (!source?.aabb || !splat) return null;
    if (!robustLocalBoundsRef.current) {
      const robust = robustEffectBounds(source.centers, {
        center: [source.aabb.center.x, source.aabb.center.y, source.aabb.center.z],
        halfExtents: [source.aabb.halfExtents.x, source.aabb.halfExtents.y, source.aabb.halfExtents.z],
      });
      robustLocalBoundsRef.current = new BoundingBox(
        new Vec3(...robust.center),
        new Vec3(...robust.halfExtents),
      );
    }
    const transformed = new BoundingBox();
    transformed.setFromTransformedAabb(robustLocalBoundsRef.current, splat.getWorldTransform());
    return transformed;
  }, [asset]);

  const syncSceneBounds = useCallback(() => {
    const controls = controlsRef.current;
    const modelBounds = transformedModelBounds();
    if (!modelBounds) return null;
    const component = animationComponentRef.current;
    const effectBounds = transformedEffectBounds() ?? modelBounds;
    orbitAxisGuideRef.current?.setModelSpan(Math.max(
      modelBounds.halfExtents.x,
      modelBounds.halfExtents.y,
      modelBounds.halfExtents.z,
      0.0001,
    ) * 2);
    if (component) {
      component.setParameter(
        "uOoosplatEffectCenter",
        new Float32Array([effectBounds.center.x, effectBounds.center.y, effectBounds.center.z]),
      );
      component.setParameter(
        "uOoosplatEffectExtent",
        new Float32Array([
          Math.max(effectBounds.halfExtents.x, 0.0001),
          Math.max(effectBounds.halfExtents.y, 0.0001),
          Math.max(effectBounds.halfExtents.z, 0.0001),
        ]),
      );
      component.setParameter("uOoosplatEffectRadialLimit", effectRadialLimit(modelBounds, effectBounds));
    }
    if (controls) {
      const combined = modelBounds.clone();
      const gridBounds = gridRef.current?.bounds;
      if (gridBounds) combined.add(gridBounds);
      controls.setSceneBounds(combined);
    }
    return modelBounds;
  }, [transformedEffectBounds, transformedModelBounds]);

  useEffect(() => {
    const entity = modelRef.current;
    if (!entity) return;
    entity.setLocalPosition(...transform.position);
    entity.setLocalEulerAngles(...transform.rotation);
    entity.setLocalScale(transform.scale, transform.scale, transform.scale);
    syncSceneBounds();
    applyEditorUniforms();
  }, [applyEditorUniforms, syncSceneBounds, transform]);

  const fit = useCallback((resetDirection = false) => {
    const controls = controlsRef.current;
    const source = asset?.resource as GSplatResource | undefined;
    if (!source?.aabb || !controls) return false;
    const values = [
      source.aabb.center.x, source.aabb.center.y, source.aabb.center.z,
      source.aabb.halfExtents.x, source.aabb.halfExtents.y, source.aabb.halfExtents.z,
    ];
    if (values.some((value) => !Number.isFinite(value))) return false;
    const transformed = syncSceneBounds();
    if (!transformed) return false;
    controls.fit(transformed, {
      resetDirection,
      occupancy: FIT_OCCUPANCY,
      rightInsetPx: 0,
    });
    return true;
  }, [asset, syncSceneBounds]);

  useEffect(() => {
    if (!preparedAsset || !cameraRef.current || !modelRef.current || !splatRef.current) return;
    const component = splatRef.current.gsplat;
    if (!component) return;
    animationComponentRef.current = component;
    animationEffectActiveRef.current = false;
    robustLocalBoundsRef.current = null;
    component.setWorkBufferModifier({ glsl: PREVIEW_ANIMATION_GLSL });
    component.workBufferUpdate = WORKBUFFER_UPDATE_AUTO;
    component.setParameter("uOoosplatAnimationEnabled", 0);
    component.setParameter("uOoosplatAnimationTime", 0);
    component.setParameter("uOoosplatEffectCenter", new Float32Array([0, 0, 0]));
    component.setParameter("uOoosplatEffectExtent", new Float32Array([1, 1, 1]));
    component.setParameter("uOoosplatEffectRadialLimit", 1);
    component.setParameter("uOoosplatCropKind", 0);
    component.setParameter("uOoosplatCropCenter", [0, 0, 0]);
    component.setParameter("uOoosplatCropSize", [1, 1, 1]);
    component.setParameter("uOoosplatCropRadius", 1);
    component.setParameter("uOoosplatShowSelection", 0);
    const controls = new ViewerControls(app.graphicsDevice.canvas, cameraRef.current, onOrthographicViewChange);
    controls.setRectangleSelectionMode(modeRef.current === "adjust" && editorStateRef.current.tool === "rectangle");
    controlsRef.current = controls;
    const source = preparedAsset.resource as GSplatResource;
    const selection = new GaussianSelectionController(
      app.graphicsDevice,
      component,
      cameraRef.current.camera!,
      source.numSplats,
      () => { app.renderNextFrame = true; },
    );
    selectionRef.current = selection;
    void selection.applyMasks(editorStateRef.current.deletedMask, editorStateRef.current.selectionMask);
    const gridBounds = new BoundingBox();
    gridBounds.setFromTransformedAabb(source.aabb, splatRef.current.getWorldTransform());
    const grid = new GroundGrid(app, gridBounds);
    grid.setVisible(modeRef.current === "adjust");
    gridRef.current = grid;
    const cropOutline = new CropOutline(app);
    cropOutline.setCrop(editorStateRef.current.crop);
    cropOutline.setVisible(modeRef.current === "adjust" && (editorStateRef.current.tool === "sphere" || editorStateRef.current.tool === "box") && editorStateRef.current.crop !== null);
    cropOutlineRef.current = cropOutline;
    const modelSpan = Math.max(
      gridBounds.halfExtents.x,
      gridBounds.halfExtents.y,
      gridBounds.halfExtents.z,
      0.0001,
    ) * 2;
    const orbitAxisGuide = new OrbitAxisGuide(app, controls, modelSpan);
    orbitAxisGuide.setVisible(modeRef.current === "preview");
    orbitAxisGuideRef.current = orbitAxisGuide;
    syncSceneBounds();
    applyEditorUniforms();
    setAnimationUniforms(modeRef.current === "preview", animationElapsedRef.current);

    const updateHandle = app.on("update", (deltaSeconds: number) => {
      if (exportingRef.current || modeRef.current !== "preview") return;
      const previous = animationElapsedRef.current;
      const elapsed = previous + Math.min(Math.max(deltaSeconds, 0), 0.1);
      animationElapsedRef.current = elapsed;
      setAnimationUniforms(true, elapsed);
      controls.orbitBy((elapsed - previous) * ORBIT_DEGREES_PER_SECOND);
      reportAnimation(elapsed);
    });

    let firstFrame = 0;
    let readyFrame = 0;
    firstFrame = requestAnimationFrame(() => {
      readyFrame = requestAnimationFrame(() => {
        if (fit(true)) {
          if (modeRef.current === "preview") replay();
          onStatus({ phase: "ready", progress: 1, error: null, renderer });
        } else {
          onStatus({
            phase: "error",
            progress: 0,
            error: "PLY 已加载，但无法读取有效的模型边界。请确认该文件是 OOOSplat 生成的 Brush Gaussian PLY。",
            renderer,
          });
        }
      });
    });

    return () => {
      cancelAnimationFrame(firstFrame);
      cancelAnimationFrame(readyFrame);
      updateHandle.off();
      controls.destroy();
      if (controlsRef.current === controls) controlsRef.current = null;
      grid.destroy();
      if (gridRef.current === grid) gridRef.current = null;
      cropOutline.destroy();
      if (cropOutlineRef.current === cropOutline) cropOutlineRef.current = null;
      orbitAxisGuide.destroy();
      if (orbitAxisGuideRef.current === orbitAxisGuide) orbitAxisGuideRef.current = null;
      selection.destroy();
      if (selectionRef.current === selection) selectionRef.current = null;
      if (!appDestroyedRef.current && animationComponentRef.current === component && component.entity.gsplat === component) {
        component.workBufferUpdate = WORKBUFFER_UPDATE_AUTO;
        component.setParameter("uOoosplatAnimationEnabled", 0);
        component.setWorkBufferModifier(null);
        component.deleteParameter("uOoosplatAnimationEnabled");
        component.deleteParameter("uOoosplatAnimationTime");
        component.deleteParameter("uOoosplatEffectCenter");
        component.deleteParameter("uOoosplatEffectExtent");
        component.deleteParameter("uOoosplatEffectRadialLimit");
        component.deleteParameter("uOoosplatCropKind");
        component.deleteParameter("uOoosplatCropCenter");
        component.deleteParameter("uOoosplatCropSize");
        component.deleteParameter("uOoosplatCropRadius");
        component.deleteParameter("uOoosplatShowSelection");
      }
      if (animationComponentRef.current === component) animationComponentRef.current = null;
      animationEffectActiveRef.current = false;
      robustLocalBoundsRef.current = null;
    };
  }, [app, applyEditorUniforms, fit, onOrthographicViewChange, onStatus, preparedAsset, renderer, replay, reportAnimation, setAnimationUniforms, syncSceneBounds]);

  const exportVideo = useCallback(async ({
    signal,
    onProgress,
    captureRegion,
  }: {
    signal: AbortSignal;
    onProgress: (progress: GaussianVideoEncodingProgress) => void;
    captureRegion: NormalizedCaptureRegion;
  }) => {
    const controls = controlsRef.current;
    const cameraEntity = cameraRef.current;
    if (!controls || !cameraEntity?.camera) throw new Error("预览相机尚未就绪。 ");
    if (exportingRef.current) throw new Error("已有视频正在导出。 ");

    const graphicsDevice = app.graphicsDevice as WebglGraphicsDevice;
    if (graphicsDevice.maxTextureSize < Math.max(GAUSSIAN_VIDEO_WIDTH, GAUSSIAN_VIDEO_HEIGHT)) {
      throw new Error(`显卡最大纹理尺寸 ${graphicsDevice.maxTextureSize} 无法导出 1080 × 1920 视频。`);
    }
    const readbackPixels = new Uint8Array(GAUSSIAN_VIDEO_WIDTH * GAUSSIAN_VIDEO_HEIGHT * 4);
    let frameImageData: ImageData | null = null;
    const cameraState: ViewerCameraState = controls.snapshot();
    const elapsed = animationElapsedRef.current;
    const previousRenderTarget = cameraEntity.camera.renderTarget;
    const previousFov = cameraEntity.camera.fov;
    const previousEnabled = controls.enabled;
    const previousGridVisible = gridRef.current?.isVisible ?? false;
    const previousOrbitAxisVisible = orbitAxisGuideRef.current?.isVisible ?? false;
    const logo = await loadWatermarkLogo(appLogo);
    const outputCanvas = document.createElement("canvas");
    outputCanvas.width = GAUSSIAN_VIDEO_WIDTH;
    outputCanvas.height = GAUSSIAN_VIDEO_HEIGHT;
    const videoTexture = new Texture(graphicsDevice, {
      name: "OOOSplat Portrait Video",
      width: GAUSSIAN_VIDEO_WIDTH,
      height: GAUSSIAN_VIDEO_HEIGHT,
      format: PIXELFORMAT_RGBA8,
      mipmaps: false,
      minFilter: FILTER_LINEAR,
      magFilter: FILTER_LINEAR,
      addressU: ADDRESS_CLAMP_TO_EDGE,
      addressV: ADDRESS_CLAMP_TO_EDGE,
    });
    const videoRenderTarget = new RenderTarget({
      name: "OOOSplat Portrait Video Target",
      colorBuffer: videoTexture,
      depth: true,
      samples: 1,
    });

    exportingRef.current = true;
    controls.enabled = false;
    gridRef.current?.setVisible(false);
    orbitAxisGuideRef.current?.setVisible(false);
    cameraEntity.camera.renderTarget = videoRenderTarget;
    cameraEntity.camera.fov = verticalFovForCapture(previousFov, captureRegion.height);
    setAnimationUniforms(true, 0);

    try {
      return await encodeGaussianVideo({
        canvas: outputCanvas,
        logo,
        signal,
        onProgress,
        renderFrameAt: async (timeSeconds, context) => {
          signal.throwIfAborted();
          animationElapsedRef.current = timeSeconds;
          setAnimationUniforms(true, timeSeconds);
          controls.restore(cameraState);
          controls.setOrbitYaw(cameraState.yaw + orbitDegreesAt(timeSeconds));
          await waitForSplatFrame(app, cameraEntity.camera!, signal);
          await nextAnimationFrame(signal);
          app.render();
          graphicsDevice.setRenderTarget(videoRenderTarget);
          graphicsDevice.updateBegin();
          await graphicsDevice.readPixelsAsync(
            0,
            0,
            GAUSSIAN_VIDEO_WIDTH,
            GAUSSIAN_VIDEO_HEIGHT,
            readbackPixels,
            true,
          );
          signal.throwIfAborted();
          frameImageData ??= context.createImageData(GAUSSIAN_VIDEO_WIDTH, GAUSSIAN_VIDEO_HEIGHT);
          copyFlippedRgbaRows(
            readbackPixels,
            frameImageData.data,
            GAUSSIAN_VIDEO_WIDTH,
            GAUSSIAN_VIDEO_HEIGHT,
          );
          context.putImageData(frameImageData, 0, 0);
        },
      });
    } finally {
      exportingRef.current = false;
      animationElapsedRef.current = elapsed;
      controls.restore(cameraState);
      controls.enabled = previousEnabled;
      gridRef.current?.setVisible(previousGridVisible && modeRef.current === "adjust");
      orbitAxisGuideRef.current?.setVisible(previousOrbitAxisVisible && modeRef.current === "preview");
      cameraEntity.camera.renderTarget = previousRenderTarget;
      cameraEntity.camera.fov = previousFov;
      videoTexture.destroy();
      videoRenderTarget.destroy();
      setAnimationUniforms(modeRef.current === "preview", elapsed);
      reportAnimation(elapsed, true);
      app.renderNextFrame = true;
    }
  }, [app, reportAnimation, setAnimationUniforms]);

  const selectRectangle = useCallback((rectangle: SelectionRectangle, selectionMode: RectangleSelectionMode) => {
    const selection = selectionRef.current;
    if (!selection) return Promise.reject(new Error("Gaussian 选择器尚未就绪"));
    if (contextLostRef.current) return Promise.reject(new Error("图形上下文已丢失，请重新加载预览。"));
    return selection.select(rectangle, selectionMode, editorStateRef.current.crop);
  }, []);

  const freezeCrop = useCallback((currentCrop: Exclude<GaussianCrop, null>, currentDeletedMask: Uint8Array) => {
    const selection = selectionRef.current;
    if (!selection) return Promise.reject(new Error("Gaussian 选择器尚未就绪"));
    if (contextLostRef.current) return Promise.reject(new Error("图形上下文已丢失，请重新加载预览。"));
    return selection.freezeCrop(currentCrop, currentDeletedMask);
  }, []);

  const cropBounds = useCallback(() => {
    const current = editorStateRef.current.crop;
    if (!current) return transformedModelBounds();
    const half = current.kind === "sphere"
      ? new Vec3(current.radius, current.radius, current.radius)
      : new Vec3(current.size[0] / 2, current.size[1] / 2, current.size[2] / 2);
    return new BoundingBox(new Vec3(...current.center), half);
  }, [transformedModelBounds]);

  const alignView = useCallback((view: GaussianOrthographicView) => {
    const bounds = cropBounds();
    if (bounds) controlsRef.current?.alignOrthographic(view, bounds, FIT_OCCUPANCY);
  }, [cropBounds]);

  const initializeCrop = useCallback((kind: "sphere" | "box", previous: GaussianCrop) => {
    if (previous?.kind === kind) return previous;
    if (previous?.kind === "box" && kind === "sphere") {
      return { kind, center: [...previous.center], radius: Math.hypot(...previous.size) / 2 } as Exclude<GaussianCrop, null>;
    }
    if (previous?.kind === "sphere" && kind === "box") {
      return { kind, center: [...previous.center], size: [previous.radius * 2, previous.radius * 2, previous.radius * 2] } as Exclude<GaussianCrop, null>;
    }
    const bounds = transformedModelBounds();
    if (!bounds) return null;
    const center: [number, number, number] = [bounds.center.x, bounds.center.y, bounds.center.z];
    const size: [number, number, number] = [bounds.halfExtents.x * 2.04, bounds.halfExtents.y * 2.04, bounds.halfExtents.z * 2.04];
    return kind === "sphere"
      ? { kind, center, radius: Math.hypot(...size) / 2 }
      : { kind, center, size };
  }, [transformedModelBounds]);

  /**
   * Captures the current view for a reshoot guide, and projects a model-space
   * point to image pixels so the guide can circle the selected region.
   */
  const captureReshootFrame = useCallback(async () => {
    if (appDestroyedRef.current || contextLostRef.current) return null;
    const device = app.graphicsDevice as WebglGraphicsDevice;
    const width = device.width;
    const height = device.height;
    if (!width || !height) return null;
    app.render();
    const pixels = new Uint8Array(width * height * 4);
    device.setRenderTarget(null);
    device.updateBegin();
    await device.readPixelsAsync(0, 0, width, height, pixels, true);
    if (appDestroyedRef.current) return null;
    const rgba = new Uint8ClampedArray(width * height * 4);
    copyFlippedRgbaRows(pixels, rgba, width, height);
    return { width, height, rgba };
  }, [app]);

  const projectToScreen = useCallback((point: [number, number, number]) => {
    const cameraEntity = cameraRef.current;
    const splatEntity = splatRef.current;
    if (!cameraEntity?.camera || !splatEntity) return null;
    const device = app.graphicsDevice;
    const matrix = new Mat4().mul2(cameraEntity.camera.projectionMatrix, cameraEntity.camera.viewMatrix);
    const world = splatEntity.getWorldTransform().transformPoint(new Vec3(point[0], point[1], point[2]));
    const clip = matrix.transformPoint(world);
    if (!Number.isFinite(clip.x) || !Number.isFinite(clip.y)) return null;
    return {
      x: (clip.x * 0.5 + 0.5) * device.width,
      y: (1 - (clip.y * 0.5 + 0.5)) * device.height,
      width: device.width,
      height: device.height,
      visible: clip.z >= 0 && clip.z <= 1,
    };
  }, [app]);

  useImperativeHandle(ref, () => ({ replay, exportVideo, selectRectangle, freezeCrop, alignView, initializeCrop, captureReshootFrame, projectToScreen }), [alignView, captureReshootFrame, exportVideo, freezeCrop, initializeCrop, projectToScreen, replay, selectRectangle]);

  return <>
    <PreviewCamera ref={cameraRef} />
    {preparedAsset && <Entity ref={modelRef} name="Gaussian Splat Transform" position={transform.position} rotation={transform.rotation} scale={[transform.scale, transform.scale, transform.scale]}>
      <Entity ref={splatRef} name="Gaussian PLY Coordinates" rotation={PLY_TO_ENGINE_ROTATION}>
        <GSplat asset={preparedAsset} unified />
      </Entity>
    </Entity>}
  </>;
});

const SplatScene = forwardRef<SplatSceneApi, SplatSceneProps>(function SplatScene(props, ref) {
  const app = useApp();
  const maximumTextureSide = app.graphicsDevice.maxTextureSize;
  const renderer = `${app.graphicsDevice.deviceType.toUpperCase()} / UNIFIED GSPLAT`;
  const capacityError = splatTextureCapacityError(props.splatCount, maximumTextureSide);

  useEffect(() => {
    if (!capacityError) return;
    props.onStatus({
      phase: "error",
      progress: 0,
      error: capacityError,
      renderer,
    });
  }, [capacityError, props.onStatus, renderer]);

  if (capacityError) return null;
  return <LoadedSplatScene ref={ref} {...props} />;
});

const formatBytes = (bytes: number) => bytes >= 1024 ** 3
  ? `${(bytes / 1024 ** 3).toFixed(2)} GB`
  : `${(bytes / 1024 ** 2).toFixed(1)} MB`;
const compact = (values: number[]) => values.map((value) => Number(value.toFixed(2))).join(" / ");
const phaseLabels: Record<PreviewAnimationPhase, string> = {
  reveal: "显现",
  shockwave: "冲击波",
  orbit: "环绕",
};

export function GaussianViewer({ onExit, onDisposed, pipelineRunning, onStartReshoot, reshootEntry = false }: {
  onExit: () => void | Promise<void>;
  onDisposed: (projectId: string) => void;
  pipelineRunning: boolean;
  onStartReshoot: (
    projectId: string,
    regions: NonNullable<GaussianCrop>[],
    guidance: string[],
    guideImages: Array<string | null>,
  ) => void | Promise<void>;
  /** Opened from the project list's reshoot entry: land on the reshoot workflow. */
  reshootEntry?: boolean;
}) {
  const store = useGaussianTransformStore();
  const sceneApiRef = useRef<SplatSceneApi | null>(null);
  const captureGuideRef = useRef<HTMLDivElement | null>(null);
  const saveQueue = useRef(Promise.resolve());
  const editSaveQueue = useRef(Promise.resolve());
  const selectionQueue = useRef<Promise<void>>(Promise.resolve());
  const pendingSavesRef = useRef(0);
  const saveFailedRef = useRef(false);
  const videoAbortRef = useRef<AbortController | null>(null);
  const videoSessionRef = useRef<GaussianVideoExportSession | null>(null);
  const savedOutputRevisionRef = useRef(0);
  const [mode, setMode] = useState<ViewerMode>("adjust");
  const [orthographicView, setOrthographicView] = useState<GaussianOrthographicView | null>(null);
  const [selectionDrag, setSelectionDrag] = useState<null | { pointerId: number; startX: number; startY: number; x: number; y: number; selectionMode: RectangleSelectionMode }>(null);
  const [viewport, setViewport] = useState<ViewportStatus>(INITIAL_VIEWPORT);
  const [rendererRevision, setRendererRevision] = useState(0);
  const [saveRetryRevision, setSaveRetryRevision] = useState(0);
  const [gaussianExporting, setGaussianExporting] = useState(false);
  const [gaussianExportProgress, setGaussianExportProgress] = useState(0);
  const [gaussianExportResult, setGaussianExportResult] = useState<string | null>(null);
  const [animationStatus, setAnimationStatus] = useState<AnimationStatus>({ phase: "reveal", elapsedSeconds: 0 });
  const [videoCapability, setVideoCapability] = useState(INITIAL_VIDEO_CAPABILITY);
  const [videoPhase, setVideoPhase] = useState<VideoExportPhase>("idle");
  const [videoProgress, setVideoProgress] = useState<GaussianVideoEncodingProgress>({
    phase: "rendering",
    currentFrame: 0,
    totalFrames: GAUSSIAN_VIDEO_FRAME_COUNT,
    progress: 0,
  });
  const [videoError, setVideoError] = useState<string | null>(null);
  const [videoResult, setVideoResult] = useState<GaussianVideoExportResult | null>(null);
  const [pendingNavigation, setPendingNavigation] = useState<"preview" | "exit" | null>(null);
  const [navigationSaving, setNavigationSaving] = useState(false);
  const [cropFreezing, setCropFreezing] = useState(false);
  const [cropFreezeError, setCropFreezeError] = useState<string | null>(null);
  const [reshootRegions, setReshootRegions] = useState<ReshootEntry[]>([]);
  const [reshootError, setReshootError] = useState<string | null>(null);
  const [guidePending, setGuidePending] = useState<number | null>(null);
  const previewAssetUrl = useMemo(() => {
    if (!store.descriptor) return "";
    return withPreviewAssetRevision(store.descriptor.assetUrl, "retry", rendererRevision.toString());
  }, [rendererRevision, store.descriptor]);

  const onStatus = useCallback((status: ViewportStatus) => setViewport(status), []);
  const onAnimationStatus = useCallback((status: AnimationStatus) => setAnimationStatus(status), []);
  const busy = cropFreezing || gaussianExporting || !["idle", "completed", "error"].includes(videoPhase);
  const beginSave = useCallback(() => {
    pendingSavesRef.current += 1;
    useGaussianTransformStore.getState().setSaveState("saving");
  }, []);

  useEffect(() => {
    savedOutputRevisionRef.current = 0;
    setGaussianExportResult(null);
    setPendingNavigation(null);
  }, [store.descriptor?.projectId]);
  const finishSave = useCallback((error?: unknown) => {
    pendingSavesRef.current = Math.max(0, pendingSavesRef.current - 1);
    if (error !== undefined) {
      saveFailedRef.current = true;
      useGaussianTransformStore.getState().setSaveState("error", error instanceof Error ? error.message : String(error));
      return;
    }
    if (pendingSavesRef.current === 0 && !saveFailedRef.current) useGaussianTransformStore.getState().setSaveState("saved");
  }, []);

  useEffect(() => {
    let active = true;
    void checkGaussianVideoCapability().then((capability) => {
      if (active) setVideoCapability({ ...capability, checking: false });
    });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    const descriptor = store.descriptor;
    if (!descriptor) return;
    let active = true;
    if (!descriptor.editMaskAssetUrl) {
      store.setInitialDeletedMask(new Uint8Array(store.deletedMask.length));
      return;
    }
    void fetch(descriptor.editMaskAssetUrl)
      .then((response) => {
        if (!response.ok) throw new Error(`删除位图读取失败（HTTP ${response.status}）`);
        return response.arrayBuffer();
      })
      .then((buffer) => {
        if (active) {
          const current = useGaussianTransformStore.getState();
          current.setInitialDeletedMask(new Uint8Array(buffer));
          current.setSaveState("saved");
        }
      })
      .catch((error: unknown) => {
        if (active) useGaussianTransformStore.getState().setSaveState("error", error instanceof Error ? error.message : String(error));
      });
    return () => { active = false; };
  }, [saveRetryRevision, store.descriptor?.editMaskAssetUrl, store.descriptor?.projectId]);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      setViewport((current) => current.phase === "initializing" ? {
        phase: "error",
        progress: 0,
        error: "WebGL2 渲染器初始化超时。请更新显卡驱动和 Microsoft Edge WebView2 Runtime 后重试。",
        renderer: "WEBGL2 / UNIFIED GSPLAT",
      } : current);
    }, 10_000);
    return () => window.clearTimeout(timeout);
  }, [rendererRevision, store.descriptor?.projectId]);

  useEffect(() => {
    if (!store.descriptor || store.revision === 0) return;
    const projectId = store.descriptor.projectId;
    const transform = store.transform;
    beginSave();
    saveQueue.current = saveQueue.current
      .catch(() => undefined)
      .then(() => saveGaussianTransform(projectId, transform))
      .then(() => {
        if (useGaussianTransformStore.getState().descriptor?.projectId === projectId) {
          finishSave();
        }
      })
      .catch((error: unknown) => {
        if (useGaussianTransformStore.getState().descriptor?.projectId === projectId) {
          finishSave(error);
        }
      });
  }, [beginSave, finishSave, saveRetryRevision, store.descriptor?.projectId, store.revision]);

  useEffect(() => {
    if (!store.descriptor || store.editChangeSerial === 0) return;
    const projectId = store.descriptor.projectId;
    const savedSerial = store.editChangeSerial;
    const crop = store.editing.crop;
    const mask = store.deletedMask.slice();
    beginSave();
    editSaveQueue.current = editSaveQueue.current
      .catch(() => undefined)
      .then(async () => {
        const current = useGaussianTransformStore.getState();
        if (current.descriptor?.projectId !== projectId) return;
        const reservation = await beginGaussianEditSave(projectId, crop, current.editing.revision);
        if (reservation.expectedMaskBytes !== mask.byteLength) throw new Error("编辑位图长度与后端预期不一致");
        const editing = await commitGaussianEditSave(reservation.editId, mask);
        if (useGaussianTransformStore.getState().descriptor?.projectId === projectId) {
          useGaussianTransformStore.getState().applySavedEditing(editing, savedSerial);
          finishSave();
        }
      })
      .catch((error: unknown) => {
        if (useGaussianTransformStore.getState().descriptor?.projectId === projectId) finishSave(error);
      });
  }, [beginSave, finishSave, saveRetryRevision, store.descriptor?.projectId, store.editChangeSerial]);

  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (mode !== "adjust" || !store.descriptor || cropFreezing) return;
      const editingField = document.activeElement instanceof HTMLInputElement || document.activeElement instanceof HTMLTextAreaElement;
      if (store.tool === "rectangle" && !editingField && event.key === "Escape") {
        event.preventDefault(); useGaussianTransformStore.getState().clearSelection(); return;
      }
      if (store.tool === "rectangle" && !editingField && (event.key === "Delete" || event.key === "Backspace")) {
        event.preventDefault(); useGaussianTransformStore.getState().deleteSelection(); return;
      }
      if (!event.ctrlKey && !event.metaKey) return;
      const key = event.key.toLowerCase();
      const isUndo = key === "z" && !event.shiftKey;
      const isRedo = (key === "z" && event.shiftKey) || key === "y";
      if (!isUndo && !isRedo) return;
      event.preventDefault();
      if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
      const previousTool = useGaussianTransformStore.getState().tool;
      if (isRedo) useGaussianTransformStore.getState().redo();
      else useGaussianTransformStore.getState().undo();
      const next = useGaussianTransformStore.getState();
      if (next.tool !== previousTool && (next.tool === "sphere" || next.tool === "box")) {
        setOrthographicView("side");
        requestAnimationFrame(() => sceneApiRef.current?.alignView("side"));
      }
    };
    window.addEventListener("keydown", keyDown);
    return () => window.removeEventListener("keydown", keyDown);
  }, [cropFreezing, mode, store.descriptor?.projectId, store.tool]);

  useEffect(() => {
    let unlisten: undefined | (() => void);
    void onGaussianExportProgress((event: GaussianExportProgress) => {
      if (event.projectId === useGaussianTransformStore.getState().descriptor?.projectId) {
        setGaussianExportProgress(event.progress);
      }
    }).then((fn) => { unlisten = fn; });
    return () => unlisten?.();
  }, []);

  useEffect(() => () => {
    videoAbortRef.current?.abort();
    const session = videoSessionRef.current;
    if (session) void cancelGaussianVideoExport(session.exportId);
  }, []);

  useEffect(() => {
    const projectId = store.descriptor?.projectId;
    if (!projectId) return;
    return () => {
      queueMicrotask(() => onDisposed(projectId));
    };
  }, [onDisposed, store.descriptor?.projectId]);

  const exportGaussian = async (): Promise<boolean> => {
    if (!store.descriptor || busy) return false;
    setGaussianExporting(true);
    setGaussianExportProgress(0);
    setGaussianExportResult(null);
    try {
      await Promise.all([saveQueue.current, editSaveQueue.current]);
      const current = useGaussianTransformStore.getState();
      if (current.saveState === "error") throw new Error(current.saveError ?? "编辑状态尚未保存，请重试后再导出");
      const result = await exportTransformedGaussian(store.descriptor.projectId, current.transform, current.editing.revision);
      setGaussianExportProgress(100);
      setGaussianExportResult(result.path);
      savedOutputRevisionRef.current = current.revision;
      return true;
    } catch (error) {
      setViewport((current) => ({ ...current, phase: "error", error: error instanceof Error ? error.message : String(error) }));
      return false;
    } finally {
      setGaussianExporting(false);
    }
  };

  const leavePreview = async () => {
    if (busy) return;
    await Promise.all([saveQueue.current, editSaveQueue.current]).catch(() => undefined);
    if (useGaussianTransformStore.getState().saveState === "error") return;
    await onExit();
  };

  const exportVideo = async () => {
    if (!store.descriptor || !sceneApiRef.current || busy || !videoCapability.supported) return;
    const guide = captureGuideRef.current;
    const canvas = guide?.parentElement?.querySelector("canvas");
    if (!guide || !(canvas instanceof HTMLCanvasElement)) {
      setVideoError("无法读取视频取景框，请退出预览后重试。");
      setVideoPhase("error");
      return;
    }
    let captureRegion: NormalizedCaptureRegion;
    try {
      captureRegion = normalizedCaptureRegion(canvas.getBoundingClientRect(), guide.getBoundingClientRect());
    } catch (error) {
      setVideoError(error instanceof Error ? error.message : String(error));
      setVideoPhase("error");
      return;
    }
    setVideoPhase("preparing");
    setVideoError(null);
    setVideoResult(null);
    setVideoProgress({ phase: "rendering", currentFrame: 0, totalFrames: GAUSSIAN_VIDEO_FRAME_COUNT, progress: 0 });
    const abortController = new AbortController();
    videoAbortRef.current = abortController;
    let committed = false;
    try {
      const session = await beginGaussianVideoExport(store.descriptor.projectId);
      videoSessionRef.current = session;
      setVideoPhase("rendering");
      const bytes = await sceneApiRef.current.exportVideo({
        signal: abortController.signal,
        captureRegion,
        onProgress: (progress) => {
          setVideoProgress(progress);
          setVideoPhase(progress.phase === "finalizing" ? "finalizing" : "rendering");
        },
      });
      abortController.signal.throwIfAborted();
      setVideoPhase("saving");
      const result = await commitGaussianVideoExport(session.exportId, bytes);
      committed = true;
      videoSessionRef.current = null;
      setVideoResult(result);
      setVideoPhase("completed");
    } catch (error) {
      if (!abortController.signal.aborted) {
        setVideoError(error instanceof Error ? error.message : String(error));
        setVideoPhase("error");
      } else {
        setVideoPhase("idle");
      }
    } finally {
      const session = videoSessionRef.current;
      if (session && !committed) await cancelGaussianVideoExport(session.exportId).catch(() => undefined);
      videoSessionRef.current = null;
      if (videoAbortRef.current === abortController) videoAbortRef.current = null;
    }
  };

  const cancelVideo = () => videoAbortRef.current?.abort();
  const retry = () => {
    setViewport(INITIAL_VIEWPORT);
    setRendererRevision((value) => value + 1);
  };
  const retrySave = () => {
    saveFailedRef.current = false;
    useGaussianTransformStore.getState().setSaveState("dirty");
    setSaveRetryRevision((value) => value + 1);
  };
  const runHistory = (direction: "undo" | "redo") => {
    if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
    const previousTool = useGaussianTransformStore.getState().tool;
    useGaussianTransformStore.getState()[direction]();
    const next = useGaussianTransformStore.getState();
    if (next.tool !== previousTool && (next.tool === "sphere" || next.tool === "box")) {
      setOrthographicView("side");
      requestAnimationFrame(() => sceneApiRef.current?.alignView("side"));
    }
  };
  const hasUnsavedGaussianEdits = () => {
    const current = useGaussianTransformStore.getState();
    const transformChanged = current.transform.scale !== IDENTITY_TRANSFORM.scale
      || current.transform.position.some((value, index) => value !== IDENTITY_TRANSFORM.position[index])
      || current.transform.rotation.some((value, index) => value !== IDENTITY_TRANSFORM.rotation[index]);
    const hasEdits = transformChanged || current.editing.crop !== null || current.editing.deletedCount > 0;
    return hasEdits && current.revision !== savedOutputRevisionRef.current;
  };
  const completeNavigation = async (target: "preview" | "exit") => {
    setPendingNavigation(null);
    if (target === "preview") {
      setMode("preview");
      setVideoError(null);
      setVideoPhase("idle");
    } else {
      await leavePreview();
    }
  };
  const requestNavigation = (target: "preview" | "exit") => {
    if (busy) return;
    if (hasUnsavedGaussianEdits()) setPendingNavigation(target);
    else void completeNavigation(target);
  };
  const saveAndContinue = async () => {
    if (!pendingNavigation) return;
    const target = pendingNavigation;
    setNavigationSaving(true);
    const saved = await exportGaussian();
    setNavigationSaving(false);
    if (saved) await completeNavigation(target);
  };
  const switchMode = (nextMode: ViewerMode) => {
    if (busy || nextMode === mode) return;
    if (nextMode === "preview") {
      requestNavigation("preview");
      return;
    }
    setMode("adjust");
    setVideoError(null);
  };

  const enableCrop = (kind: "sphere" | "box", view: GaussianOrthographicView = orthographicView ?? "side") => {
    const crop = sceneApiRef.current?.initializeCrop(kind, store.editing.crop);
    if (crop && JSON.stringify(crop) !== JSON.stringify(store.editing.crop)) {
      store.beginCropTransaction();
      store.setCropLive(crop);
      store.commitCropTransaction();
    }
    requestAnimationFrame(() => sceneApiRef.current?.alignView(view));
  };

  const switchTool = async (tool: GaussianEditorTool) => {
    if (busy || tool === store.tool) return;
    const current = useGaussianTransformStore.getState();
    if (tool === "transform" && (current.tool === "sphere" || current.tool === "box") && current.editing.crop) {
      current.commitCropTransaction();
      const latest = useGaussianTransformStore.getState();
      const crop = latest.editing.crop;
      if (!crop || !sceneApiRef.current) return;
      setCropFreezeError(null);
      setCropFreezing(true);
      try {
        const deletedMask = await sceneApiRef.current.freezeCrop(crop, latest.deletedMask);
        useGaussianTransformStore.getState().commitCropFreeze(crop, deletedMask);
      } catch (error) {
        setCropFreezeError(error instanceof Error ? error.message : String(error));
      } finally {
        setCropFreezing(false);
      }
      return;
    }
    if (tool === "sphere" || tool === "box") {
      setOrthographicView("side");
      enableCrop(tool, "side");
    }
    store.setTool(tool);
  };

  const alignView = (view: GaussianOrthographicView) => {
    setOrthographicView(view);
    sceneApiRef.current?.alignView(view);
  };

  // The reshoot panel only exists while a sphere or box tool is active, so the
  // list entry activates one instead of leaving the user on an empty adjust view.
  const reshootEntryApplied = useRef(false);
  useEffect(() => {
    if (!reshootEntry || reshootEntryApplied.current) return;
    reshootEntryApplied.current = true;
    setMode("adjust");
    const current = useGaussianTransformStore.getState();
    if (current.tool !== "sphere" && current.tool !== "box") void switchTool("sphere");
  }, [reshootEntry, switchTool]);

  const rectanglePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (mode !== "adjust" || store.tool !== "rectangle" || event.button !== 0 || busy || viewport.phase !== "ready") return;
    if ((event.target as Element).closest("button,input,aside")) return;
    event.preventDefault(); event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    setSelectionDrag({ pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, x: event.clientX, y: event.clientY, selectionMode: event.ctrlKey ? "remove" : event.shiftKey ? "add" : "replace" });
  };
  const rectanglePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!selectionDrag || event.pointerId !== selectionDrag.pointerId) return;
    event.preventDefault(); event.stopPropagation();
    setSelectionDrag((current) => current ? { ...current, x: event.clientX, y: event.clientY } : null);
  };
  const rectanglePointerEnd = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = selectionDrag;
    if (!drag || event.pointerId !== drag.pointerId) return;
    event.preventDefault(); event.stopPropagation();
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    setSelectionDrag(null);
    if (Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) < 3) {
      if (drag.selectionMode === "replace") store.clearSelection();
      return;
    }
    const canvas = event.currentTarget.querySelector("canvas");
    if (!(canvas instanceof HTMLCanvasElement) || !sceneApiRef.current) return;
    const bounds = canvas.getBoundingClientRect();
    const toNdcX = (value: number) => (value - bounds.left) / Math.max(bounds.width, 1) * 2 - 1;
    const toNdcY = (value: number) => 1 - (value - bounds.top) / Math.max(bounds.height, 1) * 2;
    const x1 = toNdcX(drag.startX); const x2 = toNdcX(event.clientX); const y1 = toNdcY(drag.startY); const y2 = toNdcY(event.clientY);
    const api = sceneApiRef.current;
    const rectangle = { minX: Math.min(x1, x2), minY: Math.min(y1, y2), maxX: Math.max(x1, x2), maxY: Math.max(y1, y2) };
    selectionQueue.current = selectionQueue.current
      .catch(() => undefined)
      .then(async () => {
        const mask = await api.selectRectangle(rectangle, drag.selectionMode);
        useGaussianTransformStore.getState().setSelectionMask(mask);
      })
      .catch((error: unknown) => setViewport((current) => ({ ...current, error: error instanceof Error ? error.message : String(error) })));
  };

  if (!store.descriptor) return null;

  const loadingLabel = viewport.phase === "initializing"
    ? "正在初始化 WebGL2 渲染器"
    : viewport.phase === "mounting"
      ? "正在创建高斯泼溅 GPU 资源"
      : "正在读取高斯泼溅文件";
  const phaseLabel = {
    initializing: "初始化中",
    loading: "加载中",
    mounting: "准备中",
    ready: "就绪",
    error: "异常",
  }[viewport.phase];
  const videoBusy = !["idle", "completed", "error"].includes(videoPhase);
  const addReshootRegion = async () => {
    const crop = store.editing.crop;
    if (!crop) {
      setReshootError("请先使用“球选择”或“盒选择”圈出模糊区域。");
      return;
    }
    if (findReshootRegion(reshootRegions.map((entry) => entry.region), crop) >= 0) {
      setReshootError("该区域已在补拍清单中。请调整选择范围或移动相机后再加入，避免重复补拍同一区域。");
      return;
    }
    const index = reshootRegions.length;
    const entry: ReshootEntry = { region: cloneCrop(crop)!, guidance: reshootGuidance(crop, index), guideImage: null };
    setReshootRegions((regions) => [...regions, entry]);
    setReshootError(null);
    setGuidePending(index);
    try {
      const projected = sceneApiRef.current?.projectToScreen(crop.center);
      const frame = await sceneApiRef.current?.captureReshootFrame();
      if (!projected || !frame) throw new Error("无法获取当前视角的画面。");
      // Screen pixels per model unit, so the highlight matches the selection size.
      const edgePoint: [number, number, number] = crop.kind === "sphere"
        ? [crop.center[0] + crop.radius, crop.center[1], crop.center[2]]
        : [crop.center[0] + crop.size[0] / 2, crop.center[1], crop.center[2]];
      const edge = sceneApiRef.current?.projectToScreen(edgePoint);
      const pixelsPerUnit = edge ? Math.hypot(edge.x - projected.x, edge.y - projected.y) || 1 : 1;
      const guideImage = composeReshootGuideImage({
        frame,
        marker: { x: projected.x, y: projected.y, radius: guideMarkerRadius(crop, pixelsPerUnit) },
        region: crop,
        index,
        directions: shootingDirections(crop),
      });
      if (!guideImage) throw new Error("无法生成补拍指引图。");
      setReshootRegions((regions) => regions.map((item, position) => position === index ? { ...item, guideImage } : item));
    } catch (error) {
      // The region stays in the list: guidance text and the region geometry are
      // still usable without the annotated frame.
      setReshootError(`区域 ${index + 1} 的指引图生成失败：${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setGuidePending(null);
    }
  };
  const startReshoot = async () => {
    if (!store.descriptor || reshootRegions.length === 0 || busy) {
      setReshootError("请至少添加一个需要补拍的区域。");
      return;
    }
    await onStartReshoot(
      store.descriptor.projectId,
      reshootRegions.map((entry) => entry.region),
      reshootRegions.map((entry) => entry.guidance),
      reshootRegions.map((entry) => entry.guideImage),
    );
  };
  const videoButtonLabel = videoPhase === "preparing"
    ? "准备导出"
    : videoPhase === "rendering"
      ? `渲染 ${videoProgress.currentFrame} / ${videoProgress.totalFrames}`
      : videoPhase === "finalizing"
        ? "正在封装 MP4"
        : videoPhase === "saving"
          ? "正在保存"
          : "导出竖屏视频";
  const toolItems: Array<{ id: GaussianEditorTool; label: string; icon: typeof MousePointer2 }> = [
    { id: "transform", label: "变换", icon: MousePointer2 },
    { id: "rectangle", label: "矩形选择", icon: RectangleHorizontal },
    { id: "sphere", label: "球选择", icon: CircleDot },
    { id: "box", label: "盒选择", icon: Box },
  ];
  const selectionRectStyle = selectionDrag ? {
    left: Math.min(selectionDrag.startX, selectionDrag.x), top: Math.min(selectionDrag.startY, selectionDrag.y),
    width: Math.abs(selectionDrag.x - selectionDrag.startX), height: Math.abs(selectionDrag.y - selectionDrag.startY),
  } : undefined;

  return <section className={`preview-pane active preview-workspace viewer-mode-${mode}`} aria-label="高斯泼溅预览">
    <header className="preview-header">
      <div className="preview-heading">
        <button className="preview-back-icon" type="button" title="返回任务" aria-label="返回任务" disabled={busy} onClick={() => requestNavigation("exit")}><ArrowLeft size={19} /></button>
        <h1>03 预览</h1>
      </div>
      <div className="preview-mode-control">
        <div className={`preview-mode-toggle mode-${mode}`} role="group" aria-label="预览工作模式">
          <button type="button" className={mode === "adjust" ? "active" : ""} aria-pressed={mode === "adjust"} disabled={busy} onClick={() => switchMode("adjust")}>调整</button>
          <button type="button" className={mode === "preview" ? "active" : ""} aria-pressed={mode === "preview"} disabled={busy} onClick={() => switchMode("preview")}>动画</button>
        </div>
        <p>{mode === "adjust" ? "调整高斯泼溅的原点、大小与位置等" : "调整画面并导出展示视频"}</p>
      </div>
      <div className="preview-input-hints" aria-label="视图鼠标操作">
        <span><Orbit size={15} /><kbd>{mode === "adjust" && store.tool === "rectangle" ? "中键" : "左键"}</kbd>旋转</span>
        <span><Move size={15} /><kbd>右键</kbd>拖动</span>
        <span><ZoomIn size={15} /><kbd>滚轮</kbd>缩放</span>
        {mode === "adjust" && store.tool === "rectangle" && <>
          <span><RectangleHorizontal size={15} /><kbd>左键</kbd>框选</span>
          <span><Plus size={15} /><kbd>Shift</kbd>添加</span>
          <span><Minus size={15} /><kbd>Ctrl</kbd>移除</span>
          <span><Trash2 size={15} /><kbd>Delete / Backspace</kbd>删除</span>
          <span><X size={15} /><kbd>Esc</kbd>取消选择</span>
        </>}
      </div>
    </header>
    <div className="preview-commandbar">
      <div className="preview-commandbar-left">
        {mode === "adjust" && <div className="preview-editor-tools" role="toolbar" aria-label="Gaussian 编辑工具">
          {toolItems.map(({ id, label, icon: Icon }) => <button key={id} type="button" className={store.tool === id ? "active" : ""} aria-pressed={store.tool === id} disabled={busy || viewport.phase !== "ready"} onClick={() => void switchTool(id)}><Icon size={14} />{label}</button>)}
        </div>}
      </div>
      <div className="preview-commandbar-center">
        {mode === "adjust" && <div className="preview-view-control" role="group" aria-label="正交视图">
          {(["side", "front", "top"] as const).map((view) => <button key={view} type="button" className={orthographicView === view ? "active" : ""} aria-pressed={orthographicView === view} disabled={busy || viewport.phase !== "ready"} title={`切换到${view === "side" ? "侧视图" : view === "front" ? "正视图" : "顶视图"}`} onClick={() => alignView(view)}>{view === "side" ? "侧视" : view === "front" ? "正视" : "顶视"}</button>)}
        </div>}
      </div>
      <div className="preview-header-actions">
        {mode === "adjust" ? <>
          <button type="button" title="撤销（Ctrl+Z）" disabled={store.history.length === 0 || busy} onClick={() => runHistory("undo")}><Undo2 size={14} />撤销</button>
          <button type="button" title="重做（Ctrl+Shift+Z / Ctrl+Y）" disabled={store.future.length === 0 || busy} onClick={() => runHistory("redo")}><Redo2 size={14} />重做</button>
          {store.tool === "rectangle" && <button type="button" disabled={store.selectedCount === 0 || busy} onClick={store.deleteSelection}><Trash2 size={14} />删除选中</button>}
          <button type="button" title="恢复为原始 final.ply" disabled={busy || (store.history.length === 0 && store.editing.crop === null && store.editing.deletedCount === 0 && store.transform.scale === 1 && store.transform.position.every((value) => value === 0) && store.transform.rotation.every((value) => value === 0))} onClick={() => { store.resetAll(); setGaussianExportResult(null); }}><RotateCcw size={14} />全部撤销</button>
          <button type="button" disabled={busy || viewport.phase !== "ready"} onClick={() => void exportGaussian()}>{gaussianExporting ? <LoaderCircle className="spin" size={14} /> : <Save size={14} />} {gaussianExporting ? `保存中 ${gaussianExportProgress.toFixed(0)}%` : "保存"}</button>
          <button type="button" className="reshoot-action" disabled={busy || viewport.phase !== "ready"} onClick={() => void startReshoot()}><Film size={14} />高清补拍 {reshootRegions.length > 0 ? `(${reshootRegions.length})` : ""}</button>
        </> : <>
          <button type="button" disabled={videoBusy || viewport.phase !== "ready"} onClick={() => sceneApiRef.current?.replay()}><Play size={14} />重新播放</button>
          {videoBusy
            ? <button type="button" className="video-cancel" disabled={videoPhase === "saving"} onClick={cancelVideo}><X size={14} />取消导出</button>
            : <button type="button" disabled={!videoCapability.supported || viewport.phase !== "ready"} title={videoCapability.reason ?? undefined} onClick={() => void exportVideo()}><Film size={14} />{videoButtonLabel}</button>}
        </>}
      </div>
    </div>
    {pipelineRunning && <div className="preview-resource-note">预览与生成任务正在同时使用图形资源，显存不足时交互可能暂时变慢。</div>}
    <div className="preview-editor">
      <div className={`gaussian-viewport tool-${store.tool}`} onPointerDownCapture={rectanglePointerDown} onPointerMoveCapture={rectanglePointerMove} onPointerUpCapture={rectanglePointerEnd} onPointerCancelCapture={rectanglePointerEnd}>
        <Application key={`${store.descriptor.projectId}-${rendererRevision}`} className="gaussian-canvas" deviceTypes={[DEVICETYPE_WEBGL2]} graphicsDeviceOptions={{ antialias: false, alpha: false, preserveDrawingBuffer: true, powerPreference: "high-performance" }}>
          <SplatScene ref={sceneApiRef} assetUrl={previewAssetUrl} splatCount={store.descriptor.splatCount} transform={store.transform} mode={mode} tool={store.tool} crop={store.editing.crop} deletedMask={store.deletedMask} selectionMask={store.selectionMask} onOrthographicViewChange={setOrthographicView} onStatus={onStatus} onAnimationStatus={onAnimationStatus} />
        </Application>
        {mode === "preview" && <div ref={captureGuideRef} className="portrait-capture-guide" aria-hidden="true">
          <div className="portrait-frame-label"><span>1080 × 1920</span><span>30 FPS</span></div>
          <div className="preview-watermark"><img src={appLogo} alt="" /><strong>OOOSplat</strong></div>
        </div>}
        {mode === "preview" && <div className="portrait-matte" aria-hidden="true" />}
        {viewport.phase !== "ready" && viewport.phase !== "error" && <div className="viewport-overlay"><LoaderCircle className="spin" size={22} /><strong>{loadingLabel}</strong>{viewport.phase === "loading" && <span>{(viewport.progress * 100).toFixed(0)}%</span>}</div>}
        {viewport.phase === "error" && <div className="viewport-overlay error"><strong>预览不可用</strong><p>{viewport.error}</p><div className="viewport-error-actions"><button type="button" onClick={retry}>重新加载</button><button type="button" onClick={() => requestNavigation("exit")}>返回任务</button></div></div>}
        {cropFreezing && <div className="viewport-overlay"><LoaderCircle className="spin" size={22} /><strong>正在固定裁切结果</strong><span>请稍候</span></div>}
        {cropFreezeError && !cropFreezing && <div className="viewport-overlay error"><strong>无法固定裁切结果</strong><p>{cropFreezeError}</p><div className="viewport-error-actions"><button type="button" onClick={() => setCropFreezeError(null)}>返回编辑</button></div></div>}
        {selectionDrag && <div className={`rectangle-selection-box mode-${selectionDrag.selectionMode}`} style={selectionRectStyle} aria-hidden="true" />}
        {mode === "adjust" && store.tool === "transform" && <TransformPanel transform={store.transform} onBegin={store.beginTransaction} onChange={store.setTransformLive} onCommit={store.commitTransaction} />}
        {mode === "adjust" && (store.tool === "sphere" || store.tool === "box") && <SelectionPanel crop={store.editing.crop} kind={store.tool} onBegin={store.beginCropTransaction} onChange={store.setCropLive} onCommit={store.commitCropTransaction} onEnable={() => { const tool = useGaussianTransformStore.getState().tool; if (tool === "sphere" || tool === "box") enableCrop(tool); }} />}
        {mode === "adjust" && (store.tool === "sphere" || store.tool === "box") && <div className="reshoot-guide-panel">
          <strong>高清补拍区域</strong>
          <p>用当前{store.tool === "sphere" ? "球选" : "盒选"}圈住模糊或细节不足的位置，然后加入补拍清单。同一区域只会记录一次。</p>
          <button type="button" disabled={!store.editing.crop || busy || guidePending !== null} onClick={() => void addReshootRegion()}>{guidePending !== null ? "正在生成指引图" : "加入当前区域"}</button>
          {reshootRegions.length > 0 && <ol>{reshootRegions.map((entry, index) => <li key={regionKey(entry.region)}>
            <div className="reshoot-guide-head"><b>区域 {index + 1}</b><span>{regionGeometryLabel(entry.region)}</span>
              <button type="button" aria-label={`移除补拍区域 ${index + 1}`} onClick={() => setReshootRegions((regions) => regions.filter((_, item) => item !== index))}>移除</button>
            </div>
            {entry.guideImage
              ? <a className="reshoot-guide-figure" href={entry.guideImage} target="_blank" rel="noreferrer" title="在新标签页查看指引图"><img src={entry.guideImage} alt={`区域 ${index + 1} 补拍方位指引`} /><span>箭头 = 补拍机位，指向该区域</span></a>
              : guidePending === index
                ? <p className="reshoot-guide-figure pending"><LoaderCircle className="spin" size={14} />正在生成方位指引图</p>
                : <p className="reshoot-guide-figure missing">未生成指引图</p>}
            <p className="reshoot-guide-text">{entry.guidance}</p>
          </li>)}</ol>}
          {reshootError && <p className="reshoot-guide-error">{reshootError}</p>}
        </div>}
        {mode === "preview" && <div className="animation-hud">
          <span className={`animation-pulse phase-${animationStatus.phase}`} />
          <b>{phaseLabels[animationStatus.phase]}</b>
          <span className="animation-time">{animationStatus.elapsedSeconds.toFixed(1)}s</span>
          <span className="orbit-axis-key"><i aria-hidden="true" />旋转轴 Y · 24 秒/圈</span>
        </div>}
        {mode === "preview" && videoBusy && <div className="video-export-overlay">
          <div><LoaderCircle className="spin" size={20} /><strong>{videoButtonLabel}</strong></div>
          <div className="video-export-track"><span style={{ width: `${videoProgress.progress * 100}%` }} /></div>
          <small>{videoPhase === "rendering" ? `${Math.round(videoProgress.progress * 100)}% · ${videoProgress.currentFrame} / ${GAUSSIAN_VIDEO_FRAME_COUNT} 帧` : "请保持窗口开启"}</small>
        </div>}
      </div>
    </div>
    <footer className="preview-statusbar">
      <span><b>泼溅数量</b>{store.descriptor.splatCount.toLocaleString()}</span>
      {mode === "adjust" && store.selectedCount > 0 && <span className="selection-count"><b>已选择</b>{store.selectedCount.toLocaleString()}</span>}
      {mode === "adjust" && store.editing.deletedCount > 0 && <span><b>已删除</b>{store.editing.deletedCount.toLocaleString()}</span>}
      <span><b>文件大小</b>{formatBytes(store.descriptor.fileSize)}</span>
      {mode === "adjust"
        ? <span><b>位置</b>{compact(store.transform.position)} <b>旋转</b>{compact(store.transform.rotation)} <b>缩放</b>{Number(store.transform.scale.toFixed(3))}</span>
        : <span><b>时间线</b>显现 5s · 冲击波 8s · 环绕 24s / 圈</span>}
      <span><b>渲染器</b>{viewport.renderer}</span>
      <span><b>状态</b>{phaseLabel}</span>
      {mode === "adjust" && <span className={`save-state ${store.saveState}`}><b>项目</b>{store.saveState === "saving" ? "保存中" : store.saveState === "error" ? "保存失败" : store.saveState === "dirty" ? "未保存" : "已保存"}</span>}
      {gaussianExportResult && mode === "adjust" && <span className="export-result" title={gaussianExportResult}><b>已保存</b>{gaussianExportResult.split(/[\\/]/).at(-1)}</span>}
      {mode === "preview" && <span><b>视频编码</b>{videoCapability.checking ? "检测中" : videoCapability.supported ? "H.264 可用" : "不可用"}</span>}
      {videoResult && mode === "preview" && <button className="statusbar-file-action" type="button" title={videoResult.path} onClick={() => void revealFile(videoResult.path)}><FolderOpen size={12} /><b>已导出</b>{videoResult.path.split(/[\\/]/).at(-1)} · {formatBytes(videoResult.fileSize)}</button>}
    </footer>
    {store.saveError && mode === "adjust" && <div className="preview-save-error"><span>{store.saveError}</span><button type="button" onClick={retrySave}>重试保存</button></div>}
    {mode === "preview" && !videoCapability.checking && !videoCapability.supported && <div className="preview-video-message warning">{videoCapability.reason}</div>}
    {mode === "preview" && videoError && <div className="preview-video-message error">视频导出失败：{videoError}</div>}
    {pendingNavigation && <div className="gaussian-save-backdrop" role="dialog" aria-modal="true" aria-labelledby="gaussian-save-title" aria-describedby="gaussian-save-description">
      <div className="gaussian-save-dialog">
        <div className="gaussian-save-symbol"><Save size={19} /></div>
        <div>
          <h2 id="gaussian-save-title">保存编辑结果？</h2>
          <p id="gaussian-save-description">当前调整尚未保存为 <b>edit.ply</b>。项目中的裁切和删除记录会继续保留，原始 <b>final.ply</b> 不会被修改。</p>
        </div>
        <div className="gaussian-save-actions">
          <button type="button" className="secondary" disabled={navigationSaving} onClick={() => setPendingNavigation(null)}>取消</button>
          <button type="button" className="secondary" disabled={navigationSaving} onClick={() => void completeNavigation(pendingNavigation)}>暂不保存</button>
          <button type="button" className="primary" disabled={navigationSaving} onClick={() => void saveAndContinue()}>{navigationSaving ? <LoaderCircle className="spin" size={14} /> : <Save size={14} />}保存并继续</button>
        </div>
      </div>
    </div>}
  </section>;
}
