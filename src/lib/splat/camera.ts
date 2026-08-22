export interface KeyMovement {
  forward: boolean;
  backward: boolean;
  left: boolean;
  right: boolean;
  up: boolean;
  down: boolean;
  boost: boolean;
}

export class OrbitCamera {
  // Common projection parameters
  public fov = 50;
  public aspect = 1.0;
  public near = 0.01;
  public far = 1000.0;

  // 6-DoF Free Fly Navigation State
  public pos: [number, number, number] = [0, 1, 3];
  public yaw = 0;   // Horizontal angle (radians)
  public pitch = 0; // Vertical pitch angle (radians, [-1.53, 1.53])

  // Smooth Interpolation Targets
  public targetYaw = 0;
  public targetPitch = 0;
  public targetPos: [number, number, number] = [0, 1, 3];

  // Target Focus / Pivot Point in 3D space
  public focusTarget: [number, number, number] = [0, 0, 0];

  // Physics Velocity & Momentum
  public velocity: [number, number, number] = [0, 0, 0];
  public speedMultiplier = 1.0;
  public baseSpeed = 0.06;

  public keyMovement: KeyMovement = {
    forward: false,
    backward: false,
    left: false,
    right: false,
    up: false,
    down: false,
    boost: false,
  };

  // Backwards compatibility properties
  public get distance(): number {
    return Math.hypot(this.pos[0] - this.focusTarget[0], this.pos[1] - this.focusTarget[1], this.pos[2] - this.focusTarget[2]) || 1.0;
  }
  public get dronePos(): [number, number, number] { return this.pos; }
  public set dronePos(v: [number, number, number]) { this.pos = v; }
  public get droneYaw(): number { return this.yaw; }
  public set droneYaw(v: number) { this.yaw = v; this.targetYaw = v; }
  public get dronePitch(): number { return this.pitch; }
  public set dronePitch(v: number) { this.pitch = v; this.targetPitch = v; }
  public get droneSpeed(): number { return this.speedMultiplier; }
  public set droneSpeed(v: number) { this.speedMultiplier = v; }
  public autoRotate = false;
  public mode: "drone" = "drone";

  constructor(initialBounds?: { center: [number, number, number]; radius: number }) {
    if (initialBounds) {
      this.resetToBounds(initialBounds);
    }
  }

  public resetToBounds(bounds: { center: [number, number, number]; radius: number }) {
    this.focusTarget = [...bounds.center];
    const dist = Math.max(1.0, bounds.radius * 2.2);

    // Initial position: Elevated and viewing center
    this.pos = [
      bounds.center[0] + dist * 0.65,
      bounds.center[1] + dist * 0.45,
      bounds.center[2] + dist * 0.85,
    ];
    this.targetPos = [...this.pos];

    this.near = Math.max(0.001, bounds.radius * 0.01);
    this.far = bounds.radius * 30.0;
    this.baseSpeed = Math.max(0.015, bounds.radius * 0.025);

    this.lookAtPoint(bounds.center[0], bounds.center[1], bounds.center[2]);
  }

  public lookAtPoint(tx: number, ty: number, tz: number) {
    this.focusTarget = [tx, ty, tz];
    const dx = tx - this.pos[0];
    const dy = ty - this.pos[1];
    const dz = tz - this.pos[2];
    const len = Math.hypot(dx, dy, dz) || 1.0;

    this.yaw = Math.atan2(dx, -dz);
    this.targetYaw = this.yaw;
    this.pitch = Math.asin(Math.max(-0.99, Math.min(0.99, dy / len)));
    this.targetPitch = this.pitch;
  }

  public setViewPreset(preset: "front" | "top" | "side" | "iso" | "reset", bounds?: { center: [number, number, number]; radius: number }) {
    const center = bounds ? bounds.center : this.focusTarget;
    const r = bounds ? Math.max(1.0, bounds.radius * 2.2) : 3.0;

    switch (preset) {
      case "front":
        this.pos = [center[0], center[1], center[2] + r];
        break;
      case "top":
        this.pos = [center[0], center[1] + r, center[2] + 0.001];
        break;
      case "side":
        this.pos = [center[0] + r, center[1], center[2]];
        break;
      case "iso":
        this.pos = [center[0] + r * 0.7, center[1] + r * 0.5, center[2] + r * 0.7];
        break;
      case "reset":
        this.pos = [center[0] + r * 0.65, center[1] + r * 0.45, center[2] + r * 0.85];
        break;
    }
    this.targetPos = [...this.pos];
    this.velocity = [0, 0, 0];
    this.lookAtPoint(center[0], center[1], center[2]);
  }

