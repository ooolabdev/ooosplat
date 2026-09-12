import { BoundingBox, Entity, PROJECTION_ORTHOGRAPHIC, PROJECTION_PERSPECTIVE, Vec3, type CameraComponent } from "playcanvas";
import { getCurrentLocale, translate } from "../../i18n";
import type { GaussianOrthographicView } from "../../types/pipeline";

const DEFAULT_YAW = 35;
const DEFAULT_PITCH = 22;
const DEFAULT_OCCUPANCY = 0.85;
const VIEW_TRANSITION_MS = 320;

export interface FitViewOptions {
  resetDirection?: boolean;
  occupancy?: number;
  rightInsetPx?: number;
}

export interface ViewerCameraState {
  target: [number, number, number];
  yaw: number;
  pitch: number;
  distance: number;
  horizontalFrameOffset: number;
  projection: number;
  orthoHeight: number;
  orthographicView: GaussianOrthographicView | null;
}

export interface ViewerOrbitGuideState {
  target: [number, number, number];
  radius: number;
  height: number;
  angleDegrees: number;
}

export class ViewerControls {
  private static readonly DRAG_THRESHOLD = 3;
  private readonly canvas: HTMLCanvasElement;
  private readonly cameraEntity: Entity;
  private readonly camera: CameraComponent;
  private target = new Vec3();
  private yaw = DEFAULT_YAW;
  private pitch = DEFAULT_PITCH;
  private distance = 5;
  private sceneBounds: BoundingBox | null = null;
  private horizontalFrameOffset = 0;
  private pointerId: number | null = null;
  private mode: "orbit" | "pan" | null = null;
  private startX = 0;
  private startY = 0;
  private lastX = 0;
  private lastY = 0;
  private dragging = false;
  private destroyed = false;
  private _enabled = true;
  private orbitButton = 0;
  private orthographicView: GaussianOrthographicView | null = null;
  private transitionFrame = 0;
  private transitioning = false;

  constructor(
    canvas: HTMLCanvasElement,
    cameraEntity: Entity,
    private readonly onOrthographicViewChange?: (view: GaussianOrthographicView | null) => void,
  ) {
    if (!cameraEntity.camera) throw new Error(translate(getCurrentLocale(), "viewer.cameraUnavailable"));
    this.canvas = canvas;
    this.cameraEntity = cameraEntity;
    this.camera = cameraEntity.camera;
    canvas.addEventListener("pointerdown", this.onPointerDown);
    canvas.addEventListener("pointermove", this.onPointerMove);
    canvas.addEventListener("pointerup", this.onPointerUp);
    canvas.addEventListener("pointercancel", this.onPointerUp);
    canvas.addEventListener("wheel", this.onWheel, { passive: false });
    canvas.addEventListener("contextmenu", this.preventContextMenu);
    this.updateCamera();
  }

  destroy() {
    if (this.destroyed) return;
    this.destroyed = true;
    this.cancelViewTransition();
    this.cancelGesture();
    this.canvas.removeEventListener("pointerdown", this.onPointerDown);
    this.canvas.removeEventListener("pointermove", this.onPointerMove);
    this.canvas.removeEventListener("pointerup", this.onPointerUp);
    this.canvas.removeEventListener("pointercancel", this.onPointerUp);
    this.canvas.removeEventListener("wheel", this.onWheel);
    this.canvas.removeEventListener("contextmenu", this.preventContextMenu);
  }

  get enabled() {
    return this._enabled;
  }

  set enabled(value: boolean) {
    this._enabled = value;
    if (!value) this.cancelGesture();
  }

  get cameraDistance() {
    return this.distance;
  }

  get cameraPitch() {
    return this.pitch;
  }

  snapshot(): ViewerCameraState {
    return {
      target: [this.target.x, this.target.y, this.target.z],
      yaw: this.yaw,
      pitch: this.pitch,
      distance: this.distance,
      horizontalFrameOffset: this.horizontalFrameOffset,
      projection: this.camera.projection,
      orthoHeight: this.camera.orthoHeight,
      orthographicView: this.orthographicView,
    };
  }

  orbitGuideState(): ViewerOrbitGuideState {
    const pitch = this.pitch * Math.PI / 180;
    const aspect = this.canvas.clientWidth / Math.max(this.canvas.clientHeight, 1)
      || Math.max(this.camera.aspectRatio || 16 / 9, 0.01);
    const horizontalDistance = this.distance * Math.cos(pitch);
    const horizontalFrameShift = this.distance
      * Math.tan((this.camera.fov * Math.PI) / 360)
      * aspect
      * this.horizontalFrameOffset;
    return {
      target: [this.target.x, this.target.y, this.target.z],
      radius: Math.max(Math.hypot(horizontalDistance, horizontalFrameShift), 0.0001),
      height: this.target.y + this.distance * Math.sin(pitch),
      angleDegrees: this.yaw + Math.atan2(horizontalFrameShift, horizontalDistance) * 180 / Math.PI,
    };
  }

