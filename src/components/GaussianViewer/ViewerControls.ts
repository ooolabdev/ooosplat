import { BoundingBox, Entity, Mat4, PROJECTION_ORTHOGRAPHIC, PROJECTION_PERSPECTIVE, Quat, Vec3, type CameraComponent } from "playcanvas";
import type { GaussianOrthographicView } from "../../types/pipeline";
import { DEFAULT_ROAM_SPEED, validRoamSpeed } from "./RoamPreferences";
import { ViewerPointerLock } from "./ViewerPointerLock";

const DEFAULT_YAW = 35;
const DEFAULT_PITCH = 22;
const DEFAULT_OCCUPANCY = 0.85;
const VIEW_TRANSITION_MS = 320;
const ROLL_LEVEL_MS = 180;
const MAX_ORBIT_PITCH = 89.9;
const ORBIT_POLE_EPSILON = 1e-6;
const ROAM_VISIBLE_HEIGHT_PER_SECOND = 0.15;

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
  roll?: number;
  fov?: number;
  /** Principal-point offset in source-image-height units (image Y points down). */
  projectionOffset?: [number, number];
  projectionScaleX?: number;
}

export interface ReferenceView {
  position: Vec3;
  forward: Vec3;
  up: Vec3;
  fov: number;
  projectionOffset: [number, number];
  projectionScaleX: number;
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
  private roll = 0;
  private projectionOffset: [number, number] = [0, 0];
  private projectionScaleX = 1;
  private roamingEnabled = false;
  private roamSpeed = DEFAULT_ROAM_SPEED;
  private readonly keys = new Set<string>();
  private roamFrame = 0;
  private roamLastAt = 0;
  private roamMoved = false;
  private lookX = 0;
  private lookY = 0;
  private readonly mouseLook: ViewerPointerLock;
  private shifted = false;
  private readonly previousTabIndex: number;
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
  private locating = false;
  private leveling = false;

  constructor(
    canvas: HTMLCanvasElement,
    cameraEntity: Entity,
    private readonly onOrthographicViewChange?: (view: GaussianOrthographicView | null, orthographic: boolean) => void,
    cameraError = "The preview camera component is unavailable",
    private readonly onViewChange?: () => void,
    private readonly onMotion?: () => void,
  ) {
    if (!cameraEntity.camera) throw new Error(cameraError);
    this.canvas = canvas;
    this.cameraEntity = cameraEntity;
    this.camera = cameraEntity.camera;
    this.mouseLook = new ViewerPointerLock(canvas, this.stopRoaming);
    this.previousTabIndex = canvas.tabIndex;
    canvas.tabIndex = 0;
    canvas.addEventListener("pointerdown", this.focusCanvas, true);
    canvas.addEventListener("blur", this.stopRoaming);
    canvas.addEventListener("pointercancel", this.stopRoaming);
    window.addEventListener("keydown", this.onKeyDown);
    window.addEventListener("keyup", this.onKeyUp);
    window.addEventListener("blur", this.stopRoaming);
    document.addEventListener("visibilitychange", this.stopRoaming);
    document.addEventListener("mousemove", this.onMouseLook);
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
    this.stopRoaming();
    this.destroyed = true;
    this.mouseLook.destroy();
    this.cancelViewTransition();
    this.cancelGesture();
    this.canvas.removeEventListener("pointerdown", this.onPointerDown);
    this.canvas.removeEventListener("pointermove", this.onPointerMove);
    this.canvas.removeEventListener("pointerup", this.onPointerUp);
    this.canvas.removeEventListener("pointercancel", this.onPointerUp);
    this.canvas.removeEventListener("wheel", this.onWheel);
    this.canvas.removeEventListener("contextmenu", this.preventContextMenu);
    this.canvas.removeEventListener("pointerdown", this.focusCanvas, true);
    this.canvas.removeEventListener("blur", this.stopRoaming);
    this.canvas.removeEventListener("pointercancel", this.stopRoaming);
    window.removeEventListener("keydown", this.onKeyDown);
    window.removeEventListener("keyup", this.onKeyUp);
    window.removeEventListener("blur", this.stopRoaming);
    document.removeEventListener("visibilitychange", this.stopRoaming);
    document.removeEventListener("mousemove", this.onMouseLook);
    this.canvas.tabIndex = this.previousTabIndex;
    this.camera.calculateProjection = null!;
  }

  get enabled() {
    return this._enabled;
  }