  // --- Look & Orientation ---
  public lookAround(dYaw: number, dPitch: number) {
    // Mouse right -> Yaw increases (looks right), Mouse left -> Yaw decreases (looks left)
    this.targetYaw += dYaw;
    this.targetPitch = Math.max(-1.53, Math.min(1.53, this.targetPitch - dPitch));
  }

  // --- Pan / Translate (2-Finger / Shift-drag) ---
  public pan(dx: number, dy: number) {
    const right = this.getRightVector();
    const speed = this.baseSpeed * this.speedMultiplier * 0.5;
    this.pos[0] -= right[0] * dx * speed;
    this.pos[2] -= right[2] * dx * speed;
    this.pos[1] += dy * speed;
    this.targetPos = [...this.pos];
  }

  // --- Mouse Wheel (Speed Modulation) ---
  public zoom(delta: number) {
    const factor = delta < 0 ? 1.15 : 0.87;
    this.speedMultiplier = Math.max(0.1, Math.min(10.0, this.speedMultiplier * factor));
  }

  // --- Vector Calculations ---
  public getForwardVector(): [number, number, number] {
    const cp = Math.cos(this.pitch);
    const sp = Math.sin(this.pitch);
    const cy = Math.cos(this.yaw);
    const sy = Math.sin(this.yaw);

    return [sy * cp, sp, -cy * cp];
  }

  public getRightVector(): [number, number, number] {
    const cy = Math.cos(this.yaw);
    const sy = Math.sin(this.yaw);
    return [cy, 0, sy];
  }

  public getTargetPoint(): [number, number, number] {
    const fwd = this.getForwardVector();
    return [
      this.pos[0] + fwd[0],
      this.pos[1] + fwd[1],
      this.pos[2] + fwd[2],
    ];
  }

  public getDroneForwardVector(): [number, number, number] {
    return this.getForwardVector();
  }

  // --- Per-frame Physics Update ---
  public update(): boolean {
    let changed = false;

    // 1. Smooth look angle interpolation
    const lookDamp = 0.35;
    const dYaw = (this.targetYaw - this.yaw) * lookDamp;
    const dPitch = (this.targetPitch - this.pitch) * lookDamp;
    if (Math.abs(dYaw) > 0.0001 || Math.abs(dPitch) > 0.0001) {
      this.yaw += dYaw;
      this.pitch += dPitch;
      changed = true;
    }

    // 2. Compute Desired Flight Direction from Keyboard
    let fwdInput = 0;
    let rightInput = 0;
    let upInput = 0;

    if (this.keyMovement.forward) fwdInput += 1;
    if (this.keyMovement.backward) fwdInput -= 1;
    if (this.keyMovement.right) rightInput += 1;
    if (this.keyMovement.left) rightInput -= 1;
    if (this.keyMovement.up) upInput += 1;
    if (this.keyMovement.down) upInput -= 1;

    // Normalize diagonal movement
    const inputLen = Math.hypot(fwdInput, rightInput, upInput);
    if (inputLen > 1.0) {
      fwdInput /= inputLen;
      rightInput /= inputLen;
      upInput /= inputLen;
    }

    const fwd = this.getForwardVector();
    const right = this.getRightVector();
    const currentSpeed = this.baseSpeed * this.speedMultiplier * (this.keyMovement.boost ? 2.5 : 1.0);

    const targetVx = (fwd[0] * fwdInput + right[0] * rightInput) * currentSpeed;
    const targetVy = (fwd[1] * fwdInput + upInput) * currentSpeed;
    const targetVz = (fwd[2] * fwdInput + right[2] * rightInput) * currentSpeed;

    // Newtonian acceleration & smooth deceleration
    const accel = 0.22;
    this.velocity[0] += (targetVx - this.velocity[0]) * accel;
    this.velocity[1] += (targetVy - this.velocity[1]) * accel;
    this.velocity[2] += (targetVz - this.velocity[2]) * accel;

    this.pos[0] += this.velocity[0];
    this.pos[1] += this.velocity[1];
    this.pos[2] += this.velocity[2];

    if (Math.hypot(this.velocity[0], this.velocity[1], this.velocity[2]) > 0.0001) {
      changed = true;
    }

    return changed;
  }

