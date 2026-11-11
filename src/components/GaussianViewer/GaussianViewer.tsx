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
import { useMcpExportActivity } from "../../lib/mcpExportExecutor";
import { Camera, GSplat } from "@playcanvas/react/components";
import { useApp, useSplat } from "@playcanvas/react/hooks";
import {
  ADDRESS_CLAMP_TO_EDGE,
  BoundingBox,
  Entity as PcEntity,
  FILTER_LINEAR,
  GAMMA_SRGB,
  GSPLAT_STREAM_INSTANCE,
  type GSplatComponent,
  GSplatResource,
  PIXELFORMAT_RGBA8,
  PIXELFORMAT_R8,
  RenderTarget,
  Texture,
  TONEMAP_LINEAR,
  Vec3,
  WORKBUFFER_UPDATE_ALWAYS,
  WORKBUFFER_UPDATE_AUTO,
  WORKBUFFER_UPDATE_ONCE,
} from "playcanvas";
import {
  ArrowLeft,
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
  Download,
  FileCode2,
} from "lucide-react";
import appLogo from "../../../assets/app-icon.svg";
import {
  beginGaussianHtmlExport,
  cancelGaussianHtmlExport,
  commitGaussianHtmlExport,
  onGaussianHtmlExportProgress,
  beginGaussianVideoExport,
  cancelGaussianVideoExport,
  commitGaussianVideoExport,
  revealFile,
  readGaussianPreviewChunk,
} from "../../lib/backend";
import { ownedBinaryChunk } from "../../lib/binaryChunk";
import { previewAssetUrl as withPreviewAssetRevision } from "../../lib/previewAssetUrl";
import { getCurrentLocale, translate, useI18n, type TranslationKey } from "../../i18n";
import { useGaussianTransformStore } from "../../stores/gaussianTransformStore";
import type {
  GaussianCrop,
  GaussianEditorTool,
  GaussianOrthographicView,
  GaussianTransform,
  GaussianVideoExportResult,
  GaussianVideoExportSession,
  GaussianHtmlView,
  GaussianHtmlExportResult,
  GaussianHtmlExportProgress,
  VideoOrientation,
  TrainingReferenceView,
} from "../../types/pipeline";
import { PLY_TO_ENGINE_ROTATION } from "./CoordinateSystem";
import {
  copyFlippedRgbaRows,
  copyRgbaReadbackRows,
  normalizedCaptureRegion,
  verticalFovForCapture,
  type NormalizedCaptureRegion,
} from "./PreviewCapture";
import { configurePreviewRenderer, previewDeviceTypes } from "./PreviewBackend";
import { PreviewRenderScheduler, requestPreviewRender } from "./PreviewRenderScheduler";
import {
  GAUSSIAN_VIDEO_FRAME_COUNT,
  gaussianVideoDimensions,
  captureGuideSize,
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
  PREVIEW_ANIMATION_WGSL,
  animationEffectsActive,
  animationPhaseAt,
  orbitDegreesAt,
  robustEffectBounds,
  effectRadialLimitForBounds,
  type PreviewAnimationPhase,
} from "./PreviewAnimation";
import { TransformPanel } from "./TransformPanel";
import { SelectionPanel } from "./SelectionPanel";
import { PreviewExportMenu, PreviewOrientationControl } from "./PreviewExportControls";
import { EDITABLE_SPLAT_ASSET_OPTIONS, splatTextureCapacityError } from "./SplatLoadPolicy";
import { createSplatRangeSource, shouldUseSplatRange } from "./SplatRangeSource";
import { ViewerControls, type ViewerCameraState } from "./ViewerControls";
import { readPreviewView, savePreviewView } from "./PreviewViewState";
import { trainingReferenceInWorld } from "./TrainingReferenceView";
import { invertSelectionShortcutLabel, isInvertSelectionShortcut } from "./SelectionShortcuts";
import { InvertSelectionIcon } from "./InvertSelectionIcon";
import { PreviewSortCoordinator } from "./PreviewSortCoordinator";
import { PreviewEditPersistence } from "./PreviewEditPersistence";
import { ModelTransformGizmo } from "./ModelTransformGizmo";
import { modelPivotFromPly, rotateAroundModelCenter, type ModelPivot } from "./ModelTransformMath";
import { readGizmoMode, saveGizmoMode, type GizmoMode } from "./GizmoPreferences";
import { readRoamSpeed, saveRoamSpeed } from "./RoamPreferences";
import { RoamSpeedControl } from "./RoamSpeedControl";
import { PreviewEditFocus } from "./PreviewEditFocus";
import { PlyExportDialog, type PlyExportDialogState } from "./PlyExportDialog";
import { createModelExport, type ModelExportOperation } from "../../lib/modelExport";
import type { ModelFormat } from "../../lib/modelExportTypes";

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
  focusCanvas: () => boolean;
  replay: () => void;
  selectRectangle: (rectangle: SelectionRectangle, selectionMode: RectangleSelectionMode) => Promise<Uint8Array>;
  invertSelection: () => Promise<Uint8Array>;
  freezeCrop: (crop: Exclude<GaussianCrop, null>, deletedMask: Uint8Array) => Promise<Uint8Array>;
  deactivateCrop: () => void;
  alignView: (view: GaussianOrthographicView) => void;
  locateView: () => void;
  stopNavigation: () => void;
  isMouseLookActive: () => boolean;
  initializeCrop: (kind: "sphere" | "box", previous: GaussianCrop) => Exclude<GaussianCrop, null> | null;
  snapshotView: () => GaussianHtmlView;
  saveView: () => boolean;
  modelPivot: () => ModelPivot | null;
  finishGizmo: (cancel?: boolean) => void;
  /** Current view as top-down RGBA pixels, used to draw a reshoot guide. */
  /** Model-space point to image pixels, so the guide can circle the region. */
  exportVideo: (options: {
    signal: AbortSignal;
    onProgress: (progress: GaussianVideoEncodingProgress) => void;
    captureRegion: NormalizedCaptureRegion;
    orientation: VideoOrientation;
  }) => Promise<Uint8Array>;
}

/** Capture before rectangle selection and the canvas' native gizmo handlers. */
export function consumeMouseLookPointer(
  event: Pick<ReactPointerEvent, "preventDefault" | "stopPropagation">,
  navigation: Pick<SplatSceneApi, "isMouseLookActive"> | null,
): boolean {
  if (!navigation?.isMouseLookActive()) return false;
  event.preventDefault();
  event.stopPropagation();
  return true;
}