  set enabled(value: boolean) {
    this._enabled = value;
    if (!value) {
      this.cancelGesture(); this.stopRoaming();
      if (this.locating || this.leveling) this.stopNavigation();
    }
  }

  setRoamingEnabled(value: boolean) {
    this.roamingEnabled = value;
    if (!value) { this.stopRoaming(); if (this.locating || this.leveling) this.stopNavigation(); }
  }

  setRoamSpeed(value: number) {
    this.roamSpeed = validRoamSpeed(value) ? value : DEFAULT_ROAM_SPEED;
  }

  isMouseLookActive() {
    return this.mouseLook.ownsLock();
  }

  focus(): boolean {
    if (this.destroyed || !this.enabled) return false;
    this.canvas.focus({ preventScroll: true });
    return document.activeElement === this.canvas;
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
      roll: this.roll,
      fov: this.camera.fov,
      projectionOffset: [...this.projectionOffset],
      projectionScaleX: this.projectionScaleX,
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
    this.stopRoaming();
    this.cancelViewTransition();
    this.target.set(...state.target);
    this.yaw = state.yaw;
    this.pitch = state.pitch;
    this.distance = state.distance;
    this.roll = state.roll ?? 0;
    this.camera.fov = state.fov ?? 52;
    this.projectionOffset = [...(state.projectionOffset ?? [0, 0])];
    this.projectionScaleX = state.projectionScaleX ?? 1;
    this.horizontalFrameOffset = state.horizontalFrameOffset;
    this.camera.projection = state.projection;
    this.camera.orthoHeight = state.orthoHeight;
    this.orthographicView = state.orthographicView;
    this.onOrthographicViewChange?.(state.orthographicView, state.projection === PROJECTION_ORTHOGRAPHIC);
    this.updateCamera();
  }

  locate(reference: ReferenceView, bounds: BoundingBox, animate = true) {
    if (this.destroyed || !this.enabled) return;
    this.stopRoaming();
    this.cancelGesture();
    const startPosition = this.cameraEntity.getPosition().clone();
    const startRotation = this.cameraEntity.getRotation().clone();
    const startFov = this.camera.fov;
    const startHeight = this.camera.orthoHeight;
    const startDistance = this.distance;
    const depth = bounds.center.clone().sub(reference.position).dot(reference.forward);
    const distance = depth > 0.0001 ? depth : Math.max(bounds.halfExtents.length(), 0.01);
    const destination = this.snapshot();
    destination.horizontalFrameOffset = 0;
    // Locate always restores the actual training camera, including when invoked
    // from a fixed or freely rotated orthographic view.
    const forward = reference.forward.clone().normalize();
    destination.target = reference.position.clone().add(forward.clone().mulScalar(distance)).toArray() as [number, number, number];
    destination.distance = distance;
    destination.yaw = Math.hypot(forward.x, forward.z) < ORBIT_POLE_EPSILON ? 0 : Math.atan2(-forward.x, -forward.z) * 180 / Math.PI;
    destination.pitch = Math.asin(Math.max(-1, Math.min(1, -forward.y))) * 180 / Math.PI;
    const yaw = destination.yaw * Math.PI / 180;
    const right = new Vec3(Math.cos(yaw), 0, -Math.sin(yaw));
    const up = new Vec3().cross(right, forward).normalize();
    destination.roll = Math.atan2(reference.up.dot(right), reference.up.dot(up)) * 180 / Math.PI;
    destination.projection = PROJECTION_PERSPECTIVE;
    destination.orthographicView = null;
    destination.fov = reference.fov;
    destination.projectionOffset = [...reference.projectionOffset];
    destination.projectionScaleX = reference.projectionScaleX;
    this.restore(destination);
    // A button click transfers focus away from the canvas. Return it synchronously,
    // never at animation completion where it could steal a newer dialog's focus.
    this.focusCanvas();
    if (!animate || this.destroyed) { this.onViewChange?.(); return; }
    const endPosition = this.cameraEntity.getPosition().clone();
    const endRotation = this.cameraEntity.getRotation().clone();
    const endHeight = this.camera.orthoHeight;
    const endFov = this.camera.fov;
    const started = performance.now();
    this.transitioning = true;
    this.locating = true;
    const tick = (now: number) => {
      if (this.destroyed || !this.transitioning) return;
      if (!this.enabled) { this.cancelViewTransition(); this.updateCamera(); this.onViewChange?.(); return; }
      const linear = Math.max(0, Math.min(1, (now - started) / VIEW_TRANSITION_MS));
      const t = 1 - (1 - linear) ** 3;
      const position = new Vec3().lerp(startPosition, endPosition, t);
      const rotation = new Quat().slerp(startRotation, endRotation, t);
      const forward = rotation.transformVector(new Vec3(0, 0, -1));
      const up = rotation.transformVector(Vec3.UP);
      this.camera.fov = startFov + (endFov - startFov) * t;
      this.camera.orthoHeight = startHeight + (endHeight - startHeight) * t;
      this.distance = startDistance + (distance - startDistance) * t;
      this.applyCamera(position, position.clone().add(forward), up);
      this.onMotion?.();
      if (linear < 1) this.transitionFrame = requestAnimationFrame(tick);
      else { this.transitioning = false; this.locating = false; this.transitionFrame = 0; this.updateCamera(); this.onViewChange?.(); }
    };
    this.transitionFrame = requestAnimationFrame(tick);
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
    if (!this.enabled && this.transitioning) {
      this.updateClipPlanes(this.cameraEntity.getPosition(), this.cameraEntity.forward);
      return;
    }
    this.cancelViewTransition();
    this.updateCamera();
  }

