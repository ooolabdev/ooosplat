// @vitest-environment jsdom

import { BoundingBox, GraphNode, Mat4, PROJECTION_ORTHOGRAPHIC, PROJECTION_PERSPECTIVE, Quat, Vec3, type Entity } from "playcanvas";
import { describe, expect, it, vi } from "vitest";
import { ViewerControls } from "./ViewerControls";
import { buildGroundGrid } from "./GroundGrid";
import { robustEffectBounds } from "./PreviewAnimation";

function pointerEvent(type: string, x: number, y: number, button = 0) {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, button, buttons: type === "pointerup" ? 0 : 1, clientX: x, clientY: y });
  Object.defineProperty(event, "pointerId", { value: 7 });
  return event;
}

function setup(width = 1200, height = 800) {
  const canvas = document.createElement("canvas");
  const captures = new Set<number>();
  Object.defineProperties(canvas, {
    clientWidth: { value: width },
    clientHeight: { value: height },
    setPointerCapture: { value: (id: number) => captures.add(id) },
    releasePointerCapture: { value: (id: number) => captures.delete(id) },
    hasPointerCapture: { value: (id: number) => captures.has(id) },
  });
  const camera = { fov: 52, aspectRatio: 1.5, nearClip: 0.01, farClip: 10_000,
    projection: PROJECTION_PERSPECTIVE, orthoHeight: 5 };
  const entity = new GraphNode();
  Object.defineProperty(entity, "camera", { value: camera });
  const setPosition = vi.spyOn(entity, "setPosition");
  const lookAt = vi.spyOn(entity, "lookAt");
  const onOrthographicViewChange = vi.fn();
  const onViewChange = vi.fn();
  const onMotion = vi.fn();
  const controls = new ViewerControls(canvas, entity as Entity, onOrthographicViewChange, undefined, onViewChange, onMotion);
  return { canvas, camera, controls, entity, setPosition, lookAt, onOrthographicViewChange, onViewChange, onMotion };
}