  restore(state: ViewerCameraState) {
    this.cancelViewTransition();
    this.target.set(...state.target);
    this.yaw = state.yaw;
    this.pitch = state.pitch;
    this.distance = state.distance;
    this.horizontalFrameOffset = state.horizontalFrameOffset;
    this.camera.projection = state.projection;
    this.camera.orthoHeight = state.orthoHeight;
    this.orthographicView = state.orthographicView;
    this.updateCamera();
  }

  orbitBy(degrees: number) {
    if (!Number.isFinite(degrees) || degrees === 0) return;
    this.cancelViewTransition();
    this.yaw += degrees;
    this.updateCamera();
  }

  setOrbitYaw(yaw: number) {
    if (!Number.isFinite(yaw)) return;
    this.cancelViewTransition();
    this.yaw = yaw;
    this.updateCamera();
  }

  setSceneBounds(bounds: BoundingBox) {
    this.sceneBounds = bounds.clone();
    this.cancelViewTransition();
    this.updateCamera();
  }

  setRectangleSelectionMode(enabled: boolean) {
    this.orbitButton = enabled ? 1 : 0;
    this.cancelGesture();
  }

  alignOrthographic(view: GaussianOrthographicView, bounds: BoundingBox, occupancy = 0.85, animate = true) {
    this.cancelViewTransition();
    this.cancelGesture();
    const startPosition = this.cameraEntity.getPosition().clone();
    const startTarget = this.target.clone();
    const startUp = this.cameraEntity.up.clone();
    const startOrthoHeight = this.camera.projection === PROJECTION_ORTHOGRAPHIC
      ? this.camera.orthoHeight
      : Math.max(this.distance * Math.tan(this.camera.fov * Math.PI / 360), 0.001);
    this.orthographicView = view;
    this.onOrthographicViewChange?.(view);
    this.camera.projection = PROJECTION_ORTHOGRAPHIC;
    const destinationTarget = bounds.center.clone();
    const aspect = this.canvas.clientWidth / Math.max(this.canvas.clientHeight, 1) || 1;
    const half = bounds.halfExtents;
    const [horizontal, vertical] = view === "side" ? [half.z, half.y] : view === "front" ? [half.x, half.y] : [half.x, half.z];
    const destinationOrthoHeight = Math.max(vertical / occupancy, horizontal / Math.max(aspect * occupancy, 0.001), 0.001);
    const destinationDistance = Math.max(half.x, half.y, half.z, 0.01) * 3;
    const destinationPosition = destinationTarget.clone();
    if (view === "side") destinationPosition.x += destinationDistance;
    else if (view === "front") destinationPosition.z += destinationDistance;
    else destinationPosition.y += destinationDistance;
    const destinationUp = view === "top" ? new Vec3(0, 0, -1) : Vec3.UP.clone();
    this.target.copy(destinationTarget);
    this.camera.orthoHeight = destinationOrthoHeight;
    this.distance = destinationDistance;
    this.horizontalFrameOffset = 0;
    if (!animate || this.destroyed) {
      this.updateCamera();
      return;
    }

    this.transitioning = true;
    const startedAt = performance.now();
    const position = new Vec3();
    const lookTarget = new Vec3();
    const up = new Vec3();
    const tick = (now: number) => {
      if (this.destroyed || !this.transitioning) return;
      const linear = Math.min(1, Math.max(0, (now - startedAt) / VIEW_TRANSITION_MS));
      const progress = 1 - Math.pow(1 - linear, 3);
      position.lerp(startPosition, destinationPosition, progress);
      lookTarget.lerp(startTarget, destinationTarget, progress);
      up.lerp(startUp, destinationUp, progress).normalize();
      this.camera.orthoHeight = startOrthoHeight + (destinationOrthoHeight - startOrthoHeight) * progress;
      this.cameraEntity.setPosition(position);
      this.cameraEntity.lookAt(lookTarget, up);
      this.updateClipPlanes(position, lookTarget.clone().sub(position).normalize());
      if (linear < 1) {
        this.transitionFrame = requestAnimationFrame(tick);
      } else {
        this.transitioning = false;
        this.transitionFrame = 0;
        this.updateCamera();
      }
    };
    this.transitionFrame = requestAnimationFrame(tick);
  }