  setRectangleSelectionMode(enabled: boolean) {
    this.stopNavigation();
    this.orbitButton = enabled ? 1 : 0;
    this.cancelGesture();
  }

  alignOrthographic(view: GaussianOrthographicView, bounds: BoundingBox, occupancy = 0.85, animate = true) {
    this.stopRoaming();
    this.cancelViewTransition();
    this.cancelGesture();
    const startPosition = this.cameraEntity.getPosition().clone();
    const startTarget = this.target.clone();
    const startUp = this.cameraEntity.up.clone();
    const startOrthoHeight = this.camera.projection === PROJECTION_ORTHOGRAPHIC
      ? this.camera.orthoHeight
      : Math.max(this.distance * Math.tan(this.camera.fov * Math.PI / 360), 0.001);
    this.orthographicView = view;
    this.onOrthographicViewChange?.(view, true);
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
    this.roll = 0;
    this.projectionOffset = [0, 0];
    this.projectionScaleX = 1;
    this.onViewChange?.();
    if (!animate || this.destroyed) {
      this.updateCamera();
      return;
    }

    this.transitioning = true;
    let startedAt = performance.now();
    let previousFrameAt = startedAt;
    const position = new Vec3();
    const lookTarget = new Vec3();
    const up = new Vec3();
    const tick = (now: number) => {
      if (this.destroyed || !this.transitioning) return;
      if (!this.enabled) {
        startedAt += now - previousFrameAt;
        previousFrameAt = now;
        this.transitionFrame = requestAnimationFrame(tick);
        return;
      }
      previousFrameAt = now;
      const linear = Math.min(1, Math.max(0, (now - startedAt) / VIEW_TRANSITION_MS));
      const progress = 1 - Math.pow(1 - linear, 3);
      position.lerp(startPosition, destinationPosition, progress);
      lookTarget.lerp(startTarget, destinationTarget, progress);
      up.lerp(startUp, destinationUp, progress).normalize();
      this.camera.orthoHeight = startOrthoHeight + (destinationOrthoHeight - startOrthoHeight) * progress;
      this.applyCamera(position, lookTarget, up);
      if (linear < 1) {
        this.transitionFrame = requestAnimationFrame(tick);
      } else {
        this.transitioning = false;
        this.transitionFrame = 0;
        this.updateCamera();
        this.onViewChange?.();
      }
    };
    this.transitionFrame = requestAnimationFrame(tick);
  }

  usePerspective() {
    this.stopRoaming();
    this.cancelViewTransition();
    this.orthographicView = null;
    this.onOrthographicViewChange?.(null, false);
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
      this.roll = 0;
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
    const baseRight = new Vec3(Math.cos(yaw), 0, -Math.sin(yaw));
    const baseUp = new Vec3().cross(baseRight, forward).normalize();
    const roll = this.roll * Math.PI / 180;
    const right = baseRight.clone().mulScalar(Math.cos(roll)).sub(baseUp.clone().mulScalar(Math.sin(roll)));
    const up = baseUp.mulScalar(Math.cos(roll)).add(baseRight.mulScalar(Math.sin(roll)));
    return { offset, forward, right, up };
  }