describe("ViewerControls", () => {
  it.each(["perspective", "orthographic"])("pans at the current %s pixel scale immediately after repeated zoom, without orbiting", projection => {
    const { controls, canvas, camera } = setup();
    if (projection === "orthographic") {
      controls.alignOrthographic("front", new BoundingBox(new Vec3(), new Vec3(1, 1, 1)), 0.85, false);
      controls.setSceneBounds(new BoundingBox(new Vec3(), new Vec3(100, 1, 100)));
    }
    for (let i = 0; i < 15; i++) canvas.dispatchEvent(new WheelEvent("wheel", { deltaY: -200, cancelable: true }));
    const before = controls.snapshot();
    canvas.dispatchEvent(pointerEvent("pointerdown", 40, 40, 2));
    canvas.dispatchEvent(pointerEvent("pointermove", 60, 40, 2));
    canvas.dispatchEvent(pointerEvent("pointerup", 60, 40, 2));
    const after = controls.snapshot();
    const height = projection === "orthographic" ? 2 * camera.orthoHeight : 2 * before.distance * Math.tan(camera.fov * Math.PI / 360);
    expect(new Vec3(...after.target).sub(new Vec3(...before.target)).length()).toBeCloseTo(20 * height / 800, 8);
    expect(after.distance).toBe(before.distance);
    controls.restore(before);
    canvas.dispatchEvent(pointerEvent("pointerdown", 0, 0));
    canvas.dispatchEvent(pointerEvent("pointermove", 10, 0));
    canvas.dispatchEvent(pointerEvent("pointerup", 10, 0));
    const rotated = controls.snapshot();
    canvas.dispatchEvent(pointerEvent("pointerdown", 40, 40, 2));
    canvas.dispatchEvent(pointerEvent("pointermove", 60, 40, 2));
    canvas.dispatchEvent(pointerEvent("pointerup", 60, 40, 2));
    expect(new Vec3(...controls.snapshot().target).sub(new Vec3(...rotated.target)).length()).toBeCloseTo(20 * height / 800, 8);
    controls.destroy();
  });

  it("restores a full reference pose including roll, FOV and off-center projection", () => {
    const { controls, entity, camera } = setup();
    const reference = { position: new Vec3(3, 4, 5), forward: new Vec3(1, -1, -1).normalize(), up: new Vec3(1, 1, 0).normalize(),
      fov: 80, projectionOffset: [0.1, -0.08] as [number, number], projectionScaleX: 1.2 };
    controls.locate(reference, new BoundingBox(new Vec3(8, -1, 0), new Vec3(2, 2, 2)), false);
    expect(entity.getPosition().distance(reference.position)).toBeLessThan(1e-5);
    expect(entity.forward.dot(reference.forward)).toBeCloseTo(1, 6);
    expect(entity.up.dot(reference.up)).toBeCloseTo(1, 6);
    expect(camera.fov).toBe(80);
    const saved = controls.snapshot();
    const projection = new Mat4();
    (camera as unknown as { calculateProjection: (m: Mat4) => void }).calculateProjection(projection);
    expect(projection.data[8]).toBeCloseTo(-0.2 / camera.aspectRatio);
    expect(projection.data[9]).toBeCloseTo(-0.16);
    controls.orbitBy(45);
    controls.restore(saved);
    expect(entity.getPosition().distance(reference.position)).toBeLessThan(1e-5);
    expect(entity.up.dot(reference.up)).toBeCloseTo(1, 6);
    controls.destroy();
  });

  it.each(["front", "side", "top"] as const)("returns from %s to the first training perspective camera", view => {
    const { controls, camera, entity, onOrthographicViewChange } = setup();
    const bounds = new BoundingBox(new Vec3(50, 50, 50), new Vec3(5, 5, 5));
    controls.alignOrthographic(view, bounds, 0.85, false);
    controls.locate({ position: new Vec3(50, 50, 30), forward: new Vec3(0, 0, 1), up: Vec3.UP,
      fov: 90, projectionOffset: [0, 0], projectionScaleX: 1 }, bounds, false);
    expect(controls.snapshot().target).toEqual([50, 50, 50]);
    expect(controls.snapshot().orthographicView).toBeNull();
    expect(camera.projection).toBe(PROJECTION_PERSPECTIVE);
    expect(entity.getPosition().distance(new Vec3(50, 50, 30))).toBeLessThan(1e-5);
    expect(entity.forward.dot(new Vec3(0, 0, 1))).toBeCloseTo(1);
    expect(camera.fov).toBe(90);
    expect(onOrthographicViewChange).toHaveBeenLastCalledWith(null, false);
    controls.destroy();
  });

  it("keeps pole reference cameras finite and upright without requiring a first orbit", () => {
    const { controls, entity } = setup();
    controls.locate({ position: new Vec3(0, 3, 0), forward: new Vec3(0, -1, 0), up: new Vec3(0, 0, -1),
      fov: 70, projectionOffset: [0, 0], projectionScaleX: 1 }, new BoundingBox(new Vec3(), new Vec3(1, 1, 1)), false);
    expect(entity.forward.y).toBeCloseTo(-1);
    expect(entity.up.z).toBeCloseTo(-1);
    expect(entity.getPosition().y).toBeCloseTo(3);
    expect(controls.snapshot().pitch).toBeCloseTo(90);
    controls.destroy();
  });

  it.each(["front", "side", "top"] as const)("restores the transformed training pose from %s without orthographic safety retreat", view => {
    const { controls, camera, entity } = setup();
    try {
      const centers = Array.from({ length: 300 }, (_, i) => [i % 2 ? -1 : 1, i % 3 ? -2 : 2, i % 5 ? -3 : 3]).flat();
      centers.push(10_000, 10_000, 10_000);
      const robust = robustEffectBounds(centers, { center: [5000, 5000, 5000], halfExtents: [5001, 5002, 5003] });
      const rotation = new Quat().setFromEulerAngles(180, 27, -12);
      const transform = new Mat4().setTRS(new Vec3(2, 3, 4), rotation, new Vec3(2, 2, 2));
      const bounds = new BoundingBox();
      bounds.setFromTransformedAabb(new BoundingBox(new Vec3(...robust.center), new Vec3(...robust.halfExtents)), transform);
      const forward = rotation.transformVector(new Vec3(0, 0, -1));
      const reference = { position: bounds.center.clone().sub(forward.clone().mulScalar(12)), forward,
        up: rotation.transformVector(Vec3.UP), fov: 64, projectionOffset: [0.1, -0.1] as [number, number], projectionScaleX: 1.3 };
      controls.setSceneBounds(new BoundingBox(new Vec3(), new Vec3(1000, 1000, 1000)));
      controls.alignOrthographic(view, bounds, 0.85, false);
      controls.locate(reference, bounds, false);
      const state = controls.snapshot();
      expect(new Vec3(...state.target).distance(bounds.center)).toBeLessThan(1e-5);
      expect(state.orthographicView).toBeNull();
      expect(state.projection).toBe(PROJECTION_PERSPECTIVE);
      expect(state.distance).toBeCloseTo(12);
      expect(state.projectionOffset).toEqual(reference.projectionOffset);
      expect(state.projectionScaleX).toBe(1.3);
      expect(entity.getPosition().distance(reference.position)).toBeLessThan(1e-5);
      expect(entity.forward.dot(reference.forward)).toBeCloseTo(1);
      expect(entity.up.dot(reference.up)).toBeCloseTo(1);
      const projection = new Mat4();
      (camera as unknown as { calculateProjection: (m: Mat4) => void }).calculateProjection(projection);
      const verticalPixelsPerUnit = projection.data[5] * 800 / (2 * 12);
      const horizontalPixelsPerUnit = projection.data[0] * 1200 / (2 * 12);
      const referencePixelsPerUnit = 800 / (2 * 12 * Math.tan(64 * Math.PI / 360));
      expect(verticalPixelsPerUnit).toBeCloseTo(referencePixelsPerUnit, 5);
      expect(horizontalPixelsPerUnit).toBeCloseTo(referencePixelsPerUnit * 1.3, 5);
      controls.restore(state);
      expect(controls.snapshot()).toEqual(state);
    } finally { controls.destroy(); }
  });

  it("uses robust extent only when the reference depth is invalid", () => {
    const { controls, camera, entity } = setup();
    try {
      const bounds = new BoundingBox(Vec3.ZERO, new Vec3(1, 2, 3));
      controls.alignOrthographic("front", bounds, 0.85, false);
      controls.locate({ position: new Vec3(0, 0, -5), forward: new Vec3(0, 0, -1), up: Vec3.UP,
        fov: 60, projectionOffset: [0, 0], projectionScaleX: 1 }, bounds, false);
      expect(controls.snapshot().distance).toBeCloseTo(Math.sqrt(14));
      expect(camera.projection).toBe(PROJECTION_PERSPECTIVE);
      expect(entity.getPosition().distance(new Vec3(0, 0, -5))).toBeLessThan(1e-5);
    } finally { controls.destroy(); }
  });
  it("does not move the camera for a click or sub-threshold pointer motion", () => {
    const { canvas, controls, setPosition } = setup();
    const initialUpdates = setPosition.mock.calls.length;

    canvas.dispatchEvent(pointerEvent("pointerdown", 40, 40));
    canvas.dispatchEvent(pointerEvent("pointermove", 41, 41));
    canvas.dispatchEvent(pointerEvent("pointerup", 41, 41));

    expect(setPosition).toHaveBeenCalledTimes(initialUpdates);
    controls.destroy();
  });

  it("cancels an active camera gesture when another control takes ownership", () => {
    const { canvas, controls, setPosition } = setup();
    const initialUpdates = setPosition.mock.calls.length;

    canvas.dispatchEvent(pointerEvent("pointerdown", 20, 20));
    controls.cancelGesture();
    canvas.dispatchEvent(pointerEvent("pointermove", 80, 80));

    expect(setPosition).toHaveBeenCalledTimes(initialUpdates);
    controls.destroy();
  });

  it("limits a single wheel event and can be destroyed repeatedly", () => {
    const { canvas, controls } = setup();
    const before = controls.cameraDistance;

    canvas.dispatchEvent(new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaY: -1_000_000 }));

    expect(controls.cameraDistance).toBeLessThan(before);
    expect(controls.cameraDistance).toBeGreaterThanOrEqual(before * Math.exp(-0.24));
    expect(() => { controls.destroy(); controls.destroy(); }).not.toThrow();
  });

  it("starts above the model and resets to an upright viewing direction", () => {
    const { controls, setPosition } = setup();
    const initialPosition = setPosition.mock.calls.at(-1)?.[0] as Vec3;

    expect(initialPosition.y).toBeGreaterThan(0);
    controls.fit(new BoundingBox(new Vec3(), new Vec3(1, 1, 1)), { resetDirection: true });

    const resetPosition = setPosition.mock.calls.at(-1)?.[0] as Vec3;
    expect(controls.cameraPitch).toBe(22);
    expect(resetPosition.y).toBeGreaterThan(0);
    controls.destroy();
  });

  it("frames tightly and shifts the model away from the transform panel", () => {
    const { controls, lookAt } = setup();
    const bounds = new BoundingBox(new Vec3(), new Vec3(1, 1, 1));
    const oldBoundingSphereDistance = bounds.halfExtents.length() / Math.sin(26 * Math.PI / 180) * 1.18;

    controls.fit(bounds, { resetDirection: true, occupancy: 0.85, rightInsetPx: 286 });

    const viewTarget = lookAt.mock.calls.at(-1)?.[0] as Vec3;
    expect(controls.cameraDistance).toBeLessThan(oldBoundingSphereDistance);
    expect(viewTarget.x).toBeGreaterThan(bounds.center.x);
    controls.destroy();
  });

  it("extends the perspective far plane to include the ground grid", () => {
    const { camera, controls } = setup();
    controls.setSceneBounds(new BoundingBox(new Vec3(0, 1, 0), new Vec3(50, 1, 50)));
    controls.fit(new BoundingBox(new Vec3(), new Vec3(1, 1, 1)), { occupancy: 0.85 });

    expect(camera.nearClip).toBeLessThanOrEqual(0.001);
    expect(camera.farClip).toBeGreaterThan(50);
    controls.destroy();
  });

  it("keeps the original grid inside the orthographic depth range without changing screen framing", () => {
    const { camera, controls, entity } = setup();
    const model = new BoundingBox(new Vec3(), new Vec3(1.5, 1, 1));
    const grid = buildGroundGrid(model);
    controls.alignOrthographic("front", model, 0.85, false);
    controls.restore({ ...controls.snapshot(), orthographicView: null, yaw: 45, pitch: 22 });
    const saved = controls.snapshot();
    const points = [...grid.positions, new Vec3(0.5, 1, -0.5)];
    const project = (point: Vec3) => {
      const view = new Mat4().invert(entity.getWorldTransform());
      const local = view.transformPoint(point);
      return [local.x / (camera.orthoHeight * camera.aspectRatio), local.y / camera.orthoHeight];
    };
    const before = points.map(project);
    expect(Math.min(...grid.positions.map(point => point.clone().sub(entity.getPosition()).dot(entity.forward)))).toBeLessThan(0);

    const scene = model.clone();
    scene.add(grid.bounds);
    controls.setSceneBounds(scene);

    for (const [index, point] of points.entries()) {
      const depth = point.clone().sub(entity.getPosition()).dot(entity.forward);
      expect(depth).toBeGreaterThan(camera.nearClip);
      expect(depth).toBeLessThan(camera.farClip);
      const after = project(point);
      expect(after[0]).toBeCloseTo(before[index][0], 5);
      expect(after[1]).toBeCloseTo(before[index][1], 5);
    }
    expect(controls.snapshot()).toEqual(saved);
    controls.restore(saved);
    for (const point of grid.positions) {
      expect(point.clone().sub(entity.getPosition()).dot(entity.forward)).toBeGreaterThan(camera.nearClip);
    }
    controls.destroy();
  });

  it("keeps the full grid and translated model in range while orbiting, panning, zooming and realigning", () => {
    const { camera, controls, entity, canvas } = setup();
    const model = new BoundingBox(new Vec3(2, -1, 3), new Vec3(1.5, 1, 1));
    const grid = buildGroundGrid(model);
    const scene = model.clone();
    scene.add(grid.bounds);
    controls.setSceneBounds(scene);
    const points = [...grid.positions];
    for (const x of [-1, 1]) for (const y of [-1, 1]) for (const z of [-1, 1]) {
      points.push(new Vec3(model.center.x + x * model.halfExtents.x, model.center.y + y * model.halfExtents.y, model.center.z + z * model.halfExtents.z));
    }
    const checkDepths = () => {
      for (const point of points) {
        const depth = point.clone().sub(entity.getPosition()).dot(entity.forward);
        expect(depth).toBeGreaterThan(camera.nearClip);
        expect(depth).toBeLessThan(camera.farClip);
      }
    };
    for (const view of ["front", "side", "top"] as const) {
      controls.alignOrthographic(view, model, 0.85, false);
      checkDepths();
      canvas.dispatchEvent(pointerEvent("pointerdown", 40, 40));
      canvas.dispatchEvent(pointerEvent("pointermove", 60, 30));
      canvas.dispatchEvent(pointerEvent("pointerup", 60, 30));
      checkDepths();
      for (const pitch of [-70, -5, 0.1, 22, 70, 89.9]) {
        for (const yaw of [0, 45, 90, 180, 270]) {
          controls.restore({ ...controls.snapshot(), pitch, yaw });
          checkDepths();
        }
      }
      canvas.dispatchEvent(pointerEvent("pointerdown", 40, 40, 2));
      canvas.dispatchEvent(pointerEvent("pointermove", 100, 80, 2));
      canvas.dispatchEvent(pointerEvent("pointerup", 100, 80, 2));
      checkDepths();
      canvas.dispatchEvent(new WheelEvent("wheel", { cancelable: true, deltaY: 240 }));
      checkDepths();
    }
    const transformed = new BoundingBox(new Vec3(-10, 5, -8), new Vec3(4, 3, 2));
    const updated = transformed.clone();
    updated.add(grid.bounds);
    controls.setSceneBounds(updated);
    checkDepths();
    controls.destroy();
  });

  it("snapshots, orbits, and restores the exact interactive camera state", () => {
    const { controls } = setup();
    const initial = controls.snapshot();

    controls.orbitBy(15);
    expect(controls.snapshot().yaw).toBe(initial.yaw + 15);

    controls.restore(initial);
    expect(controls.snapshot()).toEqual(initial);
    controls.destroy();
  });

  it("reports the current Y-axis orbit center, radius, height, and direction", () => {
    const { controls } = setup();
    controls.fit(new BoundingBox(new Vec3(3, 4, 5), new Vec3(2, 1, 2)), { occupancy: 0.85 });
    const initial = controls.orbitGuideState();

    expect(initial.target).toEqual([3, 4, 5]);
    expect(initial.radius).toBeGreaterThan(0);
    expect(initial.height).toBeGreaterThan(initial.target[1]);

    controls.orbitBy(15);
    const rotated = controls.orbitGuideState();
    expect(rotated.radius).toBeCloseTo(initial.radius);
    expect(rotated.height).toBeCloseTo(initial.height);
    expect(rotated.angleDegrees).toBeCloseTo(initial.angleDegrees + 15);
    controls.destroy();
  });

  it("restores the orthographic view indicator with the camera", () => {
    const { controls, onOrthographicViewChange } = setup();
    controls.alignOrthographic("top", new BoundingBox(new Vec3(1, 2, 3), new Vec3(2, 3, 4)), 0.85, false);
    const saved = controls.snapshot();
    controls.usePerspective();
    controls.restore(saved);
    expect(controls.snapshot()).toEqual(saved);
    expect(onOrthographicViewChange).toHaveBeenLastCalledWith("top", true);
    controls.destroy();
  });

  it("animates between orthographic views and pauses the transition while the model gizmo owns input", () => {
    const frames: FrameRequestCallback[] = [];
    const request = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      frames.push(callback);
      return frames.length;
    });
    const cancel = vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => undefined);
    const clock = vi.spyOn(performance, "now").mockReturnValue(1000);
    const { camera, controls, setPosition } = setup();
    const before = setPosition.mock.calls.length;

    controls.alignOrthographic("side", new BoundingBox(new Vec3(2, 3, 4), new Vec3(5, 6, 7)));
    expect(camera.projection).toBe(PROJECTION_ORTHOGRAPHIC);
    expect(frames).toHaveLength(1);
    frames.shift()?.(1160);
    expect(setPosition.mock.calls.length).toBeGreaterThan(before);
    expect(frames).toHaveLength(1);
    controls.enabled = false;
    const pausedUpdates = setPosition.mock.calls.length;
    frames.shift()?.(2160);
    controls.setSceneBounds(new BoundingBox(new Vec3(2, 3, 4), new Vec3(6, 7, 8)));
    frames.shift()?.(3160);
    expect(setPosition).toHaveBeenCalledTimes(pausedUpdates);
    expect(frames).toHaveLength(1);
    controls.enabled = true;
    frames.shift()?.(3320);
    expect(setPosition.mock.calls.length).toBeGreaterThan(pausedUpdates);
    expect(frames).toHaveLength(0);

    controls.destroy();
    request.mockRestore();
    cancel.mockRestore();
    clock.mockRestore();
  });

  it("keeps grid vertices in front of the near plane on every orthographic transition frame", () => {
    const frames: FrameRequestCallback[] = [];
    const request = vi.spyOn(window, "requestAnimationFrame").mockImplementation(callback => {
      frames.push(callback);
      return frames.length;
    });
    const cancel = vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => undefined);
    const clock = vi.spyOn(performance, "now").mockReturnValue(0);
    const { camera, controls, entity } = setup();
    const model = new BoundingBox(new Vec3(), new Vec3(1.5, 1, 1));
    const grid = buildGroundGrid(model);
    const scene = model.clone();
    scene.add(grid.bounds);
    controls.setSceneBounds(scene);
    try {
      for (const view of ["front", "side", "top"] as const) {
        controls.alignOrthographic(view, model);
        for (let time = 0; time <= 320; time += 40) {
          frames.shift()?.(time);
          for (const point of grid.positions) {
            const depth = point.clone().sub(entity.getPosition()).dot(entity.forward);
            expect(depth).toBeGreaterThan(camera.nearClip);
            expect(depth).toBeLessThan(camera.farClip);
          }
        }
        expect(frames).toHaveLength(0);
      }
    } finally {
      controls.destroy();
      request.mockRestore();
      cancel.mockRestore();
      clock.mockRestore();
    }
  });

  it.each(["front", "side"] as const)("keeps the %s orthographic framing continuous when orbiting", (view) => {
    const { canvas, camera, controls, onOrthographicViewChange } = setup();
    controls.alignOrthographic(view, new BoundingBox(new Vec3(), new Vec3(2, 1, 3)), 0.85, false);
    const aligned = controls.snapshot();
    expect(camera.projection).toBe(PROJECTION_ORTHOGRAPHIC);
    expect(onOrthographicViewChange).toHaveBeenLastCalledWith(view, true);

    canvas.dispatchEvent(pointerEvent("pointerdown", 40, 40));
    canvas.dispatchEvent(pointerEvent("pointermove", 60, 48));
    canvas.dispatchEvent(pointerEvent("pointerup", 60, 48));

    const orbited = controls.snapshot();
    expect(camera.projection).toBe(PROJECTION_ORTHOGRAPHIC);
    expect(orbited.projection).toBe(PROJECTION_ORTHOGRAPHIC);
    expect(orbited.orthoHeight).toBe(aligned.orthoHeight);
    expect(orbited.distance).toBe(aligned.distance);
    expect(orbited.orthographicView).toBeNull();
    expect(onOrthographicViewChange).toHaveBeenLastCalledWith(null, true);
    controls.destroy();
  });

  it("leaves the top view through a stable pole without flipping the screen direction", () => {
    const { canvas, camera, controls, lookAt, onOrthographicViewChange } = setup();
    controls.alignOrthographic("top", new BoundingBox(new Vec3(), new Vec3(2, 1, 3)), 0.85, false);
    const aligned = controls.snapshot();

    canvas.dispatchEvent(pointerEvent("pointerdown", 40, 40));
    canvas.dispatchEvent(pointerEvent("pointermove", 60, 40));
    canvas.dispatchEvent(pointerEvent("pointerup", 60, 40));

    const orbited = controls.snapshot();
    const up = lookAt.mock.calls.at(-1)?.[1] as Vec3;
    expect(camera.projection).toBe(PROJECTION_ORTHOGRAPHIC);
    expect(orbited.orthographicView).toBeNull();
    expect(orbited.orthoHeight).toBe(aligned.orthoHeight);
    expect(orbited.distance).toBe(aligned.distance);
    expect(orbited.pitch).toBeCloseTo(89.9);
    expect(up.z).toBeLessThan(-0.99);
    expect(onOrthographicViewChange).toHaveBeenLastCalledWith(null, true);
    controls.destroy();
  });

});