  usePerspective() {
    this.cancelViewTransition();
    this.orthographicView = null;
    this.onOrthographicViewChange?.(null);
    this.camera.projection = PROJECTION_PERSPECTIVE;
    this.updateCamera();
  }

  cancelGesture() {
    this.pointerId = null;
    this.mode = null;
    this.dragging = false;
  }

  fit(bounds: BoundingBox, options: FitViewOptions = {}) {
    this.cancelViewTransition();
    if (options.resetDirection) {
      this.yaw = DEFAULT_YAW;
      this.pitch = DEFAULT_PITCH;
    }

    const occupancy = Math.max(0.5, Math.min(0.98, options.occupancy ?? DEFAULT_OCCUPANCY));
    if (this.camera.projection === PROJECTION_ORTHOGRAPHIC && this.orthographicView) {
      this.alignOrthographic(this.orthographicView, bounds, occupancy, false);
      return;
    }
    const width = Math.max(this.canvas.clientWidth, 1);
    const height = Math.max(this.canvas.clientHeight, 1);
    const aspect = width > 1 && height > 1 ? width / height : Math.max(this.camera.aspectRatio || 16 / 9, 0.01);
    const rightInset = width > 1 ? Math.max(0, Math.min(options.rightInsetPx ?? 0, width * 0.45)) : 0;
    const usableWidthRatio = Math.max(0.55, (width - rightInset) / width);
    const halfVerticalFov = (this.camera.fov * Math.PI) / 360;
    const verticalCapacity = Math.max(Math.tan(halfVerticalFov) * occupancy, 0.001);
    const horizontalCapacity = Math.max(verticalCapacity * aspect * usableWidthRatio, 0.001);
    const { forward, right, up } = this.cameraBasis();
    const center = bounds.center;
    const half = bounds.halfExtents;
    let requiredDistance = 0.05;

    for (const x of [-half.x, half.x]) {
      for (const y of [-half.y, half.y]) {
        for (const z of [-half.z, half.z]) {
          const corner = new Vec3(x, y, z);
          const horizontal = Math.abs(corner.dot(right));
          const vertical = Math.abs(corner.dot(up));
          const depth = corner.dot(forward);
          requiredDistance = Math.max(
            requiredDistance,
            horizontal / horizontalCapacity - depth,
            vertical / verticalCapacity - depth,
            -depth + 0.01,
          );
        }
      }
    }

    this.target.copy(center);
    this.distance = Math.max(requiredDistance, 0.05);
    this.horizontalFrameOffset = width > 1 ? rightInset / width : 0;
    this.updateCamera();
  }

  private cameraBasis() {
    const yaw = this.yaw * Math.PI / 180;
    const pitch = this.pitch * Math.PI / 180;
    const offset = new Vec3(
      Math.sin(yaw) * Math.cos(pitch),
      Math.sin(pitch),
      Math.cos(yaw) * Math.cos(pitch),
    );
    const forward = offset.clone().mulScalar(-1);
    const right = new Vec3().cross(forward, Vec3.UP).normalize();
    const up = new Vec3().cross(right, forward).normalize();
    return { offset, forward, right, up };
  }

  private updateCamera() {
    if (this.orthographicView) {
      const position = this.target.clone();
      if (this.orthographicView === "side") position.x += this.distance;
      else if (this.orthographicView === "front") position.z += this.distance;
      else position.y += this.distance;
      this.cameraEntity.setPosition(position);
      this.cameraEntity.lookAt(this.target, this.orthographicView === "top" ? new Vec3(0, 0, -1) : Vec3.UP);
      const forward = this.target.clone().sub(position).normalize();
      this.updateClipPlanes(position, forward);
      return;
    }
    const { offset, forward, right } = this.cameraBasis();
    const canvasAspect = this.canvas.clientWidth / Math.max(this.canvas.clientHeight, 1);
    const aspect = canvasAspect > 0 ? canvasAspect : Math.max(this.camera.aspectRatio || 16 / 9, 0.01);
    const halfVerticalFov = (this.camera.fov * Math.PI) / 360;
    const viewTarget = this.target.clone().add(
      right.mulScalar(this.distance * Math.tan(halfVerticalFov) * aspect * this.horizontalFrameOffset),
    );
    const position = viewTarget.clone().add(offset.mulScalar(this.distance));
    this.cameraEntity.setPosition(position);
    this.cameraEntity.lookAt(viewTarget);
    this.updateClipPlanes(position, forward);
  }

