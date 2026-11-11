// Browser-only controller fixture: real DOM/Pointer Lock, no Gaussian renderer.
import { GraphNode, Mat4, PROJECTION_ORTHOGRAPHIC, PROJECTION_PERSPECTIVE, Vec3, type Entity } from 'playcanvas';
import { ViewerControls, type ViewerCameraState } from '../../src/components/GaussianViewer/ViewerControls';

document.documentElement.style.cssText = 'font:14px Segoe UI,sans-serif;color:#d9e2f1;background:#151c29;';
document.body.style.cssText = 'margin:0;padding:20px;';
document.body.innerHTML = '<h1 style="margin:0 0 8px;font-size:20px">ViewerControls Pointer Lock verification</h1>'
  + '<p style="margin:0 0 12px;color:#9eacc3">Actual browser input and controller; schematic canvas, not Gaussian rendering.</p>'
  + '<label>Input-focus isolation <input id="outside-input" aria-label="Input-focus isolation"></label>'
  + '<canvas id="viewer-canvas" style="display:block;width:100%;height:56vh;margin-top:12px;outline:1px solid #51617b;cursor:grab;background:#0d1420"></canvas>'
  + '<pre id="status" style="margin:12px 0 0;white-space:pre-wrap;color:#aab7ca;font:12px Consolas,monospace"></pre>';

const canvas = document.querySelector<HTMLCanvasElement>('#viewer-canvas')!;
canvas.width = Math.max(1, canvas.clientWidth);
canvas.height = Math.max(1, canvas.clientHeight);
const context = canvas.getContext('2d')!;
const camera = { fov: 62, aspectRatio: canvas.clientWidth / canvas.clientHeight, nearClip: 0.01, farClip: 10_000,
  projection: PROJECTION_PERSPECTIVE, orthoHeight: 5, calculateProjection: null as null | ((matrix: Mat4) => void) };
const entity = new GraphNode();
Object.defineProperty(entity, 'camera', { value: camera });
let saves = 0, motions = 0, alive = true;
let controls: ViewerControls;
const events: Record<string, unknown>[] = [];
const record = (value: Record<string, unknown>) => { events.push({ at: performance.now(), ...value }); if (events.length > 400) events.shift(); };
const vector = (value: Vec3) => [value.x, value.y, value.z];

function draw() {
  if (!controls) return;
  const width = canvas.width, height = canvas.height;
  const eye = entity.getPosition(), forward = entity.forward, right = entity.right, up = entity.up;
  const focal = height / (2 * Math.tan(camera.fov * Math.PI / 360));
  const project = (point: Vec3) => {
    const relative = point.clone().sub(eye), depth = relative.dot(forward);
    if (depth <= 0.05) return null;
    const state = controls.snapshot();
    return [width / 2 + focal * camera.aspectRatio * (state.projectionOffset?.[0] ?? 0)
      + relative.dot(right) * focal * (state.projectionScaleX ?? 1) / depth,
    height / 2 + focal * (state.projectionOffset?.[1] ?? 0) - relative.dot(up) * focal / depth];
  };
  context.clearRect(0, 0, width, height);
  context.strokeStyle = '#344964'; context.lineWidth = 1;
  const line = (a: Vec3, b: Vec3) => { const start = project(a), end = project(b); if (!start || !end) return;
    context.beginPath(); context.moveTo(...start as [number, number]); context.lineTo(...end as [number, number]); context.stroke(); };
  for (let index = -12; index <= 12; index++) {
    line(new Vec3(index, 0, -12), new Vec3(index, 0, 12));
    line(new Vec3(-12, 0, index), new Vec3(12, 0, index));
  }
  context.strokeStyle = '#d98876'; line(new Vec3(-8, 0, 0), new Vec3(8, 0, 0));
  context.strokeStyle = '#7cc2a3'; line(new Vec3(0, -3, 0), new Vec3(0, 6, 0));
  context.strokeStyle = '#7e9cd2'; line(new Vec3(0, 0, -8), new Vec3(0, 0, 8));
  context.strokeStyle = '#b4c1d8'; context.beginPath(); context.moveTo(width / 2 - 8, height / 2); context.lineTo(width / 2 + 8, height / 2);
  context.moveTo(width / 2, height / 2 - 8); context.lineTo(width / 2, height / 2 + 8); context.stroke();
  const state = controls.snapshot();
  document.querySelector('#status')!.textContent = JSON.stringify({ locked: document.pointerLockElement === canvas,
    yaw: state.yaw, pitch: state.pitch, roll: state.roll, eye: vector(eye), saves, motions }, null, 2);
}