  private updateCamera() {
    if (this.orthographicView) {
      const position = this.target.clone();
      if (this.orthographicView === "side") position.x += this.distance;
      else if (this.orthographicView === "front") position.z += this.distance;
      else position.y += this.distance;
      this.applyCamera(position, this.target, this.orthographicView === "top" ? new Vec3(0, 0, -1) : Vec3.UP);
      return;
    }
    const { offset, right, up } = this.cameraBasis();
    const canvasAspect = this.canvas.clientWidth / Math.max(this.canvas.clientHeight, 1);
    const aspect = canvasAspect > 0 ? canvasAspect : Math.max(this.camera.aspectRatio || 16 / 9, 0.01);
    const halfVerticalFov = (this.camera.fov * Math.PI) / 360;
    const viewTarget = this.target.clone().add(
      right.mulScalar(this.distance * Math.tan(halfVerticalFov) * aspect * this.horizontalFrameOffset),
    );
    const position = viewTarget.clone().add(offset.mulScalar(this.distance));
    this.applyCamera(position, viewTarget, up);
  }

  private applyCamera(position: Vec3, lookTarget: Vec3, up: Vec3) {
    const forward = lookTarget.clone().sub(position).normalize();
    const bounds = this.sceneBounds;
    if (this.camera.projection === PROJECTION_ORTHOGRAPHIC && bounds) {
      const half = bounds.halfExtents;
      const radius = Math.abs(forward.x) * half.x
        + Math.abs(forward.y) * half.y
        + Math.abs(forward.z) * half.z;
      const minimumDepth = bounds.center.clone().sub(position).dot(forward) - radius;
      const margin = Math.max(half.length() * 0.01, 0.001);
      if (minimumDepth < margin) {
        // Orthographic scale depends on orthoHeight, not camera distance. Move only along the
        // viewing axis to keep the whole scene in front without changing its screen framing.
        position.sub(forward.clone().mulScalar(margin - minimumDepth));
      }
    }
    this.cameraEntity.setPosition(position);
    this.cameraEntity.lookAt(lookTarget, up);
    this.updateClipPlanes(position, forward);
    this.camera.calculateProjection = this.projectionOffset.some(value => value !== 0) || this.projectionScaleX !== 1
      ? this.calculateProjection : null!;
    this.onMotion?.();
  }

  private calculateProjection = (matrix: Mat4) => {
    const aspect = Math.max(this.camera.aspectRatio || 1, 0.0001);
    if (this.camera.projection === PROJECTION_ORTHOGRAPHIC) {
      const h = this.camera.orthoHeight;
      matrix.setOrtho(-h * aspect, h * aspect, -h, h, this.camera.nearClip, this.camera.farClip);
      matrix.data[0] *= this.projectionScaleX;
    } else {
      matrix.setPerspective(this.camera.fov, aspect, this.camera.nearClip, this.camera.farClip);
      matrix.data[0] *= this.projectionScaleX;
      matrix.data[8] = -2 * this.projectionOffset[0] / aspect;
      matrix.data[9] = 2 * this.projectionOffset[1];
    }
  };

  private visibleHeight() {
    return this.camera.projection === PROJECTION_ORTHOGRAPHIC ? this.camera.orthoHeight * 2
      : 2 * this.distance * Math.tan(this.camera.fov * Math.PI / 360);
  }

  private focusCanvas = () => { if (this.enabled) this.canvas.focus({ preventScroll: true }); };
  private stopRoaming = () => {
    this.applyMouseLook();
    if (this.roamFrame) cancelAnimationFrame(this.roamFrame);
    this.roamFrame = 0;
    this.keys.clear();
    this.shifted = false;
    this.mouseLook.release();
    if (this.roamMoved) { this.roamMoved = false; this.onViewChange?.(); }
  };

  private onKeyDown = (event: KeyboardEvent) => {
    if (event.code === "Escape" && (this.keys.size || this.isMouseLookActive())) {
      event.preventDefault();
      event.stopImmediatePropagation();
      this.stopRoaming();
      return;
    }
    if (event.ctrlKey || event.metaKey || event.altKey) { this.stopRoaming(); return; }
    if (this.destroyed || !this.enabled || !this.roamingEnabled || this.camera.projection !== PROJECTION_PERSPECTIVE
      || document.activeElement !== this.canvas || event.target !== this.canvas || this.pointerId !== null) return;
    this.shifted = event.shiftKey;
    if (!["KeyW", "KeyA", "KeyS", "KeyD"].includes(event.code)) return;
    if (event.repeat && !this.keys.has(event.code)) return;
    event.preventDefault();
    this.cancelViewTransition(true);
    const entering = this.keys.size === 0;
    this.keys.add(event.code);
    if (entering) this.mouseLook.request();
    if (!this.roamFrame) { this.roamLastAt = performance.now(); this.roamFrame = requestAnimationFrame(this.roam); }
  };