describe("manual orbit horizon leveling", () => {
  const setupFrames = () => {
    const frames = new Map<number, FrameRequestCallback>(); let id = 0;
    const request = vi.spyOn(window, "requestAnimationFrame").mockImplementation(cb => { frames.set(++id, cb); return id; });
    const cancel = vi.spyOn(window, "cancelAnimationFrame").mockImplementation(id => { frames.delete(id); });
    const clock = vi.spyOn(performance, "now").mockReturnValue(0);
    const f = setup();
    const advance = (time: number) => { clock.mockReturnValue(time); const pending = [...frames.values()]; frames.clear(); pending.forEach(cb => cb(time)); };
    const done = () => { f.controls.destroy(); request.mockRestore(); cancel.mockRestore(); clock.mockRestore(); };
    return { ...f, frames, advance, done };
  };
  it.each([18, -18, 179, -179, 725])("smoothly levels roll %s after real orbit, keeps framing and persists the final pose", roll => {
    const f = setupFrames();
    try {
      f.controls.restore({ ...f.controls.snapshot(), roll, fov: 70, projectionOffset: [.1, -.1], projectionScaleX: 1.2 });
      const before = f.controls.snapshot();
      f.canvas.dispatchEvent(pointerEvent("pointerdown", 0, 0)); f.canvas.dispatchEvent(pointerEvent("pointermove", 10, 4));
      const normalized = ((roll + 180) % 360 + 360) % 360 - 180;
      expect(f.controls.snapshot().roll).toBe(normalized);
      f.advance(90); expect(f.controls.snapshot().roll).toBeCloseTo(normalized / 8);
      f.canvas.dispatchEvent(pointerEvent("pointermove", 20, 4));
      f.canvas.dispatchEvent(pointerEvent("pointerup", 20, 4));
      f.advance(180);
      const after = f.controls.snapshot();
      expect(after.roll).toBe(0); expect(after.yaw).toBeCloseTo(before.yaw - 6);
      for (const key of ["target", "distance", "projection", "fov", "orthoHeight", "projectionOffset", "projectionScaleX"] as const) expect(after[key]).toEqual(before[key]);
      expect(f.entity.right.y).toBeCloseTo(0, 5);
      expect(f.onViewChange).toHaveBeenCalled(); expect(f.frames.size).toBe(0);
    } finally { f.done(); }
  });
  it("does not level for clicks, sub-threshold drags, right pan or wheel; rectangle mode uses middle orbit", () => {
    const f = setupFrames();
    try {
      f.controls.restore({ ...f.controls.snapshot(), roll: 20 });
      for (const button of [0, 2]) {
        f.canvas.dispatchEvent(pointerEvent("pointerdown", 0, 0, button));
        f.canvas.dispatchEvent(pointerEvent("pointermove", button ? 20 : 1, 1, button));
        f.canvas.dispatchEvent(pointerEvent("pointerup", button ? 20 : 1, 1, button));
      }
      f.canvas.dispatchEvent(new WheelEvent("wheel", { deltaY: 50, cancelable: true }));
      expect(f.frames.size).toBe(0); expect(f.controls.snapshot().roll).toBe(20);
      f.controls.setRectangleSelectionMode(true);
      f.canvas.dispatchEvent(pointerEvent("pointerdown", 0, 0)); f.canvas.dispatchEvent(pointerEvent("pointermove", 20, 0));
      expect(f.frames.size).toBe(0);
      f.canvas.dispatchEvent(pointerEvent("pointerdown", 0, 0, 1)); f.canvas.dispatchEvent(pointerEvent("pointermove", 20, 0, 1));
      f.advance(180); expect(f.controls.snapshot().roll).toBe(0);
    } finally { f.done(); }
  });
  it("cancels leveling when a fixed view takes over, keeps free orthographic scale and handles near-pole views", () => {
    const f = setupFrames();
    try {
      f.controls.restore({ ...f.controls.snapshot(), roll: -30, pitch: 89.9, projection: PROJECTION_ORTHOGRAPHIC, orthoHeight: 8 });
      f.canvas.dispatchEvent(pointerEvent("pointerdown", 0, 0)); f.canvas.dispatchEvent(pointerEvent("pointermove", 10, 0));
      f.advance(90); expect(f.controls.snapshot().projection).toBe(PROJECTION_ORTHOGRAPHIC);
      expect(f.camera.orthoHeight).toBe(8);
      f.controls.alignOrthographic("front", new BoundingBox(new Vec3(), new Vec3(1, 1, 1)), .85, false);
      expect(f.frames.size).toBe(0); expect(f.controls.snapshot().roll).toBe(0);
      expect(f.entity.forward.z).toBeCloseTo(-1);
    } finally { f.done(); }
  });
  it("starts from the actual pose when dragging interrupts Locate and restores old rolled views unchanged", () => {
    const f = setupFrames();
    try {
      const saved = { ...f.controls.snapshot(), roll: 35 };
      f.controls.restore(saved);
      expect(f.controls.snapshot()).toEqual(saved);
      f.controls.locate({ position: new Vec3(3, 4, 5), forward: new Vec3(0, 0, -1), up: new Vec3(.5, .8660254, 0),
        fov: 70, projectionOffset: [0, 0], projectionScaleX: 1 }, new BoundingBox(new Vec3(), new Vec3(1, 1, 1)));
      f.advance(80);
      const position = f.entity.getPosition().clone(), rotation = f.entity.getRotation().clone();
      f.canvas.dispatchEvent(pointerEvent("pointerdown", 0, 0));
      expect(f.entity.getPosition().distance(position)).toBeLessThan(1e-5);
      expect(Math.abs(f.entity.getRotation().dot(rotation))).toBeCloseTo(1);
      f.canvas.dispatchEvent(pointerEvent("pointermove", 10, 0)); f.advance(260);
      expect(f.controls.snapshot().roll).toBe(0); expect(f.frames.size).toBe(0);
      f.controls.restore(saved); expect(f.controls.snapshot()).toEqual(saved);
    } finally { f.done(); }
  });
});