  private updateClipPlanes(cameraPosition: Vec3, forward: Vec3) {
    const bounds = this.sceneBounds;
    if (!bounds) {
      this.camera.nearClip = Math.max(this.distance * 0.001, 0.0001);
      this.camera.farClip = Math.max(this.distance * 10, 100);
      return;
    }

    const center = bounds.center;
    const half = bounds.halfExtents;
    let minimumDepth = Number.POSITIVE_INFINITY;
    let maximumDepth = 0;
    let crossesCameraPlane = false;

    for (const x of [-half.x, half.x]) {
      for (const y of [-half.y, half.y]) {
        for (const z of [-half.z, half.z]) {
          const depth = new Vec3(center.x + x, center.y + y, center.z + z)
            .sub(cameraPosition)
            .dot(forward);
          if (depth <= 0) crossesCameraPlane = true;
          else minimumDepth = Math.min(minimumDepth, depth);
          maximumDepth = Math.max(maximumDepth, depth);
        }
      }
    }

    const near = crossesCameraPlane || !Number.isFinite(minimumDepth)
      ? 0.0001
      : Math.max(minimumDepth * 0.2, 0.0001);
    this.camera.nearClip = near;
    this.camera.farClip = Math.max(maximumDepth * 1.25, near + 1, 10);
  }

  private preventContextMenu = (event: MouseEvent) => event.preventDefault();

  private cancelViewTransition() {
    if (this.transitionFrame) cancelAnimationFrame(this.transitionFrame);
    this.transitionFrame = 0;
    this.transitioning = false;
  }

  private onPointerDown = (event: PointerEvent) => {
    if (this.destroyed || this.transitioning || !this.enabled || (event.button !== this.orbitButton && event.button !== 2)) return;
    this.pointerId = event.pointerId;
    this.mode = event.button === this.orbitButton ? "orbit" : "pan";
    this.startX = event.clientX;
    this.startY = event.clientY;
    this.lastX = event.clientX;
    this.lastY = event.clientY;
    this.dragging = false;
    this.canvas.setPointerCapture(event.pointerId);
  };

  private onPointerMove = (event: PointerEvent) => {
    if (this.destroyed || !this.enabled || this.pointerId !== event.pointerId || !this.mode) return;
    if (!this.dragging) {
      const distance = Math.hypot(event.clientX - this.startX, event.clientY - this.startY);
      if (distance < ViewerControls.DRAG_THRESHOLD) return;
      this.dragging = true;
    }
    const dx = event.clientX - this.lastX;
    const dy = event.clientY - this.lastY;
    this.lastX = event.clientX;
    this.lastY = event.clientY;
    if (this.mode === "orbit") {
      if (this.orthographicView) {
        const direction = this.cameraEntity.getPosition().clone().sub(this.target).normalize();
        this.distance = Math.max(
          this.camera.orthoHeight / Math.max(Math.tan(this.camera.fov * Math.PI / 360), 0.001),
          0.01,
        );
        this.yaw = Math.atan2(direction.x, direction.z) * 180 / Math.PI;
        this.pitch = Math.asin(Math.max(-1, Math.min(1, direction.y))) * 180 / Math.PI;
        this.orthographicView = null;
        this.camera.projection = PROJECTION_PERSPECTIVE;
        this.horizontalFrameOffset = 0;
        this.onOrthographicViewChange?.(null);
      }
      this.yaw -= dx * 0.3;
      this.pitch = Math.max(-89, Math.min(89, this.pitch + dy * 0.3));
    } else {
      const speed = this.distance * 0.0016;
      this.target.add(this.cameraEntity.right.clone().mulScalar(-dx * speed));
      this.target.add(this.cameraEntity.up.clone().mulScalar(dy * speed));
    }
    this.updateCamera();
  };

  private onPointerUp = (event: PointerEvent) => {
    if (this.pointerId !== event.pointerId) return;
    if (this.canvas.hasPointerCapture(event.pointerId)) this.canvas.releasePointerCapture(event.pointerId);
    this.cancelGesture();
  };

  private onWheel = (event: WheelEvent) => {
    if (this.destroyed || this.transitioning || !this.enabled) return;
    event.preventDefault();
    const modeScale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? Math.max(this.canvas.clientHeight, 800) : 1;
    const delta = Math.max(-240, Math.min(240, event.deltaY * modeScale));
    if (this.camera.projection === PROJECTION_ORTHOGRAPHIC) {
      this.camera.orthoHeight = Math.max(0.0001, Math.min(1_000_000, this.camera.orthoHeight * Math.exp(delta * 0.001)));
    } else {
      this.distance = Math.max(0.01, Math.min(1_000_000, this.distance * Math.exp(delta * 0.001)));
    }
    this.updateCamera();
  };
}