  private onKeyUp = (event: KeyboardEvent) => {
    this.shifted = event.shiftKey;
    this.keys.delete(event.code);
    if (!this.keys.size) this.stopRoaming();
  };

  private onMouseLook = (event: MouseEvent) => {
    if (!this.isMouseLookActive() || this.destroyed || !this.enabled || !this.roamingEnabled
      || document.activeElement !== this.canvas || !this.keys.size || this.camera.projection !== PROJECTION_PERSPECTIVE) return;
    if (Number.isFinite(event.movementX)) this.lookX += event.movementX;
    if (Number.isFinite(event.movementY)) this.lookY += event.movementY;
  };

  private applyMouseLook() {
    const dx = this.lookX, dy = this.lookY;
    this.lookX = 0; this.lookY = 0;
    if ((!dx && !dy) || this.destroyed) return;
    const position = this.cameraEntity.getPosition().clone();
    this.yaw -= dx * 0.3;
    this.pitch = Math.max(-MAX_ORBIT_PITCH, Math.min(MAX_ORBIT_PITCH, this.pitch + dy * 0.3));
    const { forward, right } = this.cameraBasis();
    const aspect = this.canvas.clientWidth / Math.max(this.canvas.clientHeight, 1)
      || Math.max(this.camera.aspectRatio || 16 / 9, 0.01);
    const shift = this.distance * Math.tan(this.camera.fov * Math.PI / 360) * aspect * this.horizontalFrameOffset;
    // Reconstruct the orbit target around an unchanged eye, including the
    // sidebar framing offset. Ordinary orbit would move the eye around target.
    this.target.copy(position).add(forward.mulScalar(this.distance)).sub(right.mulScalar(shift));
    this.updateCamera();
    this.roamMoved = true;
  }

  private roam = (now: number) => {
    this.roamFrame = 0;
    if (this.destroyed || !this.enabled || !this.roamingEnabled || document.activeElement !== this.canvas
      || this.camera.projection !== PROJECTION_PERSPECTIVE || !this.keys.size) { this.stopRoaming(); return; }
    // A queued frame can predate keydown. Never move the clock backwards and
    // count that stale interval again on the next frame.
    const sampleAt = Math.max(this.roamLastAt, now);
    const dt = Math.min(0.05, (sampleAt - this.roamLastAt) / 1000);
    this.roamLastAt = sampleAt;
    this.applyMouseLook();
    const forward = Number(this.keys.has("KeyW")) - Number(this.keys.has("KeyS"));
    const right = Number(this.keys.has("KeyD")) - Number(this.keys.has("KeyA"));
    // Forward/backward follows the current sight line, including pitch. Strafe
    // stays world-horizontal and roll-independent, even at a vertical view.
    const yaw = this.yaw * Math.PI / 180;
    const movement = this.cameraBasis().forward.mulScalar(forward)
      .add(new Vec3(Math.cos(yaw) * right, 0, -Math.sin(yaw) * right));
    if (movement.lengthSq() > 0) {
      movement.normalize().mulScalar(ROAM_VISIBLE_HEIGHT_PER_SECOND * this.roamSpeed * this.visibleHeight() * dt * (this.shifted ? 3 : 1));
      this.target.add(movement);
      // A rigid translation does not need lookAt again; at a vertical view its
      // tiny horizontal direction components can otherwise perturb the roll.
      const position = this.cameraEntity.getPosition().clone().add(movement);
      this.cameraEntity.setPosition(position);
      this.updateClipPlanes(position, this.cameraEntity.forward);
      this.onMotion?.();
      this.roamMoved = true;
    }
    this.roamFrame = requestAnimationFrame(this.roam);
  };

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

  private cancelViewTransition(preservePose = false) {
    if (preservePose && this.transitioning && !this.leveling) {
      const forward = this.cameraEntity.forward.clone();
      const up = this.cameraEntity.up.clone();
      this.yaw = Math.hypot(forward.x, forward.z) < ORBIT_POLE_EPSILON ? 0 : Math.atan2(-forward.x, -forward.z) * 180 / Math.PI;
      this.pitch = Math.asin(Math.max(-1, Math.min(1, -forward.y))) * 180 / Math.PI;
      const yaw = this.yaw * Math.PI / 180;
      const right = new Vec3(Math.cos(yaw), 0, -Math.sin(yaw));
      const baseUp = new Vec3().cross(right, forward).normalize();
      this.roll = Math.atan2(up.dot(right), up.dot(baseUp)) * 180 / Math.PI;
      this.target.copy(this.cameraEntity.getPosition()).add(forward.mulScalar(this.distance));
      this.horizontalFrameOffset = 0;
      this.orthographicView = null;
      this.onOrthographicViewChange?.(null, this.camera.projection === PROJECTION_ORTHOGRAPHIC);
    }
    if (this.transitionFrame) cancelAnimationFrame(this.transitionFrame);
    this.transitionFrame = 0;
    this.transitioning = false;
    this.locating = false;
    this.leveling = false;
  }