controls = new ViewerControls(canvas, entity as Entity, undefined, undefined, () => { saves++; draw(); }, () => { motions++; draw(); });
controls.setRoamingEnabled(true);
const initial: ViewerCameraState = { ...controls.snapshot(), target: [0, 0, 0], yaw: 0, pitch: 15, distance: 8,
  roll: 0, fov: 62, horizontalFrameOffset: 0, projectionOffset: [0, 0], projectionScaleX: 1 };
controls.restore(initial);

document.addEventListener('pointerlockchange', () => { record({ kind: 'pointerlockchange', locked: document.pointerLockElement === canvas }); draw(); });
document.addEventListener('pointerlockerror', event => record({ kind: 'pointerlockerror', trusted: event.isTrusted }));
document.addEventListener('mousemove', event => record({ kind: 'mousemove', dx: event.movementX, dy: event.movementY,
  buttons: event.buttons, trusted: event.isTrusted, locked: document.pointerLockElement === canvas }));
window.addEventListener('keydown', event => record({ kind: 'keydown', code: event.code, trusted: event.isTrusted, target: (event.target as Element)?.id }));
window.addEventListener('keyup', event => record({ kind: 'keyup', code: event.code, trusted: event.isTrusted }));
window.addEventListener('unhandledrejection', event => record({ kind: 'unhandledrejection', error: String(event.reason) }));

const fixture = {
  snapshot() {
    const projection = new Mat4(); camera.calculateProjection?.(projection);
    return { state: controls.snapshot(), eye: vector(entity.getPosition()), forward: vector(entity.forward), up: vector(entity.up),
      locked: document.pointerLockElement === canvas, controllerLocked: controls.isMouseLookActive(), focus: (document.activeElement as Element)?.id,
      cursor: getComputedStyle(canvas).cursor, pointerLockAvailable: typeof canvas.requestPointerLock === 'function',
      saves, motions, alive, projection: [...projection.data], events: [...events], userAgent: navigator.userAgent };
  },
  reset(state: Partial<ViewerCameraState> = {}) {
    controls.enabled = true; controls.setRoamingEnabled(true);
    controls.restore({ ...initial, ...state });
    events.length = 0; saves = 0; motions = 0; draw();
  },
  focus() { return controls.focus(); },
  setEnabled(value: boolean) { controls.enabled = value; },
  setRoamingEnabled(value: boolean) { controls.setRoamingEnabled(value); },
  setOrthographic(value: boolean) { controls.restore({ ...controls.snapshot(), projection: value ? PROJECTION_ORTHOGRAPHIC : PROJECTION_PERSPECTIVE }); },
  destroy() { controls.destroy(); alive = false; },
  canvasRect() { const rect = canvas.getBoundingClientRect(); return { x: rect.x, y: rect.y, width: rect.width, height: rect.height }; },
  mountDeniedFrame() {
    let frame = document.querySelector<HTMLIFrameElement>('#denied-frame');
    if (frame) return;
    frame = document.createElement('iframe'); frame.id = 'denied-frame';
    frame.setAttribute('sandbox', 'allow-scripts allow-same-origin');
    frame.style.cssText = 'position:fixed;left:24px;top:100px;width:720px;height:480px;border:0;background:#151c29;z-index:10';
    frame.src = '/?embedded=denied'; document.body.append(frame);
  },
};
(window as Window & { viewerMouseLookFixture?: typeof fixture }).viewerMouseLookFixture = fixture;
draw();
