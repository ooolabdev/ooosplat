// @vitest-environment jsdom
import { BoundingBox, GraphNode, PROJECTION_ORTHOGRAPHIC, PROJECTION_PERSPECTIVE, Vec3, type Entity } from "playcanvas";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ViewerControls } from "./ViewerControls";

let owner: Element | null;
let hidden: boolean;
let now: number;
let nextFrame: number;
let frames: Map<number, FrameRequestCallback>;
let exit: ReturnType<typeof vi.fn>;
let originalLock: PropertyDescriptor | undefined;
let originalExit: PropertyDescriptor | undefined;
let originalHidden: PropertyDescriptor | undefined;
let originalVisibility: PropertyDescriptor | undefined;
const instances: ReturnType<typeof setup>[] = [];

function setOwner(value: Element | null, notify = true) {
  owner = value;
  if (notify) document.dispatchEvent(new Event("pointerlockchange"));
}
function setup(available = true) {
  const canvas = document.createElement("canvas");
  const captures = new Set<number>();
  const request = vi.fn<() => void | Promise<void>>(() => undefined);
  Object.defineProperties(canvas, {
    clientWidth: { value: 1200 }, clientHeight: { value: 800 },
    requestPointerLock: { configurable: true, value: available ? request : undefined },
    setPointerCapture: { value: (id: number) => captures.add(id) },
    releasePointerCapture: { value: (id: number) => captures.delete(id) },
    hasPointerCapture: { value: (id: number) => captures.has(id) },
  });
  const camera = { fov: 52, aspectRatio: 1.5, nearClip: .01, farClip: 10_000, projection: PROJECTION_PERSPECTIVE, orthoHeight: 5 };
  const entity = new GraphNode();
  Object.defineProperty(entity, "camera", { value: camera });
  const lookAt = vi.spyOn(entity, "lookAt");
  const saved = vi.fn(), motion = vi.fn();
  const controls = new ViewerControls(canvas, entity as Entity, undefined, undefined, saved, motion);
  document.body.append(canvas);
  controls.setRoamingEnabled(true);
  canvas.focus();
  const key = (type: "keydown" | "keyup", code: string, options: KeyboardEventInit = {}) => {
    const event = new KeyboardEvent(type, { code, key: code.replace("Key", "").toLowerCase(), bubbles: true, cancelable: true, ...options });
    canvas.dispatchEvent(event);
    return event;
  };
  return { canvas, request, camera, entity, controls, lookAt, saved, motion, key };
}
function create(available = true) {
  const state = setup(available);
  instances.push(state);
  return state;
}
function advance(timestamp: number) {
  now = timestamp;
  const pending = [...frames.values()];
  frames.clear();
  pending.forEach(callback => callback(timestamp));
}
function move(x: number, y: number) {
  const event = new MouseEvent("mousemove", { bubbles: true, cancelable: true });
  Object.defineProperties(event, { movementX: { value: x }, movementY: { value: y } });
  document.dispatchEvent(event);
}
function pointer(type: string, x: number, y: number, button = 0) {
  const event = new MouseEvent(type, { clientX: x, clientY: y, button, bubbles: true, cancelable: true });
  Object.defineProperty(event, "pointerId", { value: 5 });
  return event;
}
function deferred() {
  let resolve!: () => void, reject!: (reason: unknown) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function restoreDescriptor(name: string, descriptor: PropertyDescriptor | undefined) {
  if (descriptor) Object.defineProperty(document, name, descriptor);
  else Reflect.deleteProperty(document, name);
}
beforeEach(() => {
  owner = null; hidden = false; now = 0; nextFrame = 0; frames = new Map();
  originalLock = Object.getOwnPropertyDescriptor(document, "pointerLockElement");
  originalExit = Object.getOwnPropertyDescriptor(document, "exitPointerLock");
  originalHidden = Object.getOwnPropertyDescriptor(document, "hidden");
  originalVisibility = Object.getOwnPropertyDescriptor(document, "visibilityState");
  exit = vi.fn(() => setOwner(null));
  Object.defineProperties(document, {
    pointerLockElement: { configurable: true, get: () => owner },
    exitPointerLock: { configurable: true, value: exit },
    hidden: { configurable: true, get: () => hidden },
    visibilityState: { configurable: true, get: () => hidden ? "hidden" : "visible" },
  });
  vi.spyOn(window, "requestAnimationFrame").mockImplementation(callback => { frames.set(++nextFrame, callback); return nextFrame; });
  vi.spyOn(window, "cancelAnimationFrame").mockImplementation(id => { frames.delete(id); });
  vi.spyOn(performance, "now").mockImplementation(() => now);
  vi.spyOn(console, "debug").mockImplementation(() => undefined);
});
afterEach(() => {
  for (const state of instances.splice(0)) { state.controls.destroy(); state.canvas.remove(); }
  // A cancelled, legacy void request still awaits the browser's outcome.
  // Resolve that outcome so its document-level guard cannot leak between tests.
  document.dispatchEvent(new Event("pointerlockerror"));
  owner = null;
  restoreDescriptor("pointerLockElement", originalLock);
  restoreDescriptor("exitPointerLock", originalExit);
  restoreDescriptor("hidden", originalHidden);
  restoreDescriptor("visibilityState", originalVisibility);
  vi.restoreAllMocks();
});

describe("held-WASD pointer-lock mouse look", () => {
  it.each(["void", "promise"])("requests synchronously once and trusts actual ownership, not a %s return", async kind => {
    const s = create();
    if (kind === "promise") s.request.mockResolvedValue(undefined);
    const before = s.controls.snapshot();
    s.key("keydown", "KeyW");
    expect(s.request).toHaveBeenCalledTimes(1);
    expect(s.controls.isMouseLookActive()).toBe(false);
    await Promise.resolve();
    expect(s.controls.isMouseLookActive()).toBe(false);
    move(20, 10); advance(0);
    expect(s.controls.snapshot().yaw).toBe(before.yaw);
    setOwner(s.canvas);
    expect(s.controls.isMouseLookActive()).toBe(true);
    s.key("keydown", "KeyW", { repeat: true }); s.key("keydown", "KeyD");
    expect(s.request).toHaveBeenCalledTimes(1);
    s.key("keyup", "KeyW");
    expect(s.controls.isMouseLookActive()).toBe(true);
    expect(exit).not.toHaveBeenCalled();
    s.key("keyup", "KeyD");
    expect(s.controls.isMouseLookActive()).toBe(false);
    expect(exit).toHaveBeenCalledTimes(1);
  });

  it("merges relative mouse deltas into one frame and turns around an unchanged eye", () => {
    const s = create();
    s.controls.restore({ ...s.controls.snapshot(), target: [4, -2, 7], yaw: 32, pitch: 18, roll: 23,
      distance: 8.5, horizontalFrameOffset: .21, fov: 71, projectionOffset: [.12, -.08], projectionScaleX: 1.25 });
    const before = s.controls.snapshot(), eye = s.entity.getPosition().clone(), lookCalls = s.lookAt.mock.calls.length;
    s.key("keydown", "KeyW"); setOwner(s.canvas);
    move(10, 6); move(-3, 4); move(5, -2);
    expect(s.controls.snapshot()).toEqual(before);
    expect(frames.size).toBe(1);
    advance(0);
    const after = s.controls.snapshot();
    expect(after.yaw).toBeCloseTo(before.yaw - 12 * .3, 8);
    expect(after.pitch).toBeCloseTo(before.pitch + 8 * .3, 8);
    expect(s.entity.getPosition().distance(eye)).toBeLessThan(1e-5);
    expect(after.roll).toBe(before.roll);
    expect(after.distance).toBe(before.distance);
    expect(after.fov).toBe(before.fov);
    expect(after.horizontalFrameOffset).toBe(before.horizontalFrameOffset);
    expect(after.projectionOffset).toEqual(before.projectionOffset);
    expect(after.projectionScaleX).toBe(before.projectionScaleX);
    expect(s.lookAt.mock.calls.length - lookCalls).toBe(1);
    expect(s.saved).not.toHaveBeenCalled();
    s.key("keyup", "KeyW");
    expect(s.saved).toHaveBeenCalledTimes(1);
    expect(frames.size).toBe(0);
  });

  it("flushes the final look on last-key release instead of losing a frame", () => {
    const s = create(), before = s.controls.snapshot();
    s.key("keydown", "KeyA"); setOwner(s.canvas); move(11, 7);
    s.key("keyup", "KeyA");
    expect(s.controls.snapshot().yaw).toBeCloseTo(before.yaw - 3.3);
    expect(s.controls.snapshot().pitch).toBeCloseTo(before.pitch + 2.1);
    expect(s.saved).toHaveBeenCalledTimes(1);
    expect(frames.size).toBe(0);
    const stopped = s.controls.snapshot(); move(50, 50); advance(16);
    expect(s.controls.snapshot()).toEqual(stopped);
  });

  it("clamps vertical look and ignores non-finite relative input", () => {
    const s = create(); s.key("keydown", "KeyW"); setOwner(s.canvas);
    move(0, 100_000); advance(0);
    expect(s.controls.snapshot().pitch).toBeCloseTo(89.9);
    move(0, -100_000); advance(0);
    expect(s.controls.snapshot().pitch).toBeCloseTo(-89.9);
    const before = s.controls.snapshot(); move(Number.NaN, Number.POSITIVE_INFINITY); advance(0);
    expect(s.controls.snapshot().yaw).toBe(before.yaw);
    expect(s.controls.snapshot().pitch).toBe(before.pitch);
  });

  it("combines sight-direction flight with horizontal strafing and normalizes diagonal speed", () => {
    const s = create(); s.controls.restore({ ...s.controls.snapshot(), yaw: 0, pitch: 10, roll: 15 });
    s.key("keydown", "KeyW", { shiftKey: true }); s.key("keydown", "KeyD", { shiftKey: true }); setOwner(s.canvas);
    move(0, 100); advance(0);
    const before = s.controls.snapshot(), position = s.entity.getPosition().clone();
    const direction = s.entity.forward.clone().add(new Vec3(1, 0, 0)).normalize();
    advance(16);
    const delta = s.entity.getPosition().clone().sub(position);
    const translatedTarget = new Vec3(...s.controls.snapshot().target).sub(new Vec3(...before.target));
    expect(delta.clone().normalize().dot(direction)).toBeCloseTo(1, 7);
    expect(delta.y).toBeLessThan(0);
    expect(translatedTarget.length()).toBeCloseTo(.15 * 2 * before.distance * Math.tan(before.fov! * Math.PI / 360) * .016 * 3, 7);
    expect(delta.distance(translatedTarget)).toBeLessThan(1e-6);
    expect(s.controls.snapshot().pitch).toBeCloseTo(40);
    expect(s.controls.snapshot().roll).toBe(15);
  });

  it.each([
    [-200, "KeyW", 1], [200, "KeyW", -1],
    [-200, "KeyS", -1], [200, "KeyS", 1],
  ] as const)("follows the newly steered view vertically with mouse Y=%s and %s", (mouseY, code, verticalSign) => {
    const s = create();
    s.controls.restore({ ...s.controls.snapshot(), yaw: 31, pitch: 0, roll: 74, fov: 67 });
    s.key("keydown", code); setOwner(s.canvas); move(0, mouseY); advance(0);
    const before = s.controls.snapshot(), eye = s.entity.getPosition().clone();
    const direction = s.entity.forward.clone().mulScalar(code === "KeyW" ? 1 : -1).normalize();
    advance(16);
    const movement = s.entity.getPosition().clone().sub(eye);
    const translatedTarget = new Vec3(...s.controls.snapshot().target).sub(new Vec3(...before.target));
    expect(Math.sign(movement.y)).toBe(verticalSign);
    expect(movement.clone().normalize().dot(direction)).toBeCloseTo(1, 7);
    expect(translatedTarget.length()).toBeCloseTo(.15 * 2 * before.distance * Math.tan(67 * Math.PI / 360) * .016, 7);
    expect(movement.distance(translatedTarget)).toBeLessThan(1e-6);
    expect(s.controls.snapshot().roll).toBe(74);
    expect(s.controls.snapshot().fov).toBe(67);
    expect(s.controls.isMouseLookActive()).toBe(true);
    s.key("keyup", code);
    expect(s.controls.isMouseLookActive()).toBe(false);
  });

  it.each([
    [90, 122, "KeyW", -1], [-90, -122, "KeyW", 1],
    [90, -48, "KeyS", 1], [-90, 48, "KeyS", -1],
  ] as const)("keeps exact-pole flight finite with pitch %s, roll %s and %s", (pitch, roll, code, verticalSign) => {
    const s = create();
    s.controls.restore({ ...s.controls.snapshot(), yaw: 47, pitch, roll });
    const before = s.controls.snapshot(), eye = s.entity.getPosition().clone(), rotation = s.entity.getRotation().clone();
    s.key("keydown", code); setOwner(s.canvas); advance(16);
    const movement = s.entity.getPosition().clone().sub(eye);
    const translatedTarget = new Vec3(...s.controls.snapshot().target).sub(new Vec3(...before.target));
    expect([movement.x, movement.y, movement.z].every(Number.isFinite)).toBe(true);
    expect(Math.sign(movement.y)).toBe(verticalSign);
    expect(Math.abs(movement.x)).toBeLessThan(1e-6);
    expect(Math.abs(movement.z)).toBeLessThan(1e-6);
    expect(translatedTarget.length()).toBeCloseTo(.15 * 2 * before.distance * Math.tan(before.fov! * Math.PI / 360) * .016, 7);
    expect(movement.distance(translatedTarget)).toBeLessThan(1e-6);
    expect(s.entity.getRotation().equalsApprox(rotation)).toBe(true);
    expect(s.controls.snapshot().roll).toBe(roll);
    expect(s.controls.isMouseLookActive()).toBe(true);
  });

  it.each(["KeyA", "KeyD"])("keeps %s horizontal at a rolled, pitched locked view", code => {
    const s = create();
    s.controls.restore({ ...s.controls.snapshot(), yaw: -57, pitch: -76, roll: 90 });
    const before = s.controls.snapshot(), eye = s.entity.getPosition().clone();
    const yaw = before.yaw * Math.PI / 180;
    const direction = new Vec3(Math.cos(yaw), 0, -Math.sin(yaw)).mulScalar(code === "KeyD" ? 1 : -1);
    s.key("keydown", code); setOwner(s.canvas); advance(16);
    const movement = s.entity.getPosition().clone().sub(eye);
    expect(movement.y).toBe(0);
    expect(movement.clone().normalize().dot(direction)).toBeCloseTo(1, 7);
    expect(movement.length()).toBeCloseTo(.15 * 2 * before.distance * Math.tan(before.fov! * Math.PI / 360) * .016, 7);
  });

  it.each(["last key", "destroy"])("releases a late legacy void grant after %s", reason => {
    const s = create(); s.key("keydown", "KeyW");
    if (reason === "last key") s.key("keyup", "KeyW");
    else s.controls.destroy();
    setOwner(s.canvas);
    expect(owner).toBeNull();
    expect(exit).toHaveBeenCalledTimes(1);
    expect(s.controls.isMouseLookActive()).toBe(false);
    const stopped = s.controls.snapshot(); move(20, 10); advance(16);
    expect(s.controls.snapshot()).toEqual(stopped);
  });

  it.each(["last key", "destroy"])("releases a late promise grant after %s", async reason => {
    const s = create(), pending = deferred(); s.request.mockReturnValue(pending.promise);
    s.key("keydown", "KeyW");
    if (reason === "last key") s.key("keyup", "KeyW");
    else s.controls.destroy();
    setOwner(s.canvas, false); pending.resolve(); await Promise.resolve(); await Promise.resolve();
    expect(owner).toBeNull();
    expect(exit).toHaveBeenCalledTimes(1);
    expect(s.controls.isMouseLookActive()).toBe(false);
  });

  it("stops on ownership loss without letting key auto-repeat recapture the pointer", () => {
    const s = create(); s.key("keydown", "KeyW"); setOwner(s.canvas); advance(16);
    setOwner(null);
    const stopped = s.controls.snapshot();
    s.key("keydown", "KeyW", { repeat: true }); move(50, 20); advance(32);
    expect(s.controls.snapshot()).toEqual(stopped);
    expect(frames.size).toBe(0);
    expect(s.request).toHaveBeenCalledTimes(1);
    s.key("keyup", "KeyW"); s.key("keydown", "KeyW");
    expect(s.request).toHaveBeenCalledTimes(2);
  });

  it("recognizes an ownership loss even if the browser granted before its first change event", () => {
    const s = create(); s.key("keydown", "KeyW"); setOwner(s.canvas, false);
    expect(s.controls.isMouseLookActive()).toBe(true);
    move(3, 2); advance(16);
    setOwner(null);
    const stopped = s.controls.snapshot(); s.key("keydown", "KeyW", { repeat: true }); advance(32);
    expect(s.controls.snapshot()).toEqual(stopped);
    expect(frames.size).toBe(0);
    expect(s.request).toHaveBeenCalledTimes(1);
  });

  it("handles Escape immediately and requires a fresh key press to enter again", () => {
    const s = create(); s.key("keydown", "KeyW"); setOwner(s.canvas); move(8, 3);
    const escape = s.key("keydown", "Escape");
    expect(escape.defaultPrevented).toBe(true);
    expect(owner).toBeNull(); expect(frames.size).toBe(0);
    const stopped = s.controls.snapshot(); s.key("keydown", "KeyW", { repeat: true }); advance(16);
    expect(s.controls.snapshot()).toEqual(stopped);
    expect(s.request).toHaveBeenCalledTimes(1);
    expect(s.saved).toHaveBeenCalledTimes(1);
  });

  it.each(["window blur", "canvas blur", "disabled", "mode", "projection", "visibility", "pointercancel"])("releases pointer lock and stops on %s", reason => {
    const s = create(); s.key("keydown", "KeyW"); setOwner(s.canvas); advance(16);
    if (reason === "window blur") window.dispatchEvent(new Event("blur"));
    if (reason === "canvas blur") s.canvas.blur();
    if (reason === "disabled") s.controls.enabled = false;
    if (reason === "mode") s.controls.setRoamingEnabled(false);
    if (reason === "projection") s.controls.alignOrthographic("front", new BoundingBox(new Vec3(), new Vec3(1, 1, 1)), .85, false);
    if (reason === "visibility") { hidden = true; document.dispatchEvent(new Event("visibilitychange")); }
    if (reason === "pointercancel") s.canvas.dispatchEvent(pointer("pointercancel", 0, 0));
    expect(owner).toBeNull(); expect(frames.size).toBe(0);
    const stopped = s.controls.snapshot(); move(50, 20); advance(32);
    expect(s.controls.snapshot()).toEqual(stopped);
    expect(s.request).toHaveBeenCalledTimes(1);
  });

  it.each(["missing", "throws", "rejects", "error event"])("retains keyboard roaming and drag navigation when pointer lock %s", async failure => {
    const s = create(failure !== "missing");
    if (failure === "throws") s.request.mockImplementation(() => { throw new DOMException("Denied", "NotAllowedError"); });
    if (failure === "rejects") s.request.mockRejectedValue(new DOMException("Denied", "NotAllowedError"));
    const before = s.controls.snapshot(); s.key("keydown", "KeyW");
    if (failure === "error event") document.dispatchEvent(new Event("pointerlockerror"));
    await Promise.resolve(); await Promise.resolve();
    advance(16);
    expect(s.controls.isMouseLookActive()).toBe(false);
    expect(new Vec3(...s.controls.snapshot().target).distance(new Vec3(...before.target))).toBeGreaterThan(0);
    s.key("keydown", "KeyW", { repeat: true }); s.key("keydown", "KeyD");
    expect(s.request).toHaveBeenCalledTimes(failure === "missing" ? 0 : 1);
    s.key("keyup", "KeyW"); s.key("keyup", "KeyD");
    const yaw = s.controls.snapshot().yaw;
    s.canvas.dispatchEvent(pointer("pointerdown", 0, 0));
    s.canvas.dispatchEvent(pointer("pointermove", 15, 0));
    s.canvas.dispatchEvent(pointer("pointerup", 15, 0));
    expect(s.controls.snapshot().yaw).not.toBe(yaw);
  });

  it("never releases another canvas's lock when an old request settles", async () => {
    const old = create(), pending = deferred(); old.request.mockReturnValue(pending.promise);
    old.key("keydown", "KeyW"); old.key("keyup", "KeyW"); old.controls.destroy();
    const current = document.createElement("canvas"); document.body.append(current);
    try {
      setOwner(current);
      pending.resolve(); await Promise.resolve(); await Promise.resolve();
      expect(owner).toBe(current);
      expect(exit).not.toHaveBeenCalled();
      expect(old.controls.isMouseLookActive()).toBe(false);
    } finally { current.remove(); }
  });

  it.each(["resolves", "rejects"])("does not let an old promise that %s clear the next pending request", async outcome => {
    const s = create(), old = deferred(), current = deferred();
    s.request.mockReturnValueOnce(old.promise).mockReturnValue(current.promise);
    s.key("keydown", "KeyW"); setOwner(s.canvas); s.key("keyup", "KeyW");
    s.key("keydown", "KeyW");
    expect(s.request).toHaveBeenCalledTimes(2);
    if (outcome === "resolves") old.resolve();
    else old.reject(new DOMException("Old request cancelled", "AbortError"));
    await Promise.resolve(); await Promise.resolve();
    s.key("keyup", "KeyW"); s.key("keydown", "KeyD");
    // The second browser request is still pending. Its cancellation cannot
    // be bypassed by an old promise clearing the per-document request guard.
    expect(s.request).toHaveBeenCalledTimes(2);
    setOwner(s.canvas);
    expect(owner).toBeNull();
    expect(s.controls.isMouseLookActive()).toBe(false);
    expect(exit).toHaveBeenCalledTimes(2);
    current.resolve(); await Promise.resolve();
  });

  it("does not capture the pointer from text fields, shortcuts, orthographic views or disabled controls", () => {
    const s = create();
    const input = document.createElement("input"); document.body.append(input); input.focus();
    input.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyW", bubbles: true })); input.remove();
    s.canvas.focus(); s.key("keydown", "KeyA", { ctrlKey: true });
    s.controls.enabled = false; s.key("keydown", "KeyW"); s.controls.enabled = true;
    s.controls.setRoamingEnabled(false); s.key("keydown", "KeyW"); s.controls.setRoamingEnabled(true);
    s.controls.restore({ ...s.controls.snapshot(), projection: PROJECTION_ORTHOGRAPHIC }); s.key("keydown", "KeyW");
    expect(s.request).not.toHaveBeenCalled();
    expect(s.controls.isMouseLookActive()).toBe(false);
    expect(frames.size).toBe(0);
  });

  it("flushes a queued look before disposal and ignores subsequent pointer events", () => {
    const s = create(), before = s.controls.snapshot(); s.key("keydown", "KeyW"); setOwner(s.canvas); move(9, 4);
    s.controls.destroy();
    expect(s.controls.snapshot().yaw).toBeCloseTo(before.yaw - 2.7);
    expect(s.controls.snapshot().pitch).toBeCloseTo(before.pitch + 1.2);
    expect(s.saved).toHaveBeenCalledTimes(1);
    expect(owner).toBeNull(); expect(frames.size).toBe(0);
    const stopped = s.controls.snapshot(); move(100, 100); advance(16);
    expect(s.controls.snapshot()).toEqual(stopped);
  });
});
