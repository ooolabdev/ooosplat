import { OrbitCamera } from "./camera";
import type { ParsedSplatData } from "./plyParser";
import {
  gridFragmentShader,
  gridVertexShader,
  splatFragmentShader,
  splatVertexShader,
} from "./shaders";
import { DepthSorter } from "./sorter";

export type RenderMode = "splat" | "pointCloud";
export type BackgroundMode = "dark" | "studio" | "grid";

export class SplatRenderer {
  private canvas: HTMLCanvasElement;
  private gl: WebGL2RenderingContext;
  public camera: OrbitCamera;
  public data: ParsedSplatData;
  private sorter: DepthSorter;

  // WebGL Programs & Buffers
  private splatProgram!: WebGLProgram;
  private gridProgram!: WebGLProgram;
  private splatVAO!: WebGLVertexArrayObject;
  private gridVAO!: WebGLVertexArrayObject;
  private indexBuffer!: WebGLBuffer;

  // Data Textures
  private texCenters!: WebGLTexture;
  private texRotations!: WebGLTexture;
  private texScales!: WebGLTexture;
  private texColors!: WebGLTexture;
  private texWidth = 2048;
  private texHeight = 1;

  // Settings
  public renderMode: RenderMode = "splat";
  public backgroundMode: BackgroundMode = "grid";
  public splatScale = 1.0;

  // Performance stats
  public fps = 60;
  private frameCount = 0;
  private lastFpsUpdate = performance.now();
  private isDestroyed = false;
  private animFrameId: number | null = null;
  private lastSortedViewMatrix = new Float32Array(16);

  constructor(canvas: HTMLCanvasElement, data: ParsedSplatData) {
    this.canvas = canvas;
    this.data = data;
    this.sorter = new DepthSorter(data.count, data.positions);

    const gl = canvas.getContext("webgl2", {
      alpha: true,
      antialias: false,
      depth: false,
      stencil: false,
      premultipliedAlpha: true,
      preserveDrawingBuffer: true,
      powerPreference: "high-performance",
    });

    if (!gl) {
      throw new Error("您的设备不支持 WebGL2，无法开启 3D 预览");
    }

    // Enable floating point textures
    const extColorBufferFloat = gl.getExtension("EXT_color_buffer_float");
    if (!extColorBufferFloat) {
      console.warn("EXT_color_buffer_float not supported");
    }

    this.gl = gl;
    this.camera = new OrbitCamera(data.bounds);
    this.initPrograms();
    this.initTextures();
    this.initBuffers();
    this.startLoop();
  }