describe("focused perspective WASD flight", () => {
  function flight(height = 800) {
    const frames = new Map<number, FrameRequestCallback>();
    let id = 0;
    const request = vi.spyOn(window, "requestAnimationFrame").mockImplementation(callback => { frames.set(++id, callback); return id; });
    const cancel = vi.spyOn(window, "cancelAnimationFrame").mockImplementation(id => { frames.delete(id); });
    const clock = vi.spyOn(performance, "now").mockReturnValue(0);
    const state = setup(1200, height);
    document.body.append(state.canvas);
    state.canvas.focus();
    state.controls.setRoamingEnabled(true);
    const key = (type: string, code: string, options: KeyboardEventInit = {}) => state.canvas.dispatchEvent(new KeyboardEvent(type, { code, key: code.replace("Key", "").toLowerCase(), bubbles: true, cancelable: true, ...options }));
    const advance = (now: number) => { clock.mockReturnValue(now); const pending = [...frames.values()]; frames.clear(); pending.forEach(f => f(now)); };
    const done = () => { state.controls.destroy(); state.canvas.remove(); request.mockRestore(); cancel.mockRestore(); clock.mockRestore(); };
    return { ...state, key, advance, frames, done };
  }

  it("moves along the viewing direction at 15% speed, preserves orientation/distance and saves only when released", () => {
    const s = flight();
    try {
      const before = s.controls.snapshot();
      const forward = s.entity.forward.clone().normalize();
      const position = s.entity.getPosition().clone();
      s.key("keydown", "KeyW");
      s.advance(16); s.advance(32);
      const moved = new Vec3(...s.controls.snapshot().target).sub(new Vec3(...before.target));
      expect(moved.clone().normalize().dot(forward)).toBeCloseTo(1);
      expect(moved.y).toBeLessThan(0);
      expect(s.entity.getPosition().clone().sub(position).distance(moved)).toBeLessThan(1e-6);
      expect(s.entity.forward.dot(forward)).toBeCloseTo(1);
      expect(moved.length()).toBeCloseTo(0.15 * 2 * before.distance * Math.tan(52 * Math.PI / 360) * 0.032);
      expect(s.controls.snapshot().distance).toBe(before.distance);
      expect(s.onViewChange).not.toHaveBeenCalled();
      s.key("keyup", "KeyW");
      expect(s.onViewChange).toHaveBeenCalledOnce();
      expect(s.frames.size).toBe(0);
    } finally { s.done(); }
  });

  it("normalizes diagonal movement, supports Shift, and bounds time after a stalled frame", () => {
    const s = flight();
    try {
      const before = s.controls.snapshot();
      const yaw = before.yaw * Math.PI / 180;
      const direction = s.entity.forward.clone().add(new Vec3(Math.cos(yaw), 0, -Math.sin(yaw))).normalize();
      s.key("keydown", "KeyW", { shiftKey: true });
      s.key("keydown", "KeyD", { shiftKey: true });
      s.advance(5000);
      const moved = new Vec3(...s.controls.snapshot().target).sub(new Vec3(...before.target));
      expect(moved.length()).toBeCloseTo(0.15 * 2 * before.distance * Math.tan(52 * Math.PI / 360) * 0.05 * 3);
      expect(moved.clone().normalize().dot(direction)).toBeCloseTo(1, 7);
      expect(moved.y).toBeLessThan(0);
    } finally { s.done(); }
  });

  it.each([0.1, 0.5, 1, 2, 3, 4, 5])("applies multiplier %s without changing zoom, orientation or frame scheduling", multiplier => {
    const s = flight();
    try {
      const start = s.controls.snapshot();
      s.controls.setRoamSpeed(multiplier);
      expect(s.frames.size).toBe(0);
      s.key("keydown", "KeyW"); s.advance(16); s.key("keyup", "KeyW");
      const moved = new Vec3(...s.controls.snapshot().target).sub(new Vec3(...start.target)).length();
      expect(moved).toBeCloseTo(.15 * multiplier * 2 * start.distance * Math.tan(start.fov! * Math.PI / 360) * .016, 7);
      expect(s.controls.snapshot().distance).toBe(start.distance);
      expect(s.controls.snapshot().yaw).toBe(start.yaw);
      expect(s.frames.size).toBe(0);
    } finally { s.done(); }
  });

  it("reads speed changes during motion and retains Shift's three-times factor", () => {
    const s = flight();
    try {
      s.key("keydown", "KeyW", { shiftKey: true });
      const start = new Vec3(...s.controls.snapshot().target);
      s.advance(16); const first = new Vec3(...s.controls.snapshot().target);
      s.controls.setRoamSpeed(2); s.advance(32);
      const second = new Vec3(...s.controls.snapshot().target);
      expect(second.clone().sub(first).length() / first.clone().sub(start).length()).toBeCloseTo(2, 7);
      s.key("keyup", "KeyW");
      s.controls.setRoamSpeed(NaN); s.controls.restore({ ...s.controls.snapshot(), target: [0, 0, 0] });
      s.key("keydown", "KeyW"); s.advance(48);
      expect(new Vec3(...s.controls.snapshot().target).length()).toBeCloseTo(first.clone().sub(start).length() / 3, 7);
    } finally { s.done(); }
  });

  it.each([
    [0, 0, 0], [47, 70, 90], [-135, -70, -90],
    [90, 89.999, 180], [47, 90, 90], [-135, -90, -90],
  ])("flies W/S along sight and keeps A/D horizontal at yaw %s, pitch %s, roll %s", (yaw, pitch, roll) => {
    const s = flight();
    try {
      const initial = { ...s.controls.snapshot(), yaw, pitch, roll };
      const angle = yaw * Math.PI / 180;
      const elevation = pitch * Math.PI / 180;
      const front = new Vec3(-Math.sin(angle) * Math.cos(elevation), -Math.sin(elevation), -Math.cos(angle) * Math.cos(elevation));
      const right = new Vec3(Math.cos(angle), 0, -Math.sin(angle));
      let time = 0;
      for (const [code, direction] of [
        ['KeyW', front], ['KeyS', front.clone().mulScalar(-1)],
        ['KeyD', right], ['KeyA', right.clone().mulScalar(-1)],
      ] as const) {
        s.controls.restore(initial);
        const before = s.entity.getPosition().clone();
        const orientation = s.entity.getRotation().clone();
        s.key('keydown', code); s.advance(time += 16); s.key('keyup', code);
        const moved = new Vec3(...s.controls.snapshot().target).sub(new Vec3(...initial.target));
        if (code === 'KeyA' || code === 'KeyD') expect(moved.y).toBe(0);
        else expect(moved.y).toBeCloseTo(direction.y * moved.length(), 7);
        expect(moved.clone().normalize().dot(direction)).toBeCloseTo(1, 7);
        expect(moved.length()).toBeCloseTo(0.15 * 2 * initial.distance * Math.tan(initial.fov! * Math.PI / 360) * 0.016, 7);
        expect(s.entity.getPosition().clone().sub(before).distance(moved)).toBeLessThan(1e-6);
        expect(s.entity.getRotation().equalsApprox(orientation)).toBe(true);
      }
    } finally { s.done(); }
  });

  it('cancels opposing keys without moving or committing a view change', () => {
    const s = flight();
    try {
      const before = s.controls.snapshot();
      for (const code of ['KeyW', 'KeyS', 'KeyA', 'KeyD']) s.key('keydown', code);
      s.advance(16);
      for (const code of ['KeyW', 'KeyS', 'KeyA', 'KeyD']) s.key('keyup', code);
      expect(s.controls.snapshot()).toEqual(before);
      expect(s.onViewChange).not.toHaveBeenCalled();
      expect(s.frames.size).toBe(0);
    } finally { s.done(); }
  });

  it.each([800, 1600])("automatically slows with zoom and respects FOV at viewport height %s", height => {
    const s = flight(height);
    try {
      const move = (now: number) => {
        const before = new Vec3(...s.controls.snapshot().target);
        s.key('keydown', 'KeyW'); s.advance(now); s.key('keyup', 'KeyW');
        return new Vec3(...s.controls.snapshot().target).sub(before).length();
      };
      const original = move(16);
      s.canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: -200, cancelable: true }));
      const zoomed = move(32);
      expect(zoomed / original).toBeCloseTo(Math.exp(-0.2));
      s.controls.restore({ ...s.controls.snapshot(), fov: 90 });
      const wider = move(48);
      expect(wider / zoomed).toBeCloseTo(Math.tan(Math.PI / 4) / Math.tan(52 * Math.PI / 360));
    } finally { s.done(); }
  });

  it.each([30, 60, 144])("uses the same flight speed at %s frames per second", fps => {
    const s = flight();
    try {
      const start = s.controls.snapshot();
      s.key('keydown', 'KeyW');
      for (let frame = 1; frame <= fps; frame++) s.advance(1000 * frame / fps);
      const distance = new Vec3(...s.controls.snapshot().target).sub(new Vec3(...start.target)).length();
      expect(distance).toBeCloseTo(0.15 * 2 * start.distance * Math.tan(52 * Math.PI / 360), 5);
    } finally { s.done(); }
  });

  it("does not double-count queued frame timestamps older than keydown", () => {
    const s = flight();
    try {
      s.advance(100);
      const before = s.controls.snapshot();
      s.key("keydown", "KeyW");
      s.advance(80); s.advance(90); s.advance(116);
      const distance = new Vec3(...s.controls.snapshot().target).sub(new Vec3(...before.target)).length();
      expect(distance).toBeCloseTo(0.15 * 2 * before.distance * Math.tan(52 * Math.PI / 360) * 0.016, 7);
    } finally { s.done(); }
  });

  it.each(["blur", "disabled", "mode", "visibility", "pointercancel", "input"])("stops and commits flight on %s", reason => {
    const s = flight();
    const input = document.createElement("input");
    try {
      s.key("keydown", "KeyW"); s.advance(16);
      if (reason === "blur") window.dispatchEvent(new Event("blur"));
      if (reason === "disabled") s.controls.enabled = false;
      if (reason === "mode") s.controls.setRoamingEnabled(false);
      if (reason === "visibility") document.dispatchEvent(new Event("visibilitychange"));
      if (reason === "pointercancel") s.canvas.dispatchEvent(pointerEvent("pointercancel", 0, 0));
      if (reason === "input") { document.body.append(input); input.focus(); }
      const stopped = s.controls.snapshot();
      s.advance(32);
      expect(s.controls.snapshot()).toEqual(stopped);
      expect(s.frames.size).toBe(0);
      expect(s.onViewChange).toHaveBeenCalledOnce();
    } finally { input.remove(); s.done(); }
  });

  it("ignores input targets, modifier shortcuts and every orthographic view", () => {
    const s = flight();
    try {
      const before = s.controls.snapshot();
      s.key("keydown", "KeyA", { ctrlKey: true });
      expect(s.frames.size).toBe(0);
      const input = document.createElement("input"); document.body.append(input); input.focus();
      input.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyW", bubbles: true }));
      expect(s.frames.size).toBe(0);
      input.remove(); s.canvas.focus();
      for (const view of ["front", "side", "top"] as const) {
        s.controls.alignOrthographic(view, new BoundingBox(new Vec3(), new Vec3(1, 1, 1)), 0.85, false);
        const aligned = s.controls.snapshot();
        s.key("keydown", "KeyW"); s.advance(16);
        expect(s.frames.size).toBe(0);
        expect(s.controls.snapshot()).toEqual(aligned);
      }
      s.controls.restore(before);
      s.controls.setRoamingEnabled(false);
      s.key("keydown", "KeyW");
      expect(s.frames.size).toBe(0);
    } finally { s.done(); }
  });

  it("can interrupt locating with pointer or wheel and releases all listeners on destroy", () => {
    const s = flight();
    try {
      const reference = { position: new Vec3(0, 0, 5), forward: new Vec3(0, 0, -1), up: Vec3.UP,
        fov: 60, projectionOffset: [0, 0] as [number, number], projectionScaleX: 1 };
      const bounds = new BoundingBox(new Vec3(), new Vec3(1, 1, 1));
      s.controls.locate(reference, bounds);
      s.advance(100);
      s.canvas.dispatchEvent(new WheelEvent("wheel", { deltaY: 20, cancelable: true }));
      expect(s.frames.size).toBe(0);
      s.controls.locate(reference, bounds);
      s.canvas.dispatchEvent(pointerEvent("pointerdown", 0, 0));
      expect(s.frames.size).toBe(0);
      s.controls.destroy();
      s.key("keydown", "KeyW");
      expect(s.frames.size).toBe(0);
    } finally { s.done(); }
  });

  it("flies at the same scale after zoom, orbit, saved restore and reference locating", () => {
    const s = flight();
    try {
      s.canvas.dispatchEvent(new WheelEvent("wheel", { deltaY: -200, cancelable: true }));
      const zoom = s.controls.snapshot();
      const move = (time: number) => {
        const start = new Vec3(...s.controls.snapshot().target);
        s.key("keydown", "KeyW"); s.advance(time); s.key("keyup", "KeyW");
        return new Vec3(...s.controls.snapshot().target).sub(start).length();
      };
      const original = move(16);
      s.controls.orbitBy(48);
      expect(move(32)).toBeCloseTo(original, 7);
      s.controls.restore(zoom);
      expect(move(48)).toBeCloseTo(original, 7);
      s.controls.locate({ position: new Vec3(0, 0, zoom.distance), forward: new Vec3(0, 0, -1), up: Vec3.UP,
        fov: zoom.fov!, projectionOffset: [0, 0], projectionScaleX: 1 }, new BoundingBox(Vec3.ZERO, Vec3.ONE), false);
      expect(move(64)).toBeCloseTo(original, 7);
    } finally { s.done(); }
  });

  it.each([true, false])("returns button focus immediately and accepts WASD after locate (animate=%s)", animate => {
    const s = flight();
    const button = document.createElement("button");
    document.body.append(button);
    try {
      button.onclick = () => s.controls.locate({ position: new Vec3(3, 4, 8), forward: new Vec3(0, 0, -1), up: Vec3.UP,
        fov: 70, projectionOffset: [0, 0], projectionScaleX: 1 }, new BoundingBox(Vec3.ZERO, Vec3.ONE), animate);
      button.focus(); button.click();
      expect(document.activeElement).toBe(s.canvas);
      if (animate) s.advance(100);
      const before = s.entity.getPosition().clone();
      const zoom = s.controls.snapshot();
      const forward = s.entity.forward.clone().normalize();
      s.key("keydown", "KeyW");
      s.advance(animate ? 116 : 16);
      const movement = s.entity.getPosition().clone().sub(before);
      expect(movement.clone().normalize().dot(forward)).toBeCloseTo(1, 5);
      expect(movement.y).toBeCloseTo(forward.y * movement.length(), 6);
      expect(movement.length()).toBeCloseTo(0.15 * 2 * zoom.distance * Math.tan(zoom.fov! * Math.PI / 360) * 0.016, 5);
      s.key("keyup", "KeyW");
      expect(s.frames.size).toBe(0);
    } finally { button.remove(); s.done(); }
  });

  it.each(["front", "side", "top"] as const)("finishes locating from %s in perspective and immediately enables flight", view => {
    const s = flight();
    try {
      const bounds = new BoundingBox(Vec3.ZERO, Vec3.ONE);
      const reference = { position: new Vec3(3, 4, 8), forward: new Vec3(0, 0, -1), up: Vec3.UP,
        fov: 70, projectionOffset: [0.02, -0.01] as [number, number], projectionScaleX: 1.1 };
      s.controls.alignOrthographic(view, bounds, 0.85, false);
      s.controls.locate(reference, bounds);
      s.advance(400);
      expect(s.entity.getPosition().distance(reference.position)).toBeLessThan(1e-5);
      expect(s.entity.forward.dot(reference.forward)).toBeCloseTo(1);
      const located = s.controls.snapshot();
      expect(located.projection).toBe(PROJECTION_PERSPECTIVE);
      expect(located.orthographicView).toBeNull();
      expect(located.fov).toBe(70);
      expect(document.activeElement).toBe(s.canvas);
      s.key("keydown", "KeyW"); s.advance(416); s.key("keyup", "KeyW");
      expect(s.entity.getPosition().distance(reference.position)).toBeCloseTo(0.15 * 2 * located.distance * Math.tan(located.fov! * Math.PI / 360) * 0.016, 5);
      s.controls.alignOrthographic(view, bounds, 0.85, false);
      s.controls.restore(located);
      expect(s.entity.getPosition().distance(reference.position)).toBeLessThan(1e-5);
      expect(s.controls.snapshot()).toEqual(located);
    } finally { s.done(); }
  });

  it("does not steal later input focus when locating finishes, or focus on a blocked locate", () => {
    const s = flight();
    const input = document.createElement("input"); document.body.append(input);
    try {
      const reference = { position: new Vec3(0, 0, 8), forward: new Vec3(0, 0, -1), up: Vec3.UP,
        fov: 70, projectionOffset: [0, 0] as [number, number], projectionScaleX: 1 };
      const bounds = new BoundingBox(Vec3.ZERO, Vec3.ONE);
      s.controls.locate(reference, bounds);
      input.focus(); s.advance(400);
      expect(document.activeElement).toBe(input);
      input.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyW", bubbles: true }));
      expect(s.frames.size).toBe(0);
      s.controls.enabled = false;
      const before = s.controls.snapshot();
      s.controls.locate(reference, bounds);
      expect(document.activeElement).toBe(input);
      expect(s.controls.snapshot()).toEqual(before);
    } finally { input.remove(); s.done(); }
  });

  it("keeps the rendered pose when a gizmo or modal interrupts locating", () => {
    const s = flight();
    try {
      s.controls.locate({ position: new Vec3(3, 4, 8), forward: new Vec3(0, 0, -1), up: Vec3.UP,
        fov: 70, projectionOffset: [0, 0], projectionScaleX: 1 }, new BoundingBox(new Vec3(), new Vec3(1, 1, 1)));
      s.advance(100);
      const position = s.entity.getPosition().clone();
      const forward = s.entity.forward.clone();
      s.controls.enabled = false;
      expect(s.entity.getPosition().distance(position)).toBeLessThan(1e-5);
      expect(s.entity.forward.dot(forward)).toBeCloseTo(1);
      expect(s.frames.size).toBe(0);
      expect(s.onViewChange).toHaveBeenCalledOnce();
    } finally { s.done(); }
  });
});
