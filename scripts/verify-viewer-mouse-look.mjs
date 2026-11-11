// Actual browser Pointer Lock + ViewerControls verification. No Gaussian/GPU
// renderer, native Tauri window, model file or training is exercised here.
import { build } from 'vite';
import { createServer } from 'node:http';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import assert from 'node:assert/strict';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const out = path.join(root, '.cache/viewer-mouse-look', String(Date.now()));
await mkdir(out, { recursive: true });
await build({ root, logLevel: 'warn', define: { 'process.env.NODE_ENV': '"production"' }, build: {
  outDir: out, emptyOutDir: false, lib: { entry: path.join(root, 'scripts/fixtures/viewer-mouse-look.ts'), name: 'ViewerMouseLookFixture', formats: ['iife'], fileName: () => 'runtime.js' },
} });
const server = createServer(async (req, res) => {
  try {
    const pathname = new URL(req.url, 'http://localhost').pathname;
    if (pathname === '/') { res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end('<meta charset="utf-8"><title>Viewer mouse-look verification</title><script defer src="/runtime.js"></script>'); }
    else if (pathname === '/runtime.js') { res.setHeader('Content-Type', 'application/javascript'); res.end(await readFile(path.join(out, 'runtime.js'))); }
    else { res.statusCode = 404; res.end(); }
  } catch (error) { res.statusCode = 500; res.end(String(error)); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = spawn(process.env.OOOSPLAT_UI_TEST_BROWSER || 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', [
  '--headless=new', '--no-first-run', '--disable-background-networking', '--disable-sync', '--disable-extensions',
  '--remote-debugging-port=0', `--user-data-dir=${path.join(out, 'profile')}`, 'about:blank',
], { windowsHide: true, stdio: ['ignore', 'ignore', 'pipe'] });
let diagnostics = '', socket, ready, fail;
const startup = new Promise((resolve, reject) => { ready = resolve; fail = reject; });
browser.on('error', fail); browser.on('exit', code => fail(Error(`Browser exited ${code}`)));
browser.stderr.on('data', value => { diagnostics += value; const port = diagnostics.match(/DevTools listening on ws:\/\/127\.0\.0\.1:(\d+)/)?.[1]; if (port) ready(Number(port)); });
const startupTimer = setTimeout(() => fail(Error('Browser startup timed out')), 30000);
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const proofs = [], exceptions = [], consoleMessages = [];
const distance = (a, b) => Math.hypot(...a.map((value, index) => value - b[index]));
const assertTranslationAlong = (before, after, direction, context) => {
  const movement = after.eye.map((value, index) => value - before.eye[index]);
  const length = Math.hypot(...movement), directionLength = Math.hypot(...direction);
  assert.ok(length > 0.001, `${context}: held key must move the camera`);
  const unit = movement.map(value => value / length), expected = direction.map(value => value / directionLength);
  const directionError = distance(unit, expected);
  assert.ok(directionError < 0.0001, `${context}: movement must follow the expected direction, error ${directionError}`);
  const targetMovement = after.state.target.map((value, index) => value - before.state.target[index]);
  assert.ok(distance(movement, targetMovement) < 0.00001, `${context}: eye and observation center must translate together`);
  return { movement, targetMovement, length, expected, directionError };
};
let activeCase = 'startup';
let failureSnapshot;
try {
  const port = await startup;
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  socket = new WebSocket(pages.find(page => page.type === 'page').webSocketDebuggerUrl);
  await new Promise(resolve => socket.addEventListener('open', resolve, { once: true }));
  let sequence = 0;
  socket.addEventListener('message', event => {
    const value = JSON.parse(event.data);
    if (value.method === 'Runtime.exceptionThrown') exceptions.push(value.params.exceptionDetails);
    if (value.method === 'Runtime.consoleAPICalled') consoleMessages.push({ type: value.params.type, args: value.params.args.map(arg => arg.value ?? arg.description) });
  });
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence;
    const timer = setTimeout(() => { socket.removeEventListener('message', receive); reject(Error(`${method} timed out`)); }, 15000);
    const receive = event => { const value = JSON.parse(event.data); if (value.id !== id) return; clearTimeout(timer); socket.removeEventListener('message', receive); value.error ? reject(Error(JSON.stringify(value.error))) : resolve(value.result); };
    socket.addEventListener('message', receive); socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => { const value = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (value.exceptionDetails) throw Error(JSON.stringify(value.exceptionDetails)); return value.result.value; };
  const wait = async (expression, timeout = 5000) => { const started = Date.now(); while (Date.now() - started < timeout) { if (await evaluate(expression)) return; await pause(25); } throw Error(`${activeCase}: waiting for ${expression}`); };
  const api = 'window.viewerMouseLookFixture';
  const snapshot = (target = api) => evaluate(`${target}.snapshot()`);
  failureSnapshot = () => evaluate(`({fixture: ${api}?.snapshot(), documentFocus: document.hasFocus(), visibility: document.visibilityState, activation: {isActive: navigator.userActivation.isActive, hasBeenActive: navigator.userActivation.hasBeenActive}})`);
  const key = async (code, down = true, extra = {}) => {
    const value = code === 'Escape' ? 'Escape' : code.replace('Key', '').toLowerCase();
    const virtual = code === 'Escape' ? 27 : code.charCodeAt(code.length - 1);
    await call('Input.dispatchKeyEvent', { type: down ? 'keyDown' : 'keyUp', code, key: value, windowsVirtualKeyCode: virtual, nativeVirtualKeyCode: virtual, ...extra });
  };
  const mouse = (x, y, type = 'mouseMoved', extra = {}) => call('Input.dispatchMouseEvent', { type, x, y, button: 'none', buttons: 0, ...extra });
  const focusCanvas = async (target = api, offset = { x: 0, y: 0 }) => {
    const rect = await evaluate(`${target}.canvasRect()`);
    const x = offset.x + rect.x + rect.width / 2, y = offset.y + rect.y + rect.height / 2;
    await mouse(x, y); await mouse(x, y, 'mousePressed', { button: 'left', buttons: 1, clickCount: 1 });
    await mouse(x, y, 'mouseReleased', { button: 'left', buttons: 0, clickCount: 1 });
    await wait(`${target}.snapshot().focus==='viewer-canvas'`);
    return { x, y };
  };
  // These independent cases acquire/release native lock repeatedly. Pace them
  // like separate interactions so the browser's anti-abuse request quota is
  // respected, rather than changing permissions or retrying a denied request.
  const fresh = async state => { await pause(1100); await evaluate(`${api}.reset(${JSON.stringify(state ?? {})})`); return focusCanvas(); };
  const save = async (name, values) => { proofs.push({ name, ...values }); await writeFile(path.join(out, 'proof.json'), JSON.stringify({ actualPointerLockAPI: true, controller: 'ViewerControls', renderer: 'schematic 2D canvas only', nativeTauri: false, proofs }, null, 2)); };
  await call('Runtime.enable'); await call('Emulation.setDeviceMetricsOverride', { width: 1100, height: 780, deviceScaleFactor: 1, mobile: false });
  await call('Page.navigate', { url: `http://127.0.0.1:${server.address().port}/` });
  await wait(`!!${api}`);
  await call('Page.bringToFront');

  activeCase = 'focused-W-lock-and-mouse-look';
  let point = await fresh(), before = await snapshot();
  assert.equal(before.pointerLockAvailable, true, 'The actual browser must expose Pointer Lock');
  await key('KeyW'); await wait(`${api}.snapshot().locked`); await pause(100);
  const moving = await snapshot(), initialTranslation = assertTranslationAlong(before, moving, before.forward, activeCase);
  await mouse(point.x + 45, point.y + 20); await pause(70);
  const looked = await snapshot();
  assert.ok(Math.abs(looked.state.yaw - moving.state.yaw) > 0.01, 'No-button mouse motion must change yaw while W is held');
  assert.ok(Math.abs(looked.state.pitch - moving.state.pitch) > 0.01, 'No-button mouse motion must change pitch');
  assert.ok(looked.events.some(event => event.kind === 'keydown' && event.code === 'KeyW' && event.trusted));
  assert.ok(looked.events.some(event => event.kind === 'mousemove' && event.locked && event.trusted && event.buttons === 0 && Math.abs(event.dx) > 0));
  await key('KeyW', false); await wait(`!${api}.snapshot().locked`);
  const stopped = await snapshot(); await pause(120); const afterStop = await snapshot();
  assert.ok(distance(stopped.eye, afterStop.eye) < 1e-7); assert.equal(stopped.cursor, before.cursor);
  await mouse(point.x - 40, point.y - 20); await pause(60);
  const unlocked = await snapshot(); assert.equal(unlocked.state.yaw, stopped.state.yaw); assert.equal(unlocked.state.pitch, stopped.state.pitch);
  await save(activeCase, { before, moving, initialTranslation, looked, stopped, afterStop, unlocked });

  // Trusted key presses use the actual controller and browser lock. W/S follows
  // the current view direction, whereas A/D deliberately ignores pitch/roll.
  for (const view of [
    { name: 'up', yaw: 23, pitch: -35, roll: 19 },
    { name: 'down', yaw: -41, pitch: 35, roll: -24 },
  ]) for (const code of ['KeyW', 'KeyS']) {
    activeCase = `line-of-sight-${view.name}-${code}`;
    await fresh({ yaw: view.yaw, pitch: view.pitch, roll: view.roll });
    const start = await snapshot(), sign = code === 'KeyW' ? 1 : -1;
    await key(code); await wait(`${api}.snapshot().locked`); await pause(100);
    await key(code, false); await wait(`!${api}.snapshot().locked`);
    const end = await snapshot();
    const translation = assertTranslationAlong(start, end, start.forward.map(value => value * sign), activeCase);
    assert.ok((end.eye[1] - start.eye[1]) * (view.name === 'up' ? sign : -sign) > 0.001, `${activeCase}: vertical direction must match the view and W/S`);
    for (const field of ['yaw', 'pitch', 'roll', 'distance', 'fov']) assert.equal(end.state[field], start.state[field], activeCase);
    assert.ok(end.events.some(event => event.kind === 'keydown' && event.code === code && event.trusted), activeCase);
    await save(activeCase, { start, end, translation });
  }

  for (const view of [
    { name: 'up-positive-roll', yaw: 37, pitch: -70, roll: 61 },
    { name: 'down-negative-roll', yaw: -28, pitch: 70, roll: -53 },
  ]) for (const code of ['KeyA', 'KeyD']) {
    activeCase = `horizontal-strafe-${view.name}-${code}`;
    await fresh({ yaw: view.yaw, pitch: view.pitch, roll: view.roll });
    const start = await snapshot(), sign = code === 'KeyD' ? 1 : -1, yaw = view.yaw * Math.PI / 180;
    await key(code); await wait(`${api}.snapshot().locked`); await pause(100);
    await key(code, false); await wait(`!${api}.snapshot().locked`);
    const end = await snapshot();
    const translation = assertTranslationAlong(start, end, [Math.cos(yaw) * sign, 0, -Math.sin(yaw) * sign], activeCase);
    assert.ok(Math.abs(end.eye[1] - start.eye[1]) < 0.000001, `${activeCase}: A/D must remain horizontal even with pitch and roll`);
    for (const field of ['yaw', 'pitch', 'roll', 'distance', 'fov']) assert.equal(end.state[field], start.state[field], activeCase);
    assert.ok(end.events.some(event => event.kind === 'keydown' && event.code === code && event.trusted), activeCase);
    await save(activeCase, { start, end, translation });
  }

  activeCase = 'combined-keys-and-last-key-release';
  point = await fresh({ pitch: 0 });
  await key('KeyW'); await wait(`${api}.snapshot().locked`); await key('KeyD'); await pause(35);
  const diagonalStart = await snapshot(); await pause(140); const diagonalEnd = await snapshot();
  const dx = diagonalEnd.eye[0] - diagonalStart.eye[0], dz = diagonalEnd.eye[2] - diagonalStart.eye[2];
  assert.ok(dx > 0 && dz < 0); assert.ok(Math.abs(dx + dz) < 1e-5, 'W+D must remain a normalized horizontal diagonal');
  await key('KeyW', false); assert.equal((await snapshot()).locked, true, 'D still owns this navigation session');
  await key('KeyD', false); await wait(`!${api}.snapshot().locked`);
  await save(activeCase, { diagonalStart, diagonalEnd, stopped: await snapshot() });

  activeCase = 'opposed-keys-fixed-eye-and-reference-projection';
  point = await fresh({ target: [2, 1, -4], yaw: 45, pitch: 20, distance: 7, roll: 27, fov: 78,
    horizontalFrameOffset: 0.3, projectionOffset: [0.12, -0.09], projectionScaleX: 1.3 });
  await key('KeyW'); await key('KeyS'); await wait(`${api}.snapshot().locked`); await pause(60);
  const opposedStart = await snapshot();
  await mouse(point.x + 55, point.y + 28); await pause(80);
  const opposedEnd = await snapshot();
  assert.ok(distance(opposedStart.eye, opposedEnd.eye) < 1e-5, 'Mouse look rotates about the unchanged eye, not the orbit target');
  assert.ok(Math.abs(opposedStart.state.yaw - opposedEnd.state.yaw) > 0.01);
  for (const field of ['distance', 'fov', 'roll', 'horizontalFrameOffset', 'projectionScaleX']) assert.equal(opposedEnd.state[field], opposedStart.state[field]);
  assert.deepEqual(opposedEnd.state.projectionOffset, opposedStart.state.projectionOffset);
  assert.deepEqual(opposedEnd.projection, opposedStart.projection);
  await key('KeyW', false); await key('KeyS', false); await wait(`!${api}.snapshot().locked`);
  await save(activeCase, { opposedStart, opposedEnd });

  activeCase = 'escape-stops-and-unlocks';
  await fresh(); await key('KeyW'); await wait(`${api}.snapshot().locked`); await pause(60);
  await key('Escape'); await key('Escape', false); await wait(`!${api}.snapshot().locked`);
  const escaped = await snapshot(); await pause(120); assert.ok(distance(escaped.eye, (await snapshot()).eye) < 1e-7);
  await key('KeyW', false); await save(activeCase, { escaped });
  // Native Escape unlock has a browser-enforced reacquisition cooldown. Keep
  // independent later cases out of that interval; never grant/bypass permission.
  await pause(1500);

  activeCase = 'input-focus-orthographic-and-disabled-isolation';
  await evaluate(`${api}.reset();document.getElementById('outside-input').focus()`);
  const inputStart = await snapshot(); await key('KeyW', true, { text: 'w' }); await pause(70); await key('KeyW', false);
  assert.equal((await snapshot()).locked, false); assert.ok(distance(inputStart.eye, (await snapshot()).eye) < 1e-7);
  await fresh(); await evaluate(`${api}.setOrthographic(true)`);
  const orthoStart = await snapshot(); await key('KeyW'); await pause(70); await key('KeyW', false);
  assert.equal((await snapshot()).locked, false); assert.ok(distance(orthoStart.eye, (await snapshot()).eye) < 1e-7);
  await fresh(); await evaluate(`${api}.setRoamingEnabled(false)`);
  const disabledStart = await snapshot(); await key('KeyW'); await pause(70); await key('KeyW', false);
  assert.equal((await snapshot()).locked, false); assert.ok(distance(disabledStart.eye, (await snapshot()).eye) < 1e-7);
  await save(activeCase, { inputStart, orthoStart, disabledStart });

  activeCase = 'real-browser-denial-keeps-translation-and-drag';
  await evaluate(`${api}.reset();${api}.mountDeniedFrame()`);
  const denied = `document.getElementById('denied-frame').contentWindow.viewerMouseLookFixture`;
  await wait(`!!${denied}`);
  const offset = await evaluate(`(()=>{const r=document.getElementById('denied-frame').getBoundingClientRect();return {x:r.x,y:r.y};})()`);
  const deniedPoint = await focusCanvas(denied, offset), deniedStart = await snapshot(denied);
  await key('KeyW'); await pause(140);
  const deniedMoving = await snapshot(denied);
  assert.equal(deniedMoving.locked, false); assert.ok(distance(deniedStart.eye, deniedMoving.eye) > 0);
  assert.ok(deniedMoving.events.some(event => event.kind === 'pointerlockerror'), 'A sandbox without allow-pointer-lock must exercise an actual browser rejection');
  await mouse(deniedPoint.x + 35, deniedPoint.y + 18); await pause(50);
  const deniedMouse = await snapshot(denied); assert.equal(deniedMouse.state.yaw, deniedMoving.state.yaw); assert.equal(deniedMouse.state.pitch, deniedMoving.state.pitch);
  await key('KeyW', false); await pause(40);
  await mouse(deniedPoint.x, deniedPoint.y, 'mousePressed', { button: 'left', buttons: 1, clickCount: 1 });
  await mouse(deniedPoint.x + 35, deniedPoint.y + 18, 'mouseMoved', { button: 'left', buttons: 1 });
  await mouse(deniedPoint.x + 35, deniedPoint.y + 18, 'mouseReleased', { button: 'left', buttons: 0, clickCount: 1 });
  const deniedDragged = await snapshot(denied); assert.ok(Math.abs(deniedDragged.state.yaw - deniedMouse.state.yaw) > 0.01);
  assert.ok(!deniedDragged.events.some(event => event.kind === 'unhandledrejection'));
  await save(activeCase, { deniedStart, deniedMoving, deniedMouse, deniedDragged });
  await evaluate("document.getElementById('denied-frame').remove()");

  activeCase = 'destroy-releases-current-preview-lock';
  await fresh(); await key('KeyW'); await wait(`${api}.snapshot().locked`);
  await evaluate(`${api}.destroy()`); await wait(`!${api}.snapshot().locked`); await key('KeyW', false);
  const destroyed = await snapshot(); await pause(100); assert.ok(distance(destroyed.eye, (await snapshot()).eye) < 1e-7);
  await save(activeCase, { destroyed });
  const screenshot = await call('Page.captureScreenshot', { format: 'png' }); await writeFile(path.join(out, 'canvas.png'), Buffer.from(screenshot.data, 'base64'));
  assert.deepEqual(exceptions, [], 'No unhandled browser runtime exceptions');
  await writeFile(path.join(out, 'summary.md'), '# Viewer mouse-look browser verification\n\nPassed real Edge Pointer Lock, trusted keyboard/no-button mouse input, W/S line-of-sight ascent/descent and reverse movement, horizontal A/D with pitch and roll, last-key release, combined/opposed keys, fixed-eye reference pose, Esc, focus/orthographic/disabled isolation, sandbox denial fallback, and disposal.\n\nThis fixture uses the actual ViewerControls and browser Pointer Lock API with a schematic 2D canvas and camera node. It does not verify Gaussian rendering, native Tauri/WebView2, GPU performance, or OS cursor appearance.\n');
  console.log(`Viewer mouse-look browser verification passed: ${out}`);
} catch (error) {
  let state;
  try { state = await failureSnapshot?.(); } catch { /* Retain the original failure if the browser is gone. */ }
  await writeFile(path.join(out, 'failure.json'), JSON.stringify({ activeCase, error: String(error), state, consoleMessages, proofs, exceptions }, null, 2));
  throw error;
} finally {
  clearTimeout(startupTimer); socket?.close(); browser.kill(); server.closeAllConnections(); server.close();
  await writeFile(path.join(out, 'browser.log'), diagnostics); await writeFile(path.join(out, 'runtime-errors.json'), JSON.stringify(exceptions, null, 2));
}