  private createShader(type: number, source: string): WebGLShader {
    const gl = this.gl;
    const shader = gl.createShader(type)!;
    gl.shaderSource(shader, source);
    gl.compileShader(shader);

    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      const info = gl.getShaderInfoLog(shader);
      gl.deleteShader(shader);
      throw new Error(`着色器编译失败: ${info}`);
    }
    return shader;
  }

  private createProgram(vsSource: string, fsSource: string): WebGLProgram {
    const gl = this.gl;
    const vs = this.createShader(gl.VERTEX_SHADER, vsSource);
    const fs = this.createShader(gl.FRAGMENT_SHADER, fsSource);
    const program = gl.createProgram()!;
    gl.attachShader(program, vs);
    gl.attachShader(program, fs);
    gl.linkProgram(program);

    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      const info = gl.getProgramInfoLog(program);
      throw new Error(`程序链接失败: ${info}`);
    }
    return program;
  }

  private initPrograms() {
    this.splatProgram = this.createProgram(splatVertexShader, splatFragmentShader);
    this.gridProgram = this.createProgram(gridVertexShader, gridFragmentShader);
  }

  private initTextures() {
    const gl = this.gl;
    const count = this.data.count;
    this.texWidth = 2048;
    this.texHeight = Math.ceil(count / this.texWidth);
    const texPixels = this.texWidth * this.texHeight;

    // 1. Centers + Opacity (RGBA32F)
    const centerData = new Float32Array(texPixels * 4);
    for (let i = 0; i < count; i++) {
      centerData[i * 4 + 0] = this.data.positions[i * 3 + 0];
      centerData[i * 4 + 1] = this.data.positions[i * 3 + 1];
      centerData[i * 4 + 2] = this.data.positions[i * 3 + 2];
      centerData[i * 4 + 3] = this.data.colors[i * 4 + 3]; // Opacity in w
    }
    this.texCenters = this.createDataTexture(centerData);

    // 2. Rotations (RGBA32F: qw, qx, qy, qz)
    const rotData = new Float32Array(texPixels * 4);
    rotData.set(this.data.rotations.subarray(0, count * 4));
    this.texRotations = this.createDataTexture(rotData);

    // 3. Scales (RGBA32F: sx, sy, sz, 1.0)
    const scaleData = new Float32Array(texPixels * 4);
    for (let i = 0; i < count; i++) {
      scaleData[i * 4 + 0] = this.data.scales[i * 3 + 0];
      scaleData[i * 4 + 1] = this.data.scales[i * 3 + 1];
      scaleData[i * 4 + 2] = this.data.scales[i * 3 + 2];
      scaleData[i * 4 + 3] = 1.0;
    }
    this.texScales = this.createDataTexture(scaleData);

    // 4. Colors (RGBA32F: r, g, b, 1.0)
    const colorData = new Float32Array(texPixels * 4);
    for (let i = 0; i < count; i++) {
      colorData[i * 4 + 0] = this.data.colors[i * 4 + 0];
      colorData[i * 4 + 1] = this.data.colors[i * 4 + 1];
      colorData[i * 4 + 2] = this.data.colors[i * 4 + 2];
      colorData[i * 4 + 3] = 1.0;
    }
    this.texColors = this.createDataTexture(colorData);
  }

  private createDataTexture(data: Float32Array): WebGLTexture {
    const gl = this.gl;
    const tex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA32F,
      this.texWidth,
      this.texHeight,
      0,
      gl.RGBA,
      gl.FLOAT,
      data
    );
    gl.bindTexture(gl.TEXTURE_2D, null);
    return tex;
  }

  private initBuffers() {
    const gl = this.gl;

    // 1. Splat VAO
    this.splatVAO = gl.createVertexArray()!;
    gl.bindVertexArray(this.splatVAO);

    // Quad geometry [-1, 1]
    const quadVertices = new Float32Array([
      -1, -1,
       1, -1,
      -1,  1,
       1,  1,
    ]);
    const quadBuffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, quadBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, quadVertices, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

    // Instanced index buffer (stores sorted splat indices)
    this.indexBuffer = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.indexBuffer);
    const initialIndices = new Uint32Array(this.data.count);
    for (let i = 0; i < this.data.count; i++) initialIndices[i] = i;
    gl.bufferData(gl.ARRAY_BUFFER, initialIndices, gl.DYNAMIC_DRAW);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribIPointer(1, 1, gl.UNSIGNED_INT, 0, 0);
    gl.vertexAttribDivisor(1, 1);

    // 2. Grid VAO
    this.gridVAO = gl.createVertexArray()!;
    gl.bindVertexArray(this.gridVAO);

    const gridLines = [];
    const steps = 20;
    for (let i = -steps; i <= steps; i++) {
      const p = i / steps;
      gridLines.push(-1, 0, p,  1, 0, p);
      gridLines.push(p, 0, -1,  p, 0, 1);
    }
    const gridBuffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, gridBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(gridLines), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0);

    gl.bindVertexArray(null);
  }

  public render() {
    if (this.isDestroyed) return;
    const gl = this.gl;

    // Viewport scaling
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const displayWidth = Math.floor(this.canvas.clientWidth * dpr);
    const displayHeight = Math.floor(this.canvas.clientHeight * dpr);

    if (this.canvas.width !== displayWidth || this.canvas.height !== displayHeight) {
      this.canvas.width = displayWidth;
      this.canvas.height = displayHeight;
    }

    gl.viewport(0, 0, gl.canvas.width, gl.canvas.height);
    this.camera.aspect = gl.canvas.width / (gl.canvas.height || 1);
    const cameraChanged = this.camera.update();

    const viewMatrix = this.camera.getViewMatrix();
    const projMatrix = this.camera.getProjectionMatrix();

    // Fast back-to-front depth sorting on camera change
    if (cameraChanged || this.frameCount === 1) {
      const sorted = this.sorter.sort(viewMatrix);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.indexBuffer);
      gl.bufferSubData(gl.ARRAY_BUFFER, 0, sorted);
      this.lastSortedViewMatrix.set(viewMatrix);
    }

    // Clear background
    if (this.backgroundMode === "studio") {
      gl.clearColor(0.12, 0.14, 0.18, 1.0);
    } else if (this.backgroundMode === "dark") {
      gl.clearColor(0.06, 0.07, 0.09, 1.0);
    } else {
      gl.clearColor(0.08, 0.09, 0.12, 1.0);
    }
    gl.clear(gl.COLOR_BUFFER_BIT);

    // 1. Draw Grid Floor
    if (this.backgroundMode === "grid") {
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);

      gl.useProgram(this.gridProgram);
      gl.uniformMatrix4fv(gl.getUniformLocation(this.gridProgram, "u_view"), false, viewMatrix);
      gl.uniformMatrix4fv(gl.getUniformLocation(this.gridProgram, "u_projection"), false, projMatrix);
      gl.uniform1f(gl.getUniformLocation(this.gridProgram, "u_gridSize"), Math.max(5.0, this.data.bounds.radius * 2.0));

      gl.bindVertexArray(this.gridVAO);
      gl.drawArrays(gl.LINES, 0, 84);
    }

    // 2. Draw Splats with Premultiplied Alpha
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);

    gl.useProgram(this.splatProgram);
    gl.uniformMatrix4fv(gl.getUniformLocation(this.splatProgram, "u_view"), false, viewMatrix);
    gl.uniformMatrix4fv(gl.getUniformLocation(this.splatProgram, "u_projection"), false, projMatrix);

    const fovy = this.camera.fov * (Math.PI / 180);
    const fy = gl.canvas.height / (2 * Math.tan(fovy / 2));
    const fx = fy;
    gl.uniform2f(gl.getUniformLocation(this.splatProgram, "u_focal"), fx, fy);
    gl.uniform2f(gl.getUniformLocation(this.splatProgram, "u_viewport"), gl.canvas.width, gl.canvas.height);
    gl.uniform1f(gl.getUniformLocation(this.splatProgram, "u_splatScale"), this.splatScale);
    gl.uniform1i(gl.getUniformLocation(this.splatProgram, "u_renderMode"), this.renderMode === "pointCloud" ? 1 : 0);
    gl.uniform1i(gl.getUniformLocation(this.splatProgram, "u_texWidth"), this.texWidth);

    // Bind Data Textures
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.texCenters);
    gl.uniform1i(gl.getUniformLocation(this.splatProgram, "u_texCenters"), 0);

    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.texRotations);
    gl.uniform1i(gl.getUniformLocation(this.splatProgram, "u_texRotations"), 1);

    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, this.texScales);
    gl.uniform1i(gl.getUniformLocation(this.splatProgram, "u_texScales"), 2);

    gl.activeTexture(gl.TEXTURE3);
    gl.bindTexture(gl.TEXTURE_2D, this.texColors);
    gl.uniform1i(gl.getUniformLocation(this.splatProgram, "u_texColors"), 3);

    gl.bindVertexArray(this.splatVAO);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.data.count);

    // FPS calculation
    this.frameCount++;
    const now = performance.now();
    if (now - this.lastFpsUpdate >= 500) {
      this.fps = Math.round((this.frameCount * 1000) / (now - this.lastFpsUpdate));
      this.frameCount = 0;
      this.lastFpsUpdate = now;
    }
  }

  private startLoop() {
    const loop = () => {
      if (this.isDestroyed) return;
      this.render();
      this.animFrameId = requestAnimationFrame(loop);
    };
    this.animFrameId = requestAnimationFrame(loop);
  }

  public pickPoint(screenX: number, screenY: number): [number, number, number] | null {
    const gl = this.gl;
    const w = gl.canvas.width;
    const h = gl.canvas.height;
    if (w === 0 || h === 0) return null;

    // Convert screen coordinates to NDC [-1, 1]
    const ndcX = (2.0 * screenX) / this.canvas.clientWidth - 1.0;
    const ndcY = 1.0 - (2.0 * screenY) / this.canvas.clientHeight;

    const proj = this.camera.getProjectionMatrix();
    const view = this.camera.getViewMatrix();
    const eye = this.camera.getPosition();

    // Camera space ray direction
    const rayDirCam = [
      ndcX / proj[0],
      ndcY / proj[5],
      -1.0,
    ];
    const lenCam = Math.hypot(rayDirCam[0], rayDirCam[1], rayDirCam[2]) || 1.0;
    const normRayCam = [rayDirCam[0] / lenCam, rayDirCam[1] / lenCam, rayDirCam[2] / lenCam];

    // World space ray direction: transpose of 3x3 view matrix
    const rayDir = [
      view[0] * normRayCam[0] + view[1] * normRayCam[1] + view[2] * normRayCam[2],
      view[4] * normRayCam[0] + view[5] * normRayCam[1] + view[6] * normRayCam[2],
      view[8] * normRayCam[0] + view[9] * normRayCam[1] + view[10] * normRayCam[2],
    ];
    const lenWorld = Math.hypot(rayDir[0], rayDir[1], rayDir[2]) || 1.0;
    rayDir[0] /= lenWorld;
    rayDir[1] /= lenWorld;
    rayDir[2] /= lenWorld;

    const N = this.data.count;
    const pos = this.data.positions;
    const colors = this.data.colors;

    let closestDist = Infinity;
    let bestPoint: [number, number, number] | null = null;
    const maxThreshold = Math.max(0.1, this.data.bounds.radius * 0.05);

    for (let i = 0; i < N; i++) {
      if (colors[i * 4 + 3] < 0.1) continue;

      const idx = i * 3;
      const px = pos[idx];
      const py = pos[idx + 1];
      const pz = pos[idx + 2];

      const vx = px - eye[0];
      const vy = py - eye[1];
      const vz = pz - eye[2];

      const t = vx * rayDir[0] + vy * rayDir[1] + vz * rayDir[2];
      if (t <= 0.05 || t > this.camera.far) continue;

      const vLenSq = vx * vx + vy * vy + vz * vz;
      const perpDistSq = vLenSq - t * t;
      if (perpDistSq < 0) continue;

      const threshold = maxThreshold * (t / this.camera.distance);
      if (perpDistSq < threshold * threshold) {
        if (t < closestDist) {
          closestDist = t;
          bestPoint = [px, py, pz];
        }
      }
    }

    return bestPoint;
  }

  public captureScreenshot(): string {
    this.render();
    return this.canvas.toDataURL("image/png");
  }

  public destroy() {
    this.isDestroyed = true;
    if (this.animFrameId) {
      cancelAnimationFrame(this.animFrameId);
    }
  }
}