  private beginRollLeveling() {
    if (this.leveling) return;
    const startRoll = ((this.roll + 180) % 360 + 360) % 360 - 180;
    if (Math.abs(startRoll) < 1e-6) { this.roll = 0; return; }
    this.cancelViewTransition();
    this.roll = startRoll;
    this.transitioning = true;
    this.leveling = true;
    const started = performance.now();
    const tick = (now: number) => {
      if (this.destroyed || !this.leveling) return;
      const t = Math.max(0, Math.min(1, (now - started) / ROLL_LEVEL_MS));
      this.roll = t === 1 ? 0 : startRoll * (1 - t) ** 3;
      this.updateCamera();
      if (t < 1) this.transitionFrame = requestAnimationFrame(tick);
      else {
        this.transitionFrame = 0;
        this.transitioning = false;
        this.leveling = false;
        this.onViewChange?.();
      }
    };
    this.transitionFrame = requestAnimationFrame(tick);
  }

  stopNavigation() {
    this.stopRoaming(); this.cancelGesture();
    if (this.transitioning) {
      this.cancelViewTransition(true);
      this.updateCamera();
      this.onViewChange?.();
    }
  }

  private onPointerDown = (event: PointerEvent) => {
    if (this.isMouseLookActive()) { event.preventDefault(); return; }
    this.stopRoaming();
    if (this.destroyed || !this.enabled || (event.button !== this.orbitButton && event.button !== 2)) return;
    this.cancelViewTransition(true);
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
    if (this.isMouseLookActive()) return;
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
        const horizontalLength = Math.hypot(direction.x, direction.z);
        this.yaw = horizontalLength < ORBIT_POLE_EPSILON
          ? 0
          : Math.atan2(direction.x, direction.z) * 180 / Math.PI;
        this.pitch = Math.max(
          -MAX_ORBIT_PITCH,
          Math.min(
            MAX_ORBIT_PITCH,
            Math.asin(Math.max(-1, Math.min(1, direction.y))) * 180 / Math.PI,
          ),
        );
        this.orthographicView = null;
        this.onOrthographicViewChange?.(null, true);
      }
      this.beginRollLeveling();
      this.yaw -= dx * 0.3;
      this.pitch = Math.max(-MAX_ORBIT_PITCH, Math.min(MAX_ORBIT_PITCH, this.pitch + dy * 0.3));
    } else {
      const speed = this.visibleHeight() / Math.max(this.canvas.clientHeight, 1);
      this.target.add(this.cameraEntity.right.clone().mulScalar(-dx * speed / this.projectionScaleX));
      this.target.add(this.cameraEntity.up.clone().mulScalar(dy * speed));
    }
    this.updateCamera();
  };

  private onPointerUp = (event: PointerEvent) => {
    if (this.pointerId !== event.pointerId) return;
    if (this.canvas.hasPointerCapture(event.pointerId)) this.canvas.releasePointerCapture(event.pointerId);
    this.cancelGesture();
    this.onViewChange?.();
  };

  private onWheel = (event: WheelEvent) => {
    if (this.isMouseLookActive()) { event.preventDefault(); return; }
    if (this.destroyed || !this.enabled) return;
    this.cancelViewTransition(true);
    event.preventDefault();
    const modeScale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? Math.max(this.canvas.clientHeight, 800) : 1;
    const delta = Math.max(-240, Math.min(240, event.deltaY * modeScale));
    if (this.camera.projection === PROJECTION_ORTHOGRAPHIC) {
      this.camera.orthoHeight = Math.max(0.0001, Math.min(1_000_000, this.camera.orthoHeight * Math.exp(delta * 0.001)));
    } else {
      this.distance = Math.max(0.01, Math.min(1_000_000, this.distance * Math.exp(delta * 0.001)));
    }
    this.updateCamera();
    this.onViewChange?.();
  };
}