  // --- Pivot & Target Coordination ---
  public setPivot(x: number, y: number, z: number) {
    this.lookAtPoint(x, y, z);
  }

  public getPivot(): [number, number, number] {
    return [...this.focusTarget];
  }

  public getPosition(): [number, number, number] {
    return [...this.pos];
  }

  public setMode(_mode: any) {
    // Single dedicated free fly mode
  }

  public rotate(dTheta: number, dPhi: number) {
    this.lookAround(dTheta, dPhi);
  }

  public flyMove(fwd: number, right: number, up: number, isBoost = false) {
    const f = this.getForwardVector();
    const r = this.getRightVector();
    const speed = this.baseSpeed * this.speedMultiplier * (isBoost ? 2.5 : 1.0);

    this.velocity[0] += (f[0] * fwd + r[0] * right) * speed;
    this.velocity[1] += (f[1] * fwd + up) * speed;
    this.velocity[2] += (f[2] * fwd + r[2] * right) * speed;
  }

  // --- Screen Space Projection ---
  public projectToScreen(point: [number, number, number], width: number, height: number): [number, number] | null {
    const view = this.getViewMatrix();
    const proj = this.getProjectionMatrix();

    const vx = view[0] * point[0] + view[4] * point[1] + view[8] * point[2] + view[12];
    const vy = view[1] * point[0] + view[5] * point[1] + view[9] * point[2] + view[13];
    const vz = view[2] * point[0] + view[6] * point[1] + view[10] * point[2] + view[14];
    const vw = view[3] * point[0] + view[7] * point[1] + view[11] * point[2] + view[15];

    if (vw <= 0.001) return null;

    const ndcX = vx / vw;
    const ndcY = vy / vw;

    const screenX = ((ndcX + 1.0) / 2.0) * width;
    const screenY = ((1.0 - ndcY) / 2.0) * height;

    return [screenX, screenY];
  }

  public getViewMatrix(): Float32Array {
    return lookAt(this.pos, this.getTargetPoint(), [0, 1, 0]);
  }

  public getProjectionMatrix(): Float32Array {
    return perspective(this.fov * (Math.PI / 180), this.aspect, this.near, this.far);
  }
}

// Standard Matrix Math Utilities
function lookAt(eye: [number, number, number], target: [number, number, number], up: [number, number, number]): Float32Array {
  const z0 = eye[0] - target[0];
  const z1 = eye[1] - target[1];
  const z2 = eye[2] - target[2];
  let len = Math.sqrt(z0 * z0 + z1 * z1 + z2 * z2) || 1;
  const fz = [z0 / len, z1 / len, z2 / len];

  const x0 = up[1] * fz[2] - up[2] * fz[1];
  const x1 = up[2] * fz[0] - up[0] * fz[2];
  const x2 = up[0] * fz[1] - up[1] * fz[0];
  len = Math.sqrt(x0 * x0 + x1 * x1 + x2 * x2) || 1;
  const fx = [x0 / len, x1 / len, x2 / len];

  const fy = [
    fz[1] * fx[2] - fz[2] * fx[1],
    fz[2] * fx[0] - fz[0] * fx[2],
    fz[0] * fx[1] - fz[1] * fx[0],
  ];

  const out = new Float32Array(16);
  out[0] = fx[0]; out[4] = fx[1]; out[8] = fx[2]; out[12] = -(fx[0] * eye[0] + fx[1] * eye[1] + fx[2] * eye[2]);
  out[1] = fy[0]; out[5] = fy[1]; out[9] = fy[2]; out[13] = -(fy[0] * eye[0] + fy[1] * eye[1] + fy[2] * eye[2]);
  out[2] = fz[0]; out[6] = fz[1]; out[10] = fz[2]; out[14] = -(fz[0] * eye[0] + fz[1] * eye[1] + fz[2] * eye[2]);
  out[3] = 0;     out[7] = 0;     out[11] = 0;     out[15] = 1;
  return out;
}

function perspective(fovy: number, aspect: number, near: number, far: number): Float32Array {
  const f = 1.0 / Math.tan(fovy / 2);
  const out = new Float32Array(16);
  out[0] = f / aspect;
  out[5] = f;
  out[10] = (far + near) / (near - far);
  out[11] = -1;
  out[14] = (2 * far * near) / (near - far);
  return out;
}