/** Releasing the cursor must not also clear an existing rectangle selection. */
export function consumeMouseLookEscape(
  event: KeyboardEvent,
  navigation: Pick<SplatSceneApi, "isMouseLookActive" | "stopNavigation"> | null,
): boolean {
  if (event.key !== "Escape" || !navigation?.isMouseLookActive()) return false;
  event.preventDefault();
  event.stopImmediatePropagation();
  navigation.stopNavigation();
  return true;
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
function effectRadialLimit(fullBounds: BoundingBox, effectBounds: BoundingBox) {
  return effectRadialLimitForBounds(
    {center:[fullBounds.center.x,fullBounds.center.y,fullBounds.center.z],halfExtents:[fullBounds.halfExtents.x,fullBounds.halfExtents.y,fullBounds.halfExtents.z]},
    {center:[effectBounds.center.x,effectBounds.center.y,effectBounds.center.z],halfExtents:[effectBounds.halfExtents.x,effectBounds.halfExtents.y,effectBounds.halfExtents.z]},
  );
}

const PreviewCamera = memo(forwardRef<PcEntity>(function PreviewCamera(_props, ref) {
  return <Entity ref={ref} name="OOOSplat Preview Camera" position={INITIAL_CAMERA_POSITION} rotation={IDENTITY_ROTATION} scale={IDENTITY_SCALE}>
    <Camera enabled={false} clearColor="#0e1117" fov={52} nearClip={0.01} farClip={10000} gammaCorrection={GAMMA_SRGB} toneMapping={TONEMAP_LINEAR} />
  </Entity>;
}));

interface SplatSceneProps {
  projectId: string;
  assetUrl: string;
  fileSize: number;
  readToken?: string | null;
  referenceView?: TrainingReferenceView | null;
  splatCount: number;
  transform: GaussianTransform;
  mode: ViewerMode;
  tool: GaussianEditorTool;
  crop: GaussianCrop;
  deletedMask: Uint8Array;
  selectionMask: Uint8Array;
  editorMasksReady: boolean;
  gizmoMode: GizmoMode;
  roamSpeed: number;
  interactionBlocked: boolean;
  onGizmoDragging: (dragging: boolean, cancelled?: boolean) => void;
  onOrthographicViewChange: (view: GaussianOrthographicView | null, orthographic: boolean) => void;
  onStatus: (status: ViewportStatus) => void;
  onAnimationStatus: (status: AnimationStatus) => void;
}

const LoadedSplatScene = forwardRef<SplatSceneApi, SplatSceneProps>(function LoadedSplatScene({ projectId, assetUrl, fileSize, readToken, referenceView, transform, mode, tool, crop, deletedMask, selectionMask, editorMasksReady, gizmoMode, roamSpeed, interactionBlocked, onGizmoDragging, onOrthographicViewChange, onStatus, onAnimationStatus }, ref) {
  const { t } = useI18n();
  const app = useApp();
  const cameraRef = useRef<PcEntity>(null);
  const modelRef = useRef<PcEntity>(null);
  const splatRef = useRef<PcEntity>(null);
  const controlsRef = useRef<ViewerControls | null>(null);
  const persistViewRef = useRef<(() => void) | null>(null);
  const gridRef = useRef<GroundGrid | null>(null);
  const cropOutlineRef = useRef<CropOutline | null>(null);
  const orbitAxisGuideRef = useRef<OrbitAxisGuide | null>(null);
  const selectionRef = useRef<GaussianSelectionController | null>(null);
  const sortCoordinatorRef = useRef<PreviewSortCoordinator | null>(null);
  const renderSchedulerRef = useRef<PreviewRenderScheduler | null>(null);
  useEffect(() => {
    const scheduler = new PreviewRenderScheduler(app);
    renderSchedulerRef.current = scheduler;
    return () => { scheduler.destroy(); if (renderSchedulerRef.current === scheduler) renderSchedulerRef.current = null; };
  }, [app]);
  const appDestroyedRef = useRef(false);
  const contextLostRef = useRef(false);
  const modeRef = useRef<ViewerMode>(mode);
  const exportingRef = useRef(false);
  const animationElapsedRef = useRef(0);
  const lastReportedAtRef = useRef(-1);
  const animationComponentRef = useRef<GSplatComponent | null>(null);
  const animationEffectActiveRef = useRef(false);
  const robustLocalBoundsRef = useRef<BoundingBox | null>(null);
  const gizmoRef = useRef<ModelTransformGizmo | null>(null);
  const gizmoDraggingRef = useRef(false);
  const roamSpeedRef = useRef(roamSpeed);
  roamSpeedRef.current = roamSpeed;
  useEffect(() => { controlsRef.current?.setRoamSpeed(roamSpeed); }, [roamSpeed]);
  const sceneReadyRef = useRef(false);
  const gizmoStateRef = useRef({ mode, tool, gizmoMode, interactionBlocked });
  gizmoStateRef.current = { mode, tool, gizmoMode, interactionBlocked };
  const configureGizmo = useCallback(() => {
    const state = gizmoStateRef.current;
    const available = sceneReadyRef.current && !appDestroyedRef.current && !contextLostRef.current
      && !exportingRef.current && !state.interactionBlocked;
    if (controlsRef.current) controlsRef.current.enabled = available && !gizmoDraggingRef.current;
    controlsRef.current?.setRoamingEnabled(available && !gizmoDraggingRef.current && state.mode === "adjust");
    gizmoRef.current?.configure(state.gizmoMode, available && state.mode === "adjust" && state.tool === "transform");
  }, []);
  useEffect(configureGizmo, [configureGizmo, gizmoMode, interactionBlocked, mode, tool]);
  const editorStateRef = useRef({ crop, mode, tool, deletedMask, selectionMask });
  editorStateRef.current = { crop, mode, tool, deletedMask, selectionMask };
  app.scene.gsplatCentersEnabled = true;
  configurePreviewRenderer(app);
  app.scene.gsplat.colorUpdateAngle = 0;
  useEffect(() => {
    const coordinator = new PreviewSortCoordinator(app, () => renderSchedulerRef.current!.forceFrames());
    sortCoordinatorRef.current = coordinator;
    return () => { coordinator.destroy(); if (sortCoordinatorRef.current === coordinator) sortCoordinatorRef.current = null; };
  }, [app]);
  const rangeSource = useMemo(() => {
    if (!readToken && !shouldUseSplatRange(fileSize)) return null;
    const readRange: typeof fetch | undefined = readToken ? async (_input, init) => {
      const match = /^bytes=(\d+)-(\d+)$/.exec(new Headers(init?.headers).get("Range") ?? "");
      if (!match) return new Response(null, { status: 416 });
      const start = Number(match[1]);
      const end = Number(match[2]);
      const body = ownedBinaryChunk(await readGaussianPreviewChunk(readToken, start, end - start + 1));
      return new Response(body.buffer, { status: 206, headers: { "Content-Range": `bytes ${start}-${start + body.byteLength - 1}/${fileSize}` } });
    } : undefined;
    return createSplatRangeSource(assetUrl, fileSize, readRange);
  }, [assetUrl, fileSize, readToken]);
  const splatAssetOptions = useMemo(() => rangeSource ? {
    data: EDITABLE_SPLAT_ASSET_OPTIONS.data,
    file: {
      size: fileSize,
      contents: rangeSource.response,
    },
  } : EDITABLE_SPLAT_ASSET_OPTIONS, [fileSize, rangeSource]);
  const { asset, loading, error, subscribe } = useSplat(assetUrl, splatAssetOptions);

  useEffect(() => () => rangeSource?.abort(), [rangeSource]);
  const preparedAsset = useMemo(() => {
    if (!asset?.resource) return null;
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
    requestPreviewRender(app);
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
    renderSchedulerRef.current?.setAnimating(mode === "preview" && sceneReadyRef.current && !exportingRef.current);
  }, [mode, replay, setAnimationUniforms, tool]);

  useEffect(() => {
    appDestroyedRef.current = false;
    contextLostRef.current = false;
    const handle = app.on("destroy", () => {
      sceneReadyRef.current = false;
      gizmoRef.current?.destroy();
      gizmoRef.current = null;
      persistViewRef.current?.();
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
    const reportDeviceLost = () => {
      if (contextLostRef.current || appDestroyedRef.current) return;
      contextLostRef.current = true;
      renderSchedulerRef.current?.destroy();
      sceneReadyRef.current = false;
      gizmoRef.current?.destroy(true);
      gizmoRef.current = null;
      sortCoordinatorRef.current?.destroy();
      if (controlsRef.current) controlsRef.current.enabled = false;
      onOrthographicViewChange(null, false);
      onStatus({
        phase: "error",
        progress: 0,
        error: t("viewer.contextLost"),
        renderer,
      });
    };
    const handleWebglContextLost = (event: Event) => {
      event.preventDefault();
      reportDeviceLost();
    };
    const deviceLostHandle = app.graphicsDevice.on("devicelost", reportDeviceLost);
    canvas.addEventListener("webglcontextlost", handleWebglContextLost);
    return () => {
      deviceLostHandle.off();
      canvas.removeEventListener("webglcontextlost", handleWebglContextLost);
    };
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
    component.setParameter("uOoosplatCropInverted", currentCrop?.inverted ? 1 : 0);
    component.setParameter("uOoosplatShowSelection", currentMode === "adjust" && currentTool === "rectangle" ? 1 : 0);
    if (!animationEffectActiveRef.current) component.workBufferUpdate = WORKBUFFER_UPDATE_ONCE;
    requestPreviewRender(app);
  }, [app]);

  useEffect(() => { applyEditorUniforms(); }, [applyEditorUniforms, crop, mode, tool]);
  useEffect(() => {
    cropOutlineRef.current?.setCrop(crop);
    cropOutlineRef.current?.setVisible(mode === "adjust" && (tool === "sphere" || tool === "box") && crop !== null);
    requestPreviewRender(app);
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
      requestPreviewRender(app);
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
    if (error) onStatus({
      phase: "error",
      progress: 0,
      error: rangeSource ? t("viewer.rangeRead", { detail: error }) : error,
      renderer,
    });
    else if (asset) onStatus({ phase: "mounting", progress: 1, error: null, renderer });
    else if (loading) onStatus({ phase: "loading", progress: 0, error: null, renderer });
  }, [asset, error, loading, onStatus, rangeSource, renderer, t]);

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
    gizmoRef.current?.sync();
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
    if (!editorMasksReady || !preparedAsset?.resource || !cameraRef.current || !modelRef.current || !splatRef.current) return;
    const component = splatRef.current.gsplat;
    if (!component) return;
    animationComponentRef.current = component;
    animationEffectActiveRef.current = false;
    robustLocalBoundsRef.current = null;
    component.setWorkBufferModifier({ glsl: PREVIEW_ANIMATION_GLSL, wgsl: PREVIEW_ANIMATION_WGSL });
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
    component.setParameter("uOoosplatCropInverted", 0);
    let ready = false;
    sceneReadyRef.current = false;
    const persistView = () => {
      if (ready && !exportingRef.current && !contextLostRef.current) savePreviewView(projectId, controls.snapshot());
    };
    const controls = new ViewerControls(app.graphicsDevice.canvas, cameraRef.current, onOrthographicViewChange, t("viewer.cameraUnavailable"), persistView, () => { requestPreviewRender(app); });
    controls.enabled = false;
    controls.setRoamSpeed(roamSpeedRef.current);
    persistViewRef.current = persistView;
    window.addEventListener("pagehide", persistView);
    controls.setRectangleSelectionMode(modeRef.current === "adjust" && editorStateRef.current.tool === "rectangle");
    controlsRef.current = controls;
    const source = preparedAsset.resource as GSplatResource;
    const selection = new GaussianSelectionController(
      app.graphicsDevice,
      component,
      cameraRef.current.camera!,
      source.numSplats,
      () => { requestPreviewRender(app); },
    );
    selectionRef.current = selection;
    // Update camera/uniforms before auxiliary prerender listeners capture lines.
    const updateHandle = app.on("prerender", () => {
      if (!ready || exportingRef.current || modeRef.current !== "preview") return;
      const previous = animationElapsedRef.current;
      const elapsed = previous + (renderSchedulerRef.current?.animationDelta() ?? 0);
      animationElapsedRef.current = elapsed;
      setAnimationUniforms(true, elapsed);
      controls.orbitBy((elapsed - previous) * ORBIT_DEGREES_PER_SECOND);
      reportAnimation(elapsed);
    });
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
    const pivot = modelPivotFromPly(robustLocalBoundsRef.current!.center);
    const currentEditor = () => {
      const state = useGaussianTransformStore.getState();
      return !appDestroyedRef.current && state.descriptor?.projectId === projectId ? state : null;
    };
    const gizmo = new ModelTransformGizmo(app, cameraRef.current.camera!, pivot, {
      current: () => currentEditor()?.transform ?? null,
      beforeStart: () => {
        if (!currentEditor()) return;
        if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
        currentEditor()?.commitTransaction();
      },
      begin: () => { currentEditor()?.beginTransaction(); controls.enabled = false; },
      change: next => currentEditor()?.setTransformLive(next),
      finish: cancel => {
        if (cancel) currentEditor()?.cancelTransaction();
        else currentEditor()?.commitTransaction();
        if (cancel && currentEditor()) onGizmoDragging(false, true);
      },
      dragging: active => {
        gizmoDraggingRef.current = active;
        controls.enabled = !active && sceneReadyRef.current && !gizmoStateRef.current.interactionBlocked
          && !exportingRef.current && !contextLostRef.current && !appDestroyedRef.current;
        if (currentEditor()) onGizmoDragging(active);
      },
    });
    gizmoRef.current = gizmo;
    applyEditorUniforms();
    setAnimationUniforms(modeRef.current === "preview", animationElapsedRef.current);

    const initialization = new AbortController();
    void (async () => {
      await selection.applyMasks(editorStateRef.current.deletedMask, editorStateRef.current.selectionMask);
      initialization.signal.throwIfAborted();
      if (!fit(true)) throw new Error(t("viewer.invalidBounds"));
      const savedView = readPreviewView(projectId);
      if (savedView) controls.restore(savedView);
      cameraRef.current!.camera!.enabled = true;
      await sortCoordinatorRef.current!.waitForFrame(cameraRef.current!.camera!, initialization.signal, t("video.sortTimeout"));
      initialization.signal.throwIfAborted();
      ready = true;
      sceneReadyRef.current = true;
      renderSchedulerRef.current?.setAnimating(modeRef.current === "preview");
      controls.enabled = true;
      configureGizmo();
      if (modeRef.current === "preview") replay();
      onStatus({ phase: "ready", progress: 1, error: null, renderer });
    })().catch((error: unknown) => {
      if (!initialization.signal.aborted && !contextLostRef.current && !appDestroyedRef.current) {
        onStatus({ phase: "error", progress: 0, error: error instanceof Error ? error.message : String(error), renderer });
      }
    });

    return () => {
      initialization.abort();
      sceneReadyRef.current = false;
      renderSchedulerRef.current?.setAnimating(false);
      gizmo.destroy();
      if (gizmoRef.current === gizmo) gizmoRef.current = null;
      updateHandle.off();
      persistView();
      if (!appDestroyedRef.current && cameraRef.current?.camera) cameraRef.current.camera.enabled = false;
      window.removeEventListener("pagehide", persistView);
      if (persistViewRef.current === persistView) persistViewRef.current = null;
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
        component.deleteParameter("uOoosplatCropInverted");
        component.deleteParameter("uOoosplatShowSelection");
      }
      if (animationComponentRef.current === component) animationComponentRef.current = null;
      animationEffectActiveRef.current = false;
      robustLocalBoundsRef.current = null;
    };
  }, [app, applyEditorUniforms, configureGizmo, editorMasksReady, fit, onGizmoDragging, onOrthographicViewChange, onStatus, preparedAsset, projectId, renderer, replay, reportAnimation, setAnimationUniforms, syncSceneBounds]);

  const exportVideo = useCallback(async ({
    signal,
    onProgress,
    captureRegion,
    orientation,
  }: {
    signal: AbortSignal;
    onProgress: (progress: GaussianVideoEncodingProgress) => void;
    captureRegion: NormalizedCaptureRegion;
    orientation: VideoOrientation;
  }) => {
    const controls = controlsRef.current;
    const cameraEntity = cameraRef.current;
    if (!controls || !cameraEntity?.camera) throw new Error(t("viewer.cameraNotReady"));
    if (exportingRef.current) throw new Error(t("video.exportBusy"));

    const graphicsDevice = app.graphicsDevice;
    const { width, height } = gaussianVideoDimensions(orientation);
    if (graphicsDevice.maxTextureSize < Math.max(width, height)) {
      throw new Error(t("export.textureCapacity", { maximum: graphicsDevice.maxTextureSize }));
    }
    const readbackPixels = new Uint8Array(width * height * 4);
    let frameImageData: ImageData | null = null;
    const cameraState: ViewerCameraState = controls.snapshot();
    const elapsed = animationElapsedRef.current;
    const previousRenderTarget = cameraEntity.camera.renderTarget;
    const previousFov = cameraEntity.camera.fov;
    const previousOrthoHeight = cameraEntity.camera.orthoHeight;
    const previousEnabled = controls.enabled;
    const previousGridVisible = gridRef.current?.isVisible ?? false;
    const previousOrbitAxisVisible = orbitAxisGuideRef.current?.isVisible ?? false;
    const logo = await loadWatermarkLogo(appLogo);
    const outputCanvas = document.createElement("canvas");
    outputCanvas.width = width;
    outputCanvas.height = height;
    const videoTexture = new Texture(graphicsDevice, {
      name: "OOOSplat Video",
      width,
      height,
      format: PIXELFORMAT_RGBA8,
      mipmaps: false,
      minFilter: FILTER_LINEAR,
      magFilter: FILTER_LINEAR,
      addressU: ADDRESS_CLAMP_TO_EDGE,
      addressV: ADDRESS_CLAMP_TO_EDGE,
    });
    const videoRenderTarget = new RenderTarget({
      name: "OOOSplat Video Target",
      colorBuffer: videoTexture,
      depth: true,
      samples: 1,
    });

    exportingRef.current = true;
    renderSchedulerRef.current?.setAnimating(false);
    configureGizmo();
    controls.enabled = false;
    gridRef.current?.setVisible(false);
    orbitAxisGuideRef.current?.setVisible(false);
    cameraEntity.camera.renderTarget = videoRenderTarget;
    cameraEntity.camera.fov = verticalFovForCapture(previousFov, captureRegion.height);
    cameraEntity.camera.orthoHeight = previousOrthoHeight * captureRegion.height;
    setAnimationUniforms(true, 0);

    try {
      return await encodeGaussianVideo({
        canvas: outputCanvas,
        logo,
        signal,
        onProgress,
        orientation,
        renderFrameAt: async (timeSeconds, context) => {
          signal.throwIfAborted();
          animationElapsedRef.current = timeSeconds;
          setAnimationUniforms(true, timeSeconds);
          controls.restore({ ...cameraState, fov: verticalFovForCapture(previousFov, captureRegion.height),
            projectionOffset: cameraState.projectionOffset?.map(value => value / captureRegion.height) as [number, number] | undefined });
          cameraEntity.camera!.orthoHeight = previousOrthoHeight * captureRegion.height;
          controls.setOrbitYaw(cameraState.yaw + orbitDegreesAt(timeSeconds));
          await sortCoordinatorRef.current!.waitForFrame(cameraEntity.camera!, signal, t("video.sortTimeout"));
          signal.throwIfAborted();
          app.render();
          const pixels = await videoTexture.read(
            0,
            0,
            width,
            height,
            { data: readbackPixels, immediate: true, renderTarget: videoRenderTarget },
          );
          signal.throwIfAborted();
          frameImageData ??= context.createImageData(width, height);
          copyRgbaReadbackRows(
            pixels as Uint8Array,
            frameImageData.data,
            width,
            height,
            graphicsDevice.isWebGL2,
          );
          context.putImageData(frameImageData, 0, 0);
        },
      });
    } finally {
      exportingRef.current = false;
      renderSchedulerRef.current?.setAnimating(modeRef.current === "preview");
      configureGizmo();
      animationElapsedRef.current = elapsed;
      cameraEntity.camera.renderTarget = previousRenderTarget;
      cameraEntity.camera.fov = previousFov;
      cameraEntity.camera.orthoHeight = previousOrthoHeight;
      controls.restore(cameraState);
      controls.enabled = previousEnabled;
      gridRef.current?.setVisible(previousGridVisible && modeRef.current === "adjust");
      orbitAxisGuideRef.current?.setVisible(previousOrbitAxisVisible && modeRef.current === "preview");
      videoTexture.destroy();
      videoRenderTarget.destroy();
      setAnimationUniforms(modeRef.current === "preview", elapsed);
      reportAnimation(elapsed, true);
      requestPreviewRender(app);
    }
  }, [app, configureGizmo, reportAnimation, setAnimationUniforms]);

  const selectRectangle = useCallback((rectangle: SelectionRectangle, selectionMode: RectangleSelectionMode) => {
    const selection = selectionRef.current;
    if (!selection) return Promise.reject(new Error(t("viewer.selectorNotReady")));
    if (contextLostRef.current) return Promise.reject(new Error(t("viewer.contextReload")));
    return selection.select(rectangle, selectionMode, editorStateRef.current.crop);
  }, []);

  const freezeCrop = useCallback((currentCrop: Exclude<GaussianCrop, null>, currentDeletedMask: Uint8Array) => {
    const selection = selectionRef.current;
    if (!selection) return Promise.reject(new Error(t("viewer.selectorNotReady")));
    if (contextLostRef.current) return Promise.reject(new Error(t("viewer.contextReload")));
    return selection.freezeCrop(currentCrop, currentDeletedMask);
  }, []);

  const invertSelection = useCallback(async () => {
    const selection = selectionRef.current;
    if (!selection || contextLostRef.current) throw new Error(translate(getCurrentLocale(), "viewer.selectorNotReady"));
    const current = useGaussianTransformStore.getState();
    await selection.applyMasks(current.deletedMask, current.selectionMask);
    return selection.invertSelection();
  }, []);

  const deactivateCrop = useCallback(() => {
    const component = animationComponentRef.current;
    if (!component || appDestroyedRef.current) return;
    editorStateRef.current = { ...editorStateRef.current, crop: null, tool: "transform" };
    component.setParameter("uOoosplatCropInverted", 0);
    component.setParameter("uOoosplatCropKind", 0);
    component.setParameter("uOoosplatCropCenter", [0, 0, 0]);
    component.setParameter("uOoosplatCropSize", [1, 1, 1]);
    component.setParameter("uOoosplatCropRadius", 1);
    component.workBufferUpdate = WORKBUFFER_UPDATE_ONCE;
    cropOutlineRef.current?.setCrop(null);
    cropOutlineRef.current?.setVisible(false);
    requestPreviewRender(app);
  }, [app]);

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

  const locateView = useCallback(() => {
    // Reuse robust bounds for the orbit distance, without changing the actual
    // training camera pose or rescanning the model when Locate is clicked.
    const bounds = transformedEffectBounds();
    if (!referenceView || !bounds || !sceneReadyRef.current || gizmoStateRef.current.interactionBlocked) return;
    const current = useGaussianTransformStore.getState();
    controlsRef.current?.locate(trainingReferenceInWorld(referenceView, current.transform), bounds);
    requestPreviewRender(app);
  }, [app, referenceView, transformedEffectBounds]);

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

  const snapshotView = useCallback((): GaussianHtmlView => {
    const camera = cameraRef.current?.camera;
    if (!camera || !controlsRef.current) throw new Error(t("viewer.cameraNotReady"));
    return { ...controlsRef.current.snapshot(), fov: camera.fov, nearClip: camera.nearClip, farClip: camera.farClip };
  }, []);
  const saveView = useCallback(() => {
    const controls = controlsRef.current;
    return Boolean(controls && !exportingRef.current && !contextLostRef.current && savePreviewView(projectId, controls.snapshot()));
  }, [projectId]);
  const modelPivot = useCallback(() => robustLocalBoundsRef.current ? modelPivotFromPly(robustLocalBoundsRef.current.center) : null, []);
  const finishGizmo = useCallback((cancel = false) => gizmoRef.current?.finish(cancel), []);
  const stopNavigation = useCallback(() => controlsRef.current?.stopNavigation(), []);
  const isMouseLookActive = useCallback(() => controlsRef.current?.isMouseLookActive() ?? false, []);
  const focusCanvas = useCallback(() => {
    const state = gizmoStateRef.current;
    if (state.mode !== "adjust" || state.interactionBlocked || gizmoDraggingRef.current
      || !sceneReadyRef.current || appDestroyedRef.current || contextLostRef.current || exportingRef.current) return false;
    return controlsRef.current?.focus() ?? false;
  }, []);
  useImperativeHandle(ref, () => ({ focusCanvas, replay, exportVideo, snapshotView, saveView, modelPivot, finishGizmo, selectRectangle, invertSelection, freezeCrop, deactivateCrop, alignView, locateView, stopNavigation, isMouseLookActive, initializeCrop }), [focusCanvas, alignView, deactivateCrop, exportVideo, finishGizmo, freezeCrop, initializeCrop, invertSelection, locateView, isMouseLookActive, modelPivot, replay, saveView, selectRectangle, snapshotView, stopNavigation]);

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
  const { locale } = useI18n();
  const app = useApp();
  const maximumTextureSide = app.graphicsDevice.maxTextureSize;
  const renderer = `${app.graphicsDevice.deviceType.toUpperCase()} / UNIFIED GSPLAT`;
  const capacityError = splatTextureCapacityError(props.splatCount, maximumTextureSide, locale);

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

const formatBytes = (bytes: number, locale: string) => {
  const [value, unit, digits] = bytes >= 1024 ** 3
    ? [bytes / 1024 ** 3, "GB", 2] as const
    : [bytes / 1024 ** 2, "MB", 1] as const;
  return `${new Intl.NumberFormat(locale, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(value)} ${unit}`;
};
const compact = (values: number[]) => values.map((value) => Number(value.toFixed(2))).join(" / ");
const phaseLabelKeys: Record<PreviewAnimationPhase, TranslationKey> = {
  reveal: "animation.reveal",
  shockwave: "animation.shockwave",
  orbit: "animation.orbit",
};

export function GaussianViewer({ previewSessionId, onExit, onDisposed, pipelineRunning }: {
  previewSessionId: number;
  onExit: () => void | Promise<void>;
  onDisposed: (projectId: string, previewSessionId: number) => void;
  pipelineRunning: boolean;
}) {
  const { locale, t, formatNumber } = useI18n();
  const store = useGaussianTransformStore();
  const sceneApiRef = useRef<SplatSceneApi | null>(null);
  const captureGuideRef = useRef<HTMLDivElement | null>(null);
  const persistenceRef = useRef<PreviewEditPersistence | null>(null);
  const selectionQueue = useRef<Promise<void>>(Promise.resolve());
  const videoAbortRef = useRef<AbortController | null>(null);
  const videoSessionRef = useRef<GaussianVideoExportSession | null>(null);
  const htmlSessionRef = useRef<GaussianVideoExportSession | null>(null);
  const htmlCancelRef = useRef(false);
  const exportBusyRef = useRef(false);
  const viewportElementRef = useRef<HTMLDivElement | null>(null);
  const [mode, setMode] = useState<ViewerMode>("adjust");
  const [gizmoMode, setGizmoMode] = useState<GizmoMode>(readGizmoMode);
  const [gizmoDragging, setGizmoDragging] = useState(false);
  const [orthographicView, setOrthographicView] = useState<GaussianOrthographicView | null>(null);
  const [orthographicProjection, setOrthographicProjection] = useState(false);
  const onViewDirectionChange = useCallback((view: GaussianOrthographicView | null, orthographic: boolean) => {
    setOrthographicView(view); setOrthographicProjection(orthographic);
  }, []);
  const [selectionDrag, setSelectionDrag] = useState<null | { pointerId: number; startX: number; startY: number; x: number; y: number; selectionMode: RectangleSelectionMode }>(null);
  const [selectionBusy, setSelectionBusy] = useState(false);
  const [roamSpeed, setRoamSpeed] = useState(readRoamSpeed);
  const editFocusRef = useRef<PreviewEditFocus | null>(null);
  const gizmoFocusRef = useRef<(() => void) | null>(null);
  const rectangleFocusRef = useRef<(() => void) | null>(null);
  const [viewport, setViewport] = useState<ViewportStatus>(INITIAL_VIEWPORT);
  const [rendererRevision, setRendererRevision] = useState(0);
  const [loadedMaskKey, setLoadedMaskKey] = useState<string | null>(null);
  const [savingEdits, setSavingEdits] = useState(false);
  const [saveNotice, setSaveNotice] = useState<"saved" | "viewFailed" | null>(null);
  const [gaussianExportDialog, setGaussianExportDialog] = useState<PlyExportDialogState>({ phase: "closed" });
  const [gaussianExportProgress, setGaussianExportProgress] = useState<number | null>(0);
  const [modelFormat, setModelFormat] = useState<ModelFormat>("ply");
  const modelOperationRef = useRef<ModelExportOperation | null>(null);
  const modelAttemptRef = useRef(0);
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
  const [orientation, setOrientation] = useState<VideoOrientation>("portrait");
  const [guideSize, setGuideSize] = useState({width:0,height:0});
  const [htmlConfirm, setHtmlConfirm] = useState(false);
  const [htmlBusy, setHtmlBusy] = useState(false);
  const [htmlProgress, setHtmlProgress] = useState<GaussianHtmlExportProgress | null>(null);
  const [htmlResult, setHtmlResult] = useState<GaussianHtmlExportResult | null>(null);
  const [htmlError, setHtmlError] = useState<string | null>(null);
  const [pendingNavigation, setPendingNavigation] = useState<"preview" | "exit" | null>(null);
  const [navigationSaving, setNavigationSaving] = useState(false);
  const [cropFreezing, setCropFreezing] = useState(false);
  const [cropFreezeError, setCropFreezeError] = useState<string | null>(null);
  const previewAssetUrl = useMemo(() => {
    if (!store.descriptor) return "";
    return withPreviewAssetRevision(store.descriptor.assetUrl, "retry", rendererRevision.toString());
  }, [rendererRevision, store.descriptor]);

  const onStatus = useCallback((status: ViewportStatus) => setViewport(status), []);
  const onAnimationStatus = useCallback((status: AnimationStatus) => setAnimationStatus(status), []);
  const gaussianExporting = !["closed", "completed", "error"].includes(gaussianExportDialog.phase);
  const mcpExporting = useMcpExportActivity(state => state.projectId !== null && state.projectId === store.descriptor?.projectId);
  const localOperationBusy = savingEdits || navigationSaving || pendingNavigation !== null || htmlBusy || htmlConfirm || cropFreezing || gaussianExporting || !["idle", "completed", "error"].includes(videoPhase);
  const editorBusy = localOperationBusy || gizmoDragging || selectionBusy || selectionDrag !== null;
  useEffect(() => {
    const projectId = store.descriptor?.projectId ?? null;
    useMcpExportActivity.setState({ editorProjectId: projectId, editorBusy });
    return () => {
      if (useMcpExportActivity.getState().editorProjectId === projectId) useMcpExportActivity.setState({ editorProjectId: null, editorBusy: false });
    };
  }, [store.descriptor?.projectId, editorBusy]);
  const operationBusy = localOperationBusy || mcpExporting;
  const busy = operationBusy || gizmoDragging || selectionBusy;
  const interactionBlocked = operationBusy || selectionBusy || viewport.phase !== "ready" || cropFreezeError !== null;
  const focusAllowedRef = useRef(false);
  focusAllowedRef.current = mode === "adjust" && !interactionBlocked && !gizmoDragging && selectionDrag === null;
  useEffect(() => {
    const focus = new PreviewEditFocus(() => sceneApiRef.current?.focusCanvas() ?? false, () => focusAllowedRef.current);
    editFocusRef.current = focus;
    return () => { focus.destroy(); if (editFocusRef.current === focus) editFocusRef.current = null; };
  }, [store.descriptor?.projectId, rendererRevision]);
  useEffect(() => { editFocusRef.current?.flush(); }, [mode, interactionBlocked, gizmoDragging, selectionDrag, viewport.phase]);
  const beginFocusReturn = useCallback(() => editFocusRef.current?.begin() ?? (() => {}), []);
  const returnCanvasFocus = useCallback(() => { beginFocusReturn()(); }, [beginFocusReturn]);
  const onGizmoDragging = useCallback((dragging: boolean, cancelled = false) => {
    if (dragging) gizmoFocusRef.current = beginFocusReturn();
    else { gizmoFocusRef.current?.(); gizmoFocusRef.current = null; }
    setGizmoDragging(dragging);
    if (cancelled) void persistenceRef.current?.flush().catch(() => undefined);
  }, [beginFocusReturn]);

  useEffect(() => {
    const projectId = store.descriptor?.projectId;
    if (!projectId) return;
    const persistence = new PreviewEditPersistence(projectId);
    persistenceRef.current = persistence;
    setGaussianExportResult(null);
    setSelectionBusy(false);
    setSelectionDrag(null);
    selectionQueue.current = Promise.resolve();
    rectangleFocusRef.current = null;
    gizmoFocusRef.current = null;
    setGaussianExportDialog({ phase: "closed" });
    setPendingNavigation(null);
    setSaveNotice(null);
    return () => {
      modelAttemptRef.current += 1;
      void modelOperationRef.current?.cancel().catch(() => undefined);
      modelOperationRef.current = null;
      persistence.destroy(); if (persistenceRef.current === persistence) persistenceRef.current = null;
    };
  }, [store.descriptor?.projectId]);
  useEffect(() => { setSaveNotice(null); }, [store.revision]);

  useEffect(() => {
    let active = true;
    setVideoCapability(INITIAL_VIDEO_CAPABILITY);
    void checkGaussianVideoCapability(orientation).then((capability) => {
      if (active) setVideoCapability({ ...capability, checking: false });
    });
    return () => { active = false; };
  }, [locale, orientation]);

  useEffect(() => {
    const element = viewportElementRef.current;
    if (!element) return;
    const update = () => setGuideSize(captureGuideSize(element.clientWidth, element.clientHeight, orientation));
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, [orientation]);

  useEffect(() => {
    let disposed = false;
    let unlisten: undefined | (() => void);
    void onGaussianHtmlExportProgress((event) => {
      if (event.exportId === htmlSessionRef.current?.exportId) setHtmlProgress(event);
    }).then(fn => { if (disposed) fn(); else unlisten = fn; });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    const descriptor = store.descriptor;
    if (!descriptor) return;
    const maskKey = descriptor.editMaskAssetUrl ? `${descriptor.projectId}:${descriptor.editMaskAssetUrl}` : descriptor.projectId;
    if (loadedMaskKey === maskKey) return;
    let active = true;
    if (!descriptor.editMaskAssetUrl) {
      store.setInitialDeletedMask(new Uint8Array(store.deletedMask.length));
      setLoadedMaskKey(descriptor.projectId);
      return;
    }
    void fetch(descriptor.editMaskAssetUrl)
      .then((response) => {
        if (!response.ok) throw new Error(t("viewer.maskRead", { status: response.status }));
        return response.arrayBuffer();
      })
      .then((buffer) => {
        if (active) {
          const current = useGaussianTransformStore.getState();
          current.setInitialDeletedMask(new Uint8Array(buffer));
          current.setSaveState("saved");
          setLoadedMaskKey(`${descriptor.projectId}:${descriptor.editMaskAssetUrl}`);
        }
      })
      .catch((error: unknown) => {
        if (active) {
          const message = error instanceof Error ? error.message : String(error);
          useGaussianTransformStore.getState().setSaveState("error", message);
          setViewport(current => ({ ...current, phase: "error", error: message }));
        }
      });
    return () => { active = false; };
  }, [loadedMaskKey, rendererRevision, store.descriptor?.editMaskAssetUrl, store.descriptor?.projectId]);
  useEffect(() => {
    if (!saveNotice) return;
    const timeout = window.setTimeout(() => setSaveNotice(null), 4_000);
    return () => window.clearTimeout(timeout);
  }, [saveNotice]);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      setViewport((current) => current.phase === "initializing" ? {
        phase: "error",
        progress: 0,
        error: t("viewer.initTimeout"),
        renderer: "WEBGL2 / UNIFIED GSPLAT",
      } : current);
    }, 10_000);
    return () => window.clearTimeout(timeout);
  }, [rendererRevision, store.descriptor?.projectId]);

  useEffect(() => () => {
    videoAbortRef.current?.abort();
    const session = videoSessionRef.current;
    if (session) void cancelGaussianVideoExport(session.exportId);
    htmlCancelRef.current = true;
    if (htmlSessionRef.current) void cancelGaussianHtmlExport(htmlSessionRef.current.exportId);
  }, []);

  useEffect(() => {
    const projectId = store.descriptor?.projectId;
    if (!projectId) return;
    return () => {
      window.setTimeout(() => onDisposed(projectId, previewSessionId), 0);
    };
  }, [onDisposed, previewSessionId, store.descriptor?.projectId]);

  const flushEdits = async (retry = false, notify = true) => {
    sceneApiRef.current?.finishGizmo();
    const projectId = useGaussianTransformStore.getState().descriptor?.projectId;
    if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
    await selectionQueue.current;
    const current = useGaussianTransformStore.getState();
    current.commitTransaction(); current.commitCropTransaction();
    if (!persistenceRef.current) throw new Error(t("viewer.editsUnsaved"));
    await persistenceRef.current.flush(retry);
    if (useGaussianTransformStore.getState().descriptor?.projectId !== projectId) throw new Error(t("viewer.editsUnsaved"));
    const viewSaved = sceneApiRef.current?.saveView() ?? false;
    if (notify) setSaveNotice(viewSaved ? "saved" : "viewFailed");
  };
  const saveEdits = async (retry = false) => {
    if (busy || exportBusyRef.current || viewport.phase !== "ready") return;
    const focus = beginFocusReturn();
    exportBusyRef.current = true;
    setSavingEdits(true); setSaveNotice(null);
    try { await flushEdits(retry); focus(); }
    catch (error) { useGaussianTransformStore.getState().setSaveState("error", error instanceof Error ? error.message : String(error)); }
    finally { exportBusyRef.current = false; setSavingEdits(false); }
  };
  const exportGaussian = async (format: ModelFormat, retry = false) => {
    if (!store.descriptor || busy || exportBusyRef.current || (retry && gaussianExportDialog.phase !== "error")) return;
    exportBusyRef.current = true;
    setGaussianExportDialog({ phase: "preparing" });
    setGaussianExportProgress(0);
    setGaussianExportResult(null);
    setSaveNotice(null);
    const attempt = ++modelAttemptRef.current;
    setModelFormat(format);
    try {
      await flushEdits(retry, false);
      if (attempt !== modelAttemptRef.current) return;
      const current = useGaussianTransformStore.getState();
      const operation = createModelExport(store.descriptor.projectId, current.transform, current.editing.revision, format, (event) => {
        if (attempt !== modelAttemptRef.current) return;
        setGaussianExportProgress(event.progress);
        setGaussianExportDialog(event.phase === "awaitingConfirmation"
          ? { phase: event.phase, exportId: event.exportId, confirmationId: event.confirmationId, adjustment: event.adjustment }
          : { phase: event.phase });
      });
      modelOperationRef.current = operation;
      const result = await operation.export();
      if (attempt !== modelAttemptRef.current) return;
      setGaussianExportResult(result.path);
      setGaussianExportDialog({ phase: "completed", path: result.path, fileSize: result.fileSize, coordinateAdjustment: result.coordinateAdjustment });
    } catch (error) {
      if (attempt === modelAttemptRef.current) {
        modelOperationRef.current = null; exportBusyRef.current = false;
        setGaussianExportDialog({ phase: "error", error: error instanceof Error ? error.message : String(error) });
      }
    } finally {
      if (attempt === modelAttemptRef.current) { modelOperationRef.current = null; exportBusyRef.current = false; }
    }
  };
  const cancelModelExport = async () => {
    const operation = modelOperationRef.current;
    const attempt = ++modelAttemptRef.current;
    setGaussianExportDialog({ phase: "cancelling" });
    try { await operation?.cancel(); if (attempt === modelAttemptRef.current) setGaussianExportDialog({ phase: "closed" }); }
    catch (error) { if (attempt === modelAttemptRef.current) setGaussianExportDialog({ phase: "error", error: String(error) }); }
    finally { if (attempt === modelAttemptRef.current) { modelOperationRef.current = null; exportBusyRef.current = false; } }
  };
  const closeGaussianExportDialog = () => {
    if (!gaussianExporting) setGaussianExportDialog({ phase: "closed" });
  };
  const revealGaussianExport = async (path: string, keepDialogOpen = true) => {
    try {
      await revealFile(path);
      if (keepDialogOpen) setGaussianExportDialog(previous => ({ ...previous, phase: "completed", path, revealError: undefined }));
    } catch (error) {
      setGaussianExportDialog(previous => ({
        ...previous,
        phase: "completed",
        path,
        revealError: error instanceof Error ? error.message : String(error),
      }));
    }
  };

  const exportVideo = async () => {
    if (!store.descriptor || !sceneApiRef.current || busy || exportBusyRef.current || !videoCapability.supported) return;
    const guide = captureGuideRef.current;
    const canvas = guide?.parentElement?.querySelector("canvas");
    if (!guide || !(canvas instanceof HTMLCanvasElement)) {
      setVideoError(t("viewer.captureUnavailable"));
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
    exportBusyRef.current = true;
    setVideoError(null);
    setVideoResult(null);
    setVideoProgress({ phase: "rendering", currentFrame: 0, totalFrames: GAUSSIAN_VIDEO_FRAME_COUNT, progress: 0 });
    const abortController = new AbortController();
    videoAbortRef.current = abortController;
    let committed = false;
    try {
      await flushEdits();
      const current = useGaussianTransformStore.getState();
      if (current.saveState === "error") throw new Error(current.saveError ?? t("viewer.editsUnsaved"));
      abortController.signal.throwIfAborted();
      const session = await beginGaussianVideoExport(store.descriptor.projectId, orientation, current.editing.revision);
      videoSessionRef.current = session;
      setVideoPhase("rendering");
      const bytes = await sceneApiRef.current.exportVideo({
        signal: abortController.signal,
        captureRegion,
        orientation,
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
      exportBusyRef.current = false;
    }
  };

  const cancelVideo = () => {
    videoAbortRef.current?.abort();
    if(videoSessionRef.current)void cancelGaussianVideoExport(videoSessionRef.current.exportId).catch((error:unknown)=>setVideoError(String(error)));
  };
  const cancelHtml = () => {
    htmlCancelRef.current = true;
    if (htmlSessionRef.current) void cancelGaussianHtmlExport(htmlSessionRef.current.exportId).catch((error: unknown) => setHtmlError(String(error)));
  };
  const exportHtml = async () => {
    if (!store.descriptor || !sceneApiRef.current || exportBusyRef.current) return;
    exportBusyRef.current = true;
    htmlCancelRef.current = false;
    setHtmlConfirm(false); setHtmlBusy(true); setHtmlError(null); setHtmlResult(null);
    setHtmlProgress(null);
    try {
      await flushEdits();
      const current = useGaussianTransformStore.getState();
      if (current.saveState === "error") throw new Error(current.saveError ?? t("viewer.editsUnsaved"));
      if (htmlCancelRef.current) return;
      const session = await beginGaussianHtmlExport(store.descriptor.projectId, current.editing.revision, sceneApiRef.current.snapshotView(), locale);
      htmlSessionRef.current = session;
      if (htmlCancelRef.current) { await cancelGaussianHtmlExport(session.exportId); return; }
      const result = await commitGaussianHtmlExport(session.exportId);
      setHtmlResult(result);
    } catch (error) {
      if (!htmlCancelRef.current) setHtmlError(error instanceof Error ? error.message : String(error));
    } finally {
      if (htmlSessionRef.current) await cancelGaussianHtmlExport(htmlSessionRef.current.exportId).catch(() => undefined);
      htmlSessionRef.current = null; exportBusyRef.current = false; setHtmlBusy(false);
    }
  };
  const retry = () => {
    setSelectionBusy(false);
    setSelectionDrag(null);
    selectionQueue.current = Promise.resolve();
    setViewport(INITIAL_VIEWPORT);
    setRendererRevision((value) => value + 1);
  };
  const retrySave = () => {
    void saveEdits(true);
  };
  const runHistory = (direction: "undo" | "redo") => {
    const focus = beginFocusReturn();
    if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
    const previousTool = useGaussianTransformStore.getState().tool;
    useGaussianTransformStore.getState()[direction]();
    const next = useGaussianTransformStore.getState();
    if (next.tool !== previousTool && (next.tool === "sphere" || next.tool === "box")) {
      setOrthographicView("side");
      requestAnimationFrame(() => sceneApiRef.current?.alignView("side"));
    }
    focus();
  };
  const requestNavigation = async (target: "preview" | "exit", retry = false) => {
    if (exportBusyRef.current) return;
    exportBusyRef.current = true;
    setNavigationSaving(true);
    try {
      const current = useGaussianTransformStore.getState();
      if (target === "exit" && viewport.phase !== "ready" && current.revision === 0
        && !current.transactionStart && current.cropTransactionStart === undefined) {
        await onExit();
        return;
      }
      await flushEdits(retry);
      setPendingNavigation(null);
      if (target === "preview") { setMode("preview"); setVideoError(null); setVideoPhase("idle"); }
      else await onExit();
    } catch (error) {
      useGaussianTransformStore.getState().setSaveState("error", error instanceof Error ? error.message : String(error));
      setPendingNavigation(target);
    } finally { exportBusyRef.current = false; setNavigationSaving(false); }
  };
  const switchMode = (nextMode: ViewerMode) => {
    if (busy || nextMode === mode) return;
    if (nextMode === "preview") {
      void requestNavigation("preview");
      return;
    }
    setMode("adjust");
    setVideoError(null);
  };

  const enableCrop = (kind: "sphere" | "box", view: GaussianOrthographicView = orthographicView ?? "side") => {
    const current = useGaussianTransformStore.getState();
    const crop = sceneApiRef.current?.initializeCrop(kind, current.editing.crop);
    if (crop && JSON.stringify(crop) !== JSON.stringify(current.editing.crop)) {
      current.beginCropTransaction();
      current.setCropLive(crop);
      current.commitCropTransaction();
    }
    requestAnimationFrame(() => sceneApiRef.current?.alignView(view));
  };

  const activateTool = (tool: GaussianEditorTool) => {
    if (tool === "sphere" || tool === "box") {
      setOrthographicView("side");
      enableCrop(tool, "side");
    }
    useGaussianTransformStore.getState().setTool(tool);
  };

  const switchTool = async (tool: GaussianEditorTool) => {
    if (busy || tool === store.tool) return;
    const focus = beginFocusReturn();
    sceneApiRef.current?.finishGizmo();
    if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
    useGaussianTransformStore.getState().commitTransaction();
    const current = useGaussianTransformStore.getState();
    if ((current.tool === "sphere" || current.tool === "box") && current.editing.crop) {
      current.commitCropTransaction();
      const latest = useGaussianTransformStore.getState();
      const crop = latest.editing.crop;
      if (!crop || !sceneApiRef.current) return;
      setCropFreezeError(null);
      setCropFreezing(true);
      try {
        const deletedMask = await sceneApiRef.current.freezeCrop(crop, latest.deletedMask);
        useGaussianTransformStore.getState().commitCropFreeze(crop, deletedMask);
        sceneApiRef.current.deactivateCrop();
        activateTool(tool);
        focus();
      } catch (error) {
        setCropFreezeError(error instanceof Error ? error.message : String(error));
      } finally {
        setCropFreezing(false);
      }
      return;
    }
    activateTool(tool);
    focus();
  };

  const alignView = (view: GaussianOrthographicView) => {
    setOrthographicView(view);
    sceneApiRef.current?.alignView(view);
    returnCanvasFocus();
  };

  const changeGizmoMode = (next: GizmoMode) => {
    const focus = beginFocusReturn();
    sceneApiRef.current?.finishGizmo();
    if (document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
    useGaussianTransformStore.getState().commitTransaction();
    setGizmoMode(next); saveGizmoMode(next);
    focus();
  };
  const changeRotation = (axis: 0 | 1 | 2, value: number) => {
    const current = useGaussianTransformStore.getState();
    const pivot = sceneApiRef.current?.modelPivot();
    if (!pivot) return;
    const rotation = [...current.transform.rotation] as GaussianTransform["rotation"];
    rotation[axis] = value;
    current.setTransformLive(rotateAroundModelCenter(current.transform, pivot, rotation));
  };

  const invertSelection = useCallback(() => {
    if (busy || pendingNavigation || navigationSaving || exportBusyRef.current || viewport.phase !== "ready") return;
    const focus = beginFocusReturn();
    if (store.tool === "sphere" || store.tool === "box") {
      const current = useGaussianTransformStore.getState();
      const crop = current.editing.crop;
      if (!crop) return;
      current.beginCropTransaction();
      current.setCropLive({ ...crop, inverted: !crop.inverted });
      current.commitCropTransaction();
      focus();
      return;
    }
    const api = sceneApiRef.current;
    const projectId = store.descriptor?.projectId;
    if (!api || store.tool !== "rectangle") return;
    const owner = editFocusRef.current;
    const samePreview = () => editFocusRef.current === owner && useGaussianTransformStore.getState().descriptor?.projectId === projectId;
    setSelectionBusy(true);
    selectionQueue.current = selectionQueue.current.catch(() => undefined).then(async () => {
      if (!samePreview()) return;
      const mask = await api.invertSelection();
      const current = useGaussianTransformStore.getState();
      if (samePreview() && current.tool === "rectangle") current.setSelectionMask(mask);
      focus();
    }).catch((error: unknown) => { if (samePreview()) setViewport((current) => ({ ...current, error: error instanceof Error ? error.message : String(error) })); })
      .finally(() => { if (samePreview()) setSelectionBusy(false); });
  }, [beginFocusReturn, busy, navigationSaving, pendingNavigation, viewport.phase, store.tool, store.descriptor?.projectId]);

  useEffect(() => {
    const mouseLookEscape = (event: KeyboardEvent) => {
      consumeMouseLookEscape(event, sceneApiRef.current);
    };
    const keyDown = (event: KeyboardEvent) => {
      if (sceneApiRef.current?.isMouseLookActive()) return;
      if (mode !== "adjust" || !store.descriptor || busy) return;
      if (store.tool !== "transform" && viewport.phase === "ready" && isInvertSelectionShortcut(event)) {
        event.preventDefault();
        invertSelection();
        return;
      }
      const editingField = document.activeElement instanceof HTMLInputElement || document.activeElement instanceof HTMLTextAreaElement;
      if (store.tool === "rectangle" && !editingField && event.key === "Escape") {
        event.preventDefault(); useGaussianTransformStore.getState().clearSelection(); returnCanvasFocus(); return;
      }
      if (store.tool === "rectangle" && !editingField && (event.key === "Delete" || event.key === "Backspace")) {
        event.preventDefault(); useGaussianTransformStore.getState().deleteSelection(); returnCanvasFocus(); return;
      }
      if (editingField || !event.ctrlKey && !event.metaKey) return;
      const key = event.key.toLowerCase();
      const isUndo = key === "z" && !event.shiftKey;
      const isRedo = (key === "z" && event.shiftKey) || key === "y";
      if (!isUndo && !isRedo) return;
      const focus = beginFocusReturn();
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
      focus();
    };
    window.addEventListener("keydown", mouseLookEscape, true);
    window.addEventListener("keydown", keyDown);
    return () => {
      window.removeEventListener("keydown", mouseLookEscape, true);
      window.removeEventListener("keydown", keyDown);
    };
  }, [beginFocusReturn, returnCanvasFocus, busy, invertSelection, mode, store.descriptor?.projectId, store.tool, viewport.phase]);

  const rectanglePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (consumeMouseLookPointer(event, sceneApiRef.current)) return;
    if (mode !== "adjust" || store.tool !== "rectangle" || event.button !== 0 || busy || viewport.phase !== "ready") return;
    if ((event.target as Element).closest("button,input,aside")) return;
    sceneApiRef.current?.stopNavigation();
    rectangleFocusRef.current = beginFocusReturn();
    event.preventDefault(); event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    setSelectionDrag({ pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, x: event.clientX, y: event.clientY, selectionMode: event.ctrlKey ? "remove" : event.shiftKey ? "add" : "replace" });
  };
  const rectanglePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (consumeMouseLookPointer(event, sceneApiRef.current)) return;
    if (!selectionDrag || event.pointerId !== selectionDrag.pointerId) return;
    event.preventDefault(); event.stopPropagation();
    setSelectionDrag((current) => current ? { ...current, x: event.clientX, y: event.clientY } : null);
  };
  const rectanglePointerEnd = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (consumeMouseLookPointer(event, sceneApiRef.current)) return;
    const drag = selectionDrag;
    if (!drag || event.pointerId !== drag.pointerId) return;
    event.preventDefault(); event.stopPropagation();
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    setSelectionDrag(null);
    const focus = rectangleFocusRef.current;
    rectangleFocusRef.current = null;
    if (event.type === "pointercancel" || event.type === "lostpointercapture") { focus?.(); return; }
    const canvas = event.currentTarget.querySelector("canvas");
    if (!(canvas instanceof HTMLCanvasElement) || !sceneApiRef.current) return;
    const bounds = canvas.getBoundingClientRect();
    const toNdcX = (value: number) => (value - bounds.left) / Math.max(bounds.width, 1) * 2 - 1;
    const toNdcY = (value: number) => 1 - (value - bounds.top) / Math.max(bounds.height, 1) * 2;
    const x1 = toNdcX(drag.startX); const x2 = toNdcX(event.clientX); const y1 = toNdcY(drag.startY); const y2 = toNdcY(event.clientY);
    const api = sceneApiRef.current;
    const clicking = Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) < 3;
    const padX = clicking ? 4 / Math.max(bounds.width, 1) : 0;
    const padY = clicking ? 4 / Math.max(bounds.height, 1) : 0;
    const rectangle = { minX: Math.max(-1, Math.min(x1, x2) - padX), minY: Math.max(-1, Math.min(y1, y2) - padY), maxX: Math.min(1, Math.max(x1, x2) + padX), maxY: Math.min(1, Math.max(y1, y2) + padY) };
    const projectId = store.descriptor?.projectId;
    const owner = editFocusRef.current;
    const samePreview = () => editFocusRef.current === owner && useGaussianTransformStore.getState().descriptor?.projectId === projectId;
    setSelectionBusy(true);
    selectionQueue.current = selectionQueue.current
      .catch(() => undefined)
      .then(async () => {
        if (!samePreview()) return;
        const mask = await api.selectRectangle(rectangle, drag.selectionMode);
        const current = useGaussianTransformStore.getState();
        if (samePreview() && current.tool === "rectangle") current.setSelectionMask(mask);
        focus?.();
      })
      .catch((error: unknown) => { if (samePreview()) setViewport((current) => ({ ...current, error: error instanceof Error ? error.message : String(error) })); })
      .finally(() => { if (samePreview()) setSelectionBusy(false); });
  };

  if (!store.descriptor) return null;

  const loadingLabel = viewport.phase === "initializing"
    ? t("viewer.initializing")
    : viewport.phase === "mounting"
      ? t("viewer.mounting")
      : t("viewer.loading");
  const phaseLabel = {
    initializing: t("viewer.phaseInitializing"),
    loading: t("viewer.phaseLoading"),
    mounting: t("viewer.phaseMounting"),
    ready: t("viewer.phaseReady"),
    error: t("viewer.phaseError"),
  }[viewport.phase];
  const videoBusy = !["idle", "completed", "error"].includes(videoPhase);
  const invertShortcutLabel = invertSelectionShortcutLabel(navigator.platform);
  const videoButtonLabel = videoPhase === "preparing"
    ? t("viewer.preparingExport")
    : videoPhase === "rendering"
      ? t("viewer.rendering", { current: videoProgress.currentFrame, total: videoProgress.totalFrames })
      : videoPhase === "finalizing"
        ? t("viewer.packaging")
        : videoPhase === "saving"
          ? t("viewer.saving")
          : t("viewer.exportVideo");
  const toolItems: Array<{ id: GaussianEditorTool; label: string; icon: typeof MousePointer2 }> = [
    { id: "transform", label: t("viewer.transform"), icon: MousePointer2 },
    { id: "rectangle", label: t("viewer.rectangle"), icon: RectangleHorizontal },
    { id: "sphere", label: t("viewer.sphere"), icon: CircleDot },
    { id: "box", label: t("viewer.box"), icon: Box },
  ];
  const selectionRectStyle = selectionDrag ? {
    left: Math.min(selectionDrag.startX, selectionDrag.x), top: Math.min(selectionDrag.startY, selectionDrag.y),
    width: Math.abs(selectionDrag.x - selectionDrag.startX), height: Math.abs(selectionDrag.y - selectionDrag.startY),
  } : undefined;

  return <section className={`preview-pane active preview-workspace viewer-mode-${mode}`} aria-label={t("viewer.aria")}>
    <header className="preview-header">
      <div className="preview-heading">
        <button className="preview-back-icon" type="button" title={t("viewer.back")} aria-label={t("viewer.back")} disabled={busy} onClick={() => requestNavigation("exit")}><ArrowLeft size={19} /></button>
        <h1>{t("viewer.heading")}</h1>
      </div>
      <div className="preview-mode-control">
        <div className={`preview-mode-toggle mode-${mode}`} role="group" aria-label={t("viewer.modeAria")}>
          <button type="button" className={mode === "adjust" ? "active" : ""} aria-pressed={mode === "adjust"} disabled={busy} onClick={() => switchMode("adjust")}>{t("viewer.adjust")}</button>
          <button type="button" className={mode === "preview" ? "active" : ""} aria-pressed={mode === "preview"} disabled={busy} onClick={() => switchMode("preview")}>{t("viewer.animation")}</button>
        </div>
        <p>{mode === "adjust" ? t("viewer.adjustHint") : t("viewer.animationHint")}</p>
      </div>
      <div className="preview-input-hints" aria-label={t("viewer.inputAria")}>
        <span><Orbit size={15} /><kbd>{mode === "adjust" && store.tool === "rectangle" ? t("viewer.middle") : t("viewer.left")}</kbd>{t("viewer.rotate")}</span>
        <span><Move size={15} /><kbd>{t("viewer.right")}</kbd>{t("viewer.drag")}</span>
        <span><ZoomIn size={15} /><kbd>{t("viewer.wheel")}</kbd>{t("viewer.zoom")}</span>
        {mode === "adjust" && <span className={`preview-roam-hint${orthographicProjection ? " unavailable" : ""}`} title={t("viewer.roamHint")} aria-disabled={orthographicProjection}>
          <Move size={15} /><kbd>WASD</kbd>{t("viewer.roam")}
        </span>}
        {mode === "adjust" && store.tool === "transform" && gizmoMode !== "off" && <>
          <span><Move size={15} /><kbd>{t("viewer.left")}</kbd>{t(gizmoMode === "translate" ? "viewer.gizmoMove" : "viewer.gizmoRotate")}</span>
          <span><X size={15} /><kbd>Esc</kbd>{t("viewer.cancelTransform")}</span>
        </>}
        {mode === "adjust" && store.tool === "rectangle" && <>
          <span><RectangleHorizontal size={15} /><kbd>{t("viewer.left")}</kbd>{t("viewer.select")}</span>
          <span><Plus size={15} /><kbd>Shift</kbd>{t("viewer.add")}</span>
          <span><Minus size={15} /><kbd>Ctrl</kbd>{t("viewer.remove")}</span>
          <span><Trash2 size={15} /><kbd>Delete / Backspace</kbd>{t("viewer.delete")}</span>
          <span><X size={15} /><kbd>Esc</kbd>{t("viewer.cancelSelection")}</span>
        </>}
        {mode === "adjust" && store.tool !== "transform" && <span><InvertSelectionIcon size={15} /><kbd>{invertShortcutLabel}</kbd>{t("viewer.invertSelection")}</span>}
      </div>
    </header>
    <div className="preview-commandbar">
      <div className="preview-commandbar-left">
        {mode === "adjust" && <div className="preview-editor-tools" role="toolbar" aria-label={t("viewer.toolsAria")}>
          {toolItems.map(({ id, label, icon: Icon }) => <button key={id} type="button" className={store.tool === id ? "active" : ""} aria-pressed={store.tool === id} disabled={busy || viewport.phase !== "ready"} onClick={() => void switchTool(id)}><Icon size={14} />{label}</button>)}
        </div>}
      </div>
      <div className="preview-commandbar-center">
        {mode === "preview" && <PreviewOrientationControl orientation={orientation} disabled={busy} onChange={setOrientation} />}
        {mode === "adjust" && <div className="preview-view-control" role="group" aria-label={t("viewer.viewsAria")}>
          {(["side", "front", "top"] as const).map((view) => { const viewLabel = view === "side" ? t("viewer.side") : view === "front" ? t("viewer.front") : t("viewer.top"); return <button key={view} type="button" className={orthographicView === view ? "active" : ""} aria-pressed={orthographicView === view} disabled={busy || viewport.phase !== "ready"} title={t("viewer.switchView", { view: viewLabel })} onClick={() => alignView(view)}>{viewLabel}</button>; })}
          <button type="button" disabled={busy || viewport.phase !== "ready" || !store.descriptor.referenceView}
            title={t(!store.descriptor.referenceView ? store.descriptor.referenceViewUnavailable === "imported" ? "viewer.locateImported" : "viewer.locateUnavailable" : "viewer.locateHint")}
            onClick={() => sceneApiRef.current?.locateView()}>{t("viewer.locate")}</button>
        </div>}
      </div>
      <div className="preview-header-actions">
        {mode === "adjust" ? <>
          {store.tool !== "transform" && <button type="button" title={`${t("viewer.invertSelectionTitle")} · ${invertShortcutLabel}`} aria-keyshortcuts="Control+I Meta+I" aria-pressed={store.tool === "rectangle" ? undefined : Boolean(store.editing.crop?.inverted)} disabled={busy || viewport.phase !== "ready" || (store.tool !== "rectangle" && store.editing.crop === null)} onClick={invertSelection}><InvertSelectionIcon size={14} />{t("viewer.invertSelection")}</button>}
          <button type="button" title={t("viewer.undoTitle")} disabled={store.history.length === 0 || busy} onClick={() => runHistory("undo")}><Undo2 size={14} />{t("viewer.undo")}</button>
          <button type="button" title={t("viewer.redoTitle")} disabled={store.future.length === 0 || busy} onClick={() => runHistory("redo")}><Redo2 size={14} />{t("viewer.redo")}</button>
          <button type="button" title={t("viewer.resetAllTitle")} disabled={busy || (store.history.length === 0 && store.editing.crop === null && store.editing.deletedCount === 0 && store.transform.scale === 1 && store.transform.position.every((value) => value === 0) && store.transform.rotation.every((value) => value === 0))} onClick={() => { store.resetAll(); setGaussianExportResult(null); returnCanvasFocus(); }}><RotateCcw size={14} />{t("viewer.resetAll")}</button>
          <button type="button" disabled={busy || viewport.phase !== "ready"} onClick={() => void saveEdits()}>{savingEdits ? <LoaderCircle className="spin" size={14} /> : <Save size={14} />} {savingEdits ? t("viewer.saving") : t("viewer.saveEdits")}</button>
        </> : <>
          <button type="button" disabled={busy || viewport.phase !== "ready"} onClick={() => sceneApiRef.current?.replay()}><Play size={14} />{t("viewer.replay")}</button>
        </>}
        {videoBusy ? <button type="button" className="video-cancel" onClick={cancelVideo}><X size={14} />{t("viewer.cancelExport")}</button>
          : htmlBusy ? <button type="button" className="video-cancel" onClick={cancelHtml}><X size={14} />{t("viewer.cancelExport")}</button>
          : <PreviewExportMenu mode={mode} disabled={busy || viewport.phase !== "ready"} videoSupported={videoCapability.supported} videoReason={videoCapability.reason} onModel={format => void exportGaussian(format)} onVideo={() => void exportVideo()} onHtml={() => setHtmlConfirm(true)} />}
      </div>
    </div>
    {pipelineRunning && <div className="preview-resource-note">{t("viewer.resourceNote")}</div>}
    <div className="preview-editor">
      <div ref={viewportElementRef} inert={operationBusy} className={`gaussian-viewport tool-${store.tool}`} onPointerDownCapture={rectanglePointerDown} onPointerMoveCapture={rectanglePointerMove} onPointerUpCapture={rectanglePointerEnd} onPointerCancelCapture={rectanglePointerEnd}>
        <Application key={`${store.descriptor.projectId}-${rendererRevision}`} autoRender={false} className="gaussian-canvas" deviceTypes={previewDeviceTypes()} graphicsDeviceOptions={{ antialias: false, alpha: false, preserveDrawingBuffer: true, powerPreference: "high-performance" }}>
          <SplatScene ref={sceneApiRef} projectId={store.descriptor.projectId} assetUrl={previewAssetUrl} fileSize={store.descriptor.fileSize} readToken={store.descriptor.readToken} referenceView={store.descriptor.referenceView} splatCount={store.descriptor.splatCount} transform={store.transform} mode={mode} tool={store.tool} crop={store.editing.crop} deletedMask={store.deletedMask} selectionMask={store.selectionMask} editorMasksReady={!store.descriptor.editMaskAssetUrl || loadedMaskKey === `${store.descriptor.projectId}:${store.descriptor.editMaskAssetUrl}`} gizmoMode={gizmoMode} roamSpeed={roamSpeed} interactionBlocked={interactionBlocked || selectionDrag !== null} onGizmoDragging={onGizmoDragging} onOrthographicViewChange={onViewDirectionChange} onStatus={onStatus} onAnimationStatus={onAnimationStatus} />
        </Application>
        {mode === "preview" && <div ref={captureGuideRef} className={`portrait-capture-guide orientation-${orientation}`} style={guideSize} aria-hidden="true">
          <div className="portrait-frame-label"><span>{gaussianVideoDimensions(orientation).width} × {gaussianVideoDimensions(orientation).height}</span><span>30 FPS</span></div>
          <div className="preview-watermark"><img src={appLogo} alt="" /><strong>OOOSplat</strong></div>
        </div>}
        {mode === "preview" && <div className="portrait-matte" style={guideSize} aria-hidden="true" />}
        {viewport.phase !== "ready" && viewport.phase !== "error" && <div className="viewport-overlay"><LoaderCircle className="spin" size={22} /><strong>{loadingLabel}</strong>{viewport.phase === "loading" && <span>{(viewport.progress * 100).toFixed(0)}%</span>}</div>}
        {viewport.phase === "error" && <div className="viewport-overlay error"><strong>{t("viewer.unavailable")}</strong><p>{viewport.error}</p><div className="viewport-error-actions"><button type="button" onClick={retry}>{t("viewer.reload")}</button><button type="button" onClick={() => requestNavigation("exit")}>{t("viewer.back")}</button></div></div>}
        {cropFreezing && <div className="viewport-overlay"><LoaderCircle className="spin" size={22} /><strong>{t("viewer.freezing")}</strong><span>{t("viewer.wait")}</span></div>}
        {cropFreezeError && !cropFreezing && <div className="viewport-overlay error"><strong>{t("viewer.freezeFailed")}</strong><p>{cropFreezeError}</p><div className="viewport-error-actions"><button type="button" onClick={() => setCropFreezeError(null)}>{t("viewer.backToEdit")}</button></div></div>}
        {selectionDrag && <div className={`rectangle-selection-box mode-${selectionDrag.selectionMode}`} style={selectionRectStyle} aria-hidden="true" />}
        {mode === "adjust" && store.tool === "transform" && <TransformPanel transform={store.transform} onBegin={store.beginTransaction} onChange={store.setTransformLive} onCommit={store.commitTransaction} onFocusCanvas={returnCanvasFocus} onRotationChange={changeRotation} gizmoMode={gizmoMode} onGizmoModeChange={changeGizmoMode} disabled={busy || viewport.phase !== "ready"} />}
        {mode === "adjust" && (store.tool === "sphere" || store.tool === "box") && <SelectionPanel crop={store.editing.crop} kind={store.tool} onBegin={store.beginCropTransaction} onChange={store.setCropLive} onCommit={store.commitCropTransaction} onFocusCanvas={returnCanvasFocus} onEnable={() => { const tool = useGaussianTransformStore.getState().tool; if (tool === "sphere" || tool === "box") enableCrop(tool); }} />}
        {mode === "preview" && <div className="animation-hud">
          <span className={`animation-pulse phase-${animationStatus.phase}`} />
          <b>{t(phaseLabelKeys[animationStatus.phase])}</b>
          <span className="animation-time">{animationStatus.elapsedSeconds.toFixed(1)}s</span>
          <span className="orbit-axis-key"><i aria-hidden="true" />{t("viewer.orbitAxis")}</span>
        </div>}
        {mode === "preview" && videoBusy && <div className="video-export-overlay">
          <div><LoaderCircle className="spin" size={20} /><strong>{videoButtonLabel}</strong></div>
          <div className="video-export-track"><span style={{ width: `${videoProgress.progress * 100}%` }} /></div>
          <small>{videoPhase === "rendering" ? t("viewer.framesProgress", { percent: Math.round(videoProgress.progress * 100), current: videoProgress.currentFrame, total: GAUSSIAN_VIDEO_FRAME_COUNT }) : t("viewer.keepOpen")}</small>
        </div>}
      </div>
    </div>
    <footer className="preview-statusbar">
      <div className="preview-status-items">
      <span><b>{t("viewer.splats")}</b>{formatNumber(store.descriptor.splatCount)}</span>
      {mode === "adjust" && store.selectedCount > 0 && <span className="selection-count"><b>{t("viewer.selected")}</b>{formatNumber(store.selectedCount)}</span>}
      {mode === "adjust" && store.editing.deletedCount > 0 && <span><b>{t("viewer.deleted")}</b>{formatNumber(store.editing.deletedCount)}</span>}
      <span><b>{t("viewer.fileSize")}</b>{formatBytes(store.descriptor.fileSize, locale)}</span>
      {mode === "adjust"
        ? <span><b>{t("viewer.position")}</b>{compact(store.transform.position)} <b>{t("viewer.rotation")}</b>{compact(store.transform.rotation)} <b>{t("viewer.scale")}</b>{Number(store.transform.scale.toFixed(3))}</span>
        : <span><b>{t("viewer.timeline")}</b>{t("viewer.timelineValue")}</span>}
      <span><b>{t("viewer.renderer")}</b>{viewport.renderer}</span>
      <span><b>{t("viewer.status")}</b>{phaseLabel}</span>
      {mode === "adjust" && <span className={`save-state ${store.saveState}`}><b>{t("viewer.project")}</b>{store.saveState === "saving" ? t("viewer.stateSaving") : store.saveState === "error" ? t("viewer.stateFailed") : store.saveState === "dirty" ? t("viewer.stateDirty") : t("viewer.stateSaved")}</span>}
      {gaussianExportResult && <button className="statusbar-file-action" type="button" title={gaussianExportResult} onClick={() => void revealGaussianExport(gaussianExportResult, false)}><FolderOpen size={12} /><b>{t("viewer.exported")}</b>{gaussianExportResult.split(/[\\/]/).at(-1)}</button>}
      {mode === "preview" && <span><b>{t("viewer.videoEncoding")}</b>{videoCapability.checking ? t("viewer.checking") : videoCapability.supported ? t("viewer.h264Ready") : t("common.unavailable")}</span>}
      {videoResult && mode === "preview" && <button className="statusbar-file-action" type="button" title={videoResult.path} onClick={() => void revealFile(videoResult.path)}><FolderOpen size={12} /><b>{t("viewer.exported")}</b>{videoResult.path.split(/[\\/]/).at(-1)} · {formatBytes(videoResult.fileSize, locale)}</button>}
      {htmlResult && <button className="statusbar-file-action" type="button" title={htmlResult.path} onClick={() => void revealFile(htmlResult.path).catch((error: unknown) => setHtmlError(String(error)))}><FolderOpen size={12} /><b>{t("viewer.exported")}</b>{htmlResult.path.split(/[\\/]/).at(-1)} · {formatBytes(htmlResult.fileSize, locale)}</button>}
      </div>
      {mode === "adjust" && <RoamSpeedControl value={roamSpeed} disabled={busy || interactionBlocked || selectionDrag !== null || orthographicProjection}
        onChange={setRoamSpeed} onCommit={saveRoamSpeed} onFocusCanvas={returnCanvasFocus} />}
    </footer>
    {store.saveError && mode === "adjust" && <div className="preview-save-error"><span>{store.saveError}</span><button type="button" onClick={retrySave}>{t("viewer.retrySave")}</button></div>}
    {saveNotice && <div className={`preview-video-message ${saveNotice === "viewFailed" ? "warning" : ""}`} role="status">{t(saveNotice === "viewFailed" ? "viewer.viewSaveFailed" : "viewer.editsSaved")}</div>}
    {mode === "preview" && !videoCapability.checking && !videoCapability.supported && <div className="preview-video-message warning">{videoCapability.reason}</div>}
    {mode === "preview" && videoError && <div className="preview-video-message error">{t("viewer.exportFailed", { detail: videoError })}</div>}
    {htmlError && <div className="preview-video-message error">{t("export.failed", {detail:htmlError})}</div>}
    {htmlBusy && <div className="html-export-progress" role="status"><LoaderCircle className="spin" size={18} /><span>{t(htmlProgress?.phase === "packing" ? "export.packing" : htmlProgress?.phase === "completed" ? "export.completed" : "export.preparing")}</span><progress max="100" value={htmlProgress?.progress ?? 0} /><span>{Math.floor(htmlProgress?.progress ?? 0)}%</span><button type="button" onClick={cancelHtml}>{t("common.cancel")}</button></div>}
    <PlyExportDialog
      state={gaussianExportDialog}
      progress={gaussianExportProgress}
      onClose={closeGaussianExportDialog}
      onRetry={() => void exportGaussian(modelFormat, true)}
      onReveal={(path) => void revealGaussianExport(path)}
      onCancel={() => void cancelModelExport()}
      onConfirmAdjustment={(exportId, confirmationId) => modelOperationRef.current?.confirmAdjustment(exportId, confirmationId)}
      format={modelFormat}
    />
    {htmlConfirm && <div className="gaussian-save-backdrop" role="dialog" aria-modal="true" aria-labelledby="html-export-title" aria-describedby="html-export-description" onKeyDown={event => { if(event.key === "Escape")setHtmlConfirm(false); }}>
      <div className="gaussian-save-dialog"><div className="gaussian-save-symbol"><FileCode2 size={19} /></div><div><h2 id="html-export-title">{t("export.html")}</h2><p id="html-export-description">{t("export.htmlWarning", {size:formatBytes(Math.ceil(store.descriptor.fileSize*4/3),locale)})}</p></div><div className="gaussian-save-actions"><button autoFocus type="button" className="secondary" onClick={() => setHtmlConfirm(false)}>{t("common.cancel")}</button><button type="button" className="primary" onClick={() => void exportHtml()}><Download size={14} />{t("export.label")}</button></div></div>
    </div>}
    {pendingNavigation && <div className="gaussian-save-backdrop" role="dialog" aria-modal="true" aria-labelledby="gaussian-save-title" aria-describedby="gaussian-save-description" onKeyDown={event => { if (event.key === "Escape" && !navigationSaving) setPendingNavigation(null); }}>
      <div className="gaussian-save-dialog">
        <div className="gaussian-save-symbol"><Save size={19} /></div>
        <div>
          <h2 id="gaussian-save-title">{t("viewer.saveFailed")}</h2>
          <p id="gaussian-save-description">{t("viewer.saveFailedDescription")}</p>
        </div>
        <div className="gaussian-save-actions">
          <button autoFocus type="button" className="secondary" disabled={navigationSaving} onClick={() => setPendingNavigation(null)}>{t("viewer.backToEdit")}</button>
          <button type="button" className="primary" disabled={navigationSaving} onClick={() => void requestNavigation(pendingNavigation, true)}>{navigationSaving ? <LoaderCircle className="spin" size={14} /> : <Save size={14} />}{t("viewer.retrySave")}</button>
        </div>
      </div>
    </div>}
  </section>;
}
