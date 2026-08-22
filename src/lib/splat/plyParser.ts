export interface ParsedSplatData {
  count: number;
  positions: Float32Array; // N * 3
  rotations: Float32Array; // N * 4 (qw, qx, qy, qz)
  scales: Float32Array;    // N * 3
  colors: Float32Array;    // N * 4 (RGBA, 0.0 - 1.0)
  bounds: {
    min: [number, number, number];
    max: [number, number, number];
    center: [number, number, number];
    centroid: [number, number, number];
    radius: number;
  };
}

const SH_C0 = 0.28209479177387814;

function sigmoid(x: number): number {
  return 1 / (1 + Math.exp(-x));
}

export function parsePlyBuffer(buffer: ArrayBuffer | Uint8Array): ParsedSplatData {
  const bytes = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
  const dataView = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);

  // 1. Read ASCII Header
  let headerText = "";
  let headerEnd = -1;
  const maxHeaderLen = Math.min(bytes.length, 10000);

  for (let i = 0; i < maxHeaderLen; i++) {
    headerText += String.fromCharCode(bytes[i]);
    if (headerText.endsWith("end_header\n") || headerText.endsWith("end_header\r\n")) {
      headerEnd = i + 1;
      break;
    }
  }

  if (headerEnd === -1) {
    throw new Error("无效的 PLY 文件：未找到 end_header 标头");
  }

  const lines = headerText.split(/\r?\n/);
  let vertexCount = 0;
  const properties: Array<{ name: string; type: string; size: number }> = [];
  let isVertexElement = false;

  for (const line of lines) {
    const tokens = line.trim().split(/\s+/);
    if (tokens[0] === "element" && tokens[1] === "vertex") {
      vertexCount = parseInt(tokens[2], 10);
      isVertexElement = true;
    } else if (tokens[0] === "element" && tokens[1] !== "vertex") {
      isVertexElement = false;
    } else if (tokens[0] === "property" && isVertexElement) {
      const type = tokens[1];
      const name = tokens[2];
      const size = type === "float" || type === "float32" || type === "int" || type === "uint" ? 4 : type === "double" || type === "float64" ? 8 : type === "short" || type === "ushort" ? 2 : 1;
      properties.push({ name, type, size });
    }
  }

  if (vertexCount <= 0) {
    throw new Error("PLY 文件中未包含有效的点云数据 (vertex count = 0)");
  }

  // Calculate stride and property offsets
  let stride = 0;
  const propOffsets = new Map<string, number>();
  for (const prop of properties) {
    propOffsets.set(prop.name, stride);
    stride += prop.size;
  }

  const posXOff = propOffsets.get("x") ?? 0;
  const posYOff = propOffsets.get("y") ?? 4;
  const posZOff = propOffsets.get("z") ?? 8;

  const fdc0Off = propOffsets.get("f_dc_0");
  const fdc1Off = propOffsets.get("f_dc_1");
  const fdc2Off = propOffsets.get("f_dc_2");

  const redOff = propOffsets.get("red");
  const greenOff = propOffsets.get("green");
  const blueOff = propOffsets.get("blue");

  const opacityOff = propOffsets.get("opacity");

  const scale0Off = propOffsets.get("scale_0");
  const scale1Off = propOffsets.get("scale_1");
  const scale2Off = propOffsets.get("scale_2");

  const rot0Off = propOffsets.get("rot_0");
  const rot1Off = propOffsets.get("rot_1");
  const rot2Off = propOffsets.get("rot_2");
  const rot3Off = propOffsets.get("rot_3");

  const positions = new Float32Array(vertexCount * 3);
  const rotations = new Float32Array(vertexCount * 4);
  const scales = new Float32Array(vertexCount * 3);
  const colors = new Float32Array(vertexCount * 4);

  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;

  let offset = headerEnd;

  for (let i = 0; i < vertexCount; i++) {
    const pointOffset = offset + i * stride;

    // Position (Convert COLMAP +Y down, +Z fwd to WebGL +Y up, +Z back)
    const px = dataView.getFloat32(pointOffset + posXOff, true);
    const py = -dataView.getFloat32(pointOffset + posYOff, true);
    const pz = -dataView.getFloat32(pointOffset + posZOff, true);

    positions[i * 3 + 0] = px;
    positions[i * 3 + 1] = py;
    positions[i * 3 + 2] = pz;

    if (px < minX) minX = px; if (px > maxX) maxX = px;
    if (py < minY) minY = py; if (py > maxY) maxY = py;
    if (pz < minZ) minZ = pz; if (pz > maxZ) maxZ = pz;

    // Color
    let r = 1, g = 1, b = 1;
    if (fdc0Off !== undefined && fdc1Off !== undefined && fdc2Off !== undefined) {
      const fdc0 = dataView.getFloat32(pointOffset + fdc0Off, true);
      const fdc1 = dataView.getFloat32(pointOffset + fdc1Off, true);
      const fdc2 = dataView.getFloat32(pointOffset + fdc2Off, true);
      r = Math.min(1, Math.max(0, 0.5 + SH_C0 * fdc0));
      g = Math.min(1, Math.max(0, 0.5 + SH_C0 * fdc1));
      b = Math.min(1, Math.max(0, 0.5 + SH_C0 * fdc2));
    } else if (redOff !== undefined && greenOff !== undefined && blueOff !== undefined) {
      r = dataView.getUint8(pointOffset + redOff) / 255;
      g = dataView.getUint8(pointOffset + greenOff) / 255;
      b = dataView.getUint8(pointOffset + blueOff) / 255;
    }

    // Opacity
    let alpha = 1.0;
    if (opacityOff !== undefined) {
      const rawOpacity = dataView.getFloat32(pointOffset + opacityOff, true);
      alpha = sigmoid(rawOpacity);
    }
    colors[i * 4 + 0] = r;
    colors[i * 4 + 1] = g;
    colors[i * 4 + 2] = b;
    colors[i * 4 + 3] = alpha;

    // Scales
    if (scale0Off !== undefined && scale1Off !== undefined && scale2Off !== undefined) {
      scales[i * 3 + 0] = Math.exp(dataView.getFloat32(pointOffset + scale0Off, true));
      scales[i * 3 + 1] = Math.exp(dataView.getFloat32(pointOffset + scale1Off, true));
      scales[i * 3 + 2] = Math.exp(dataView.getFloat32(pointOffset + scale2Off, true));
    } else {
      scales[i * 3 + 0] = 0.01;
      scales[i * 3 + 1] = 0.01;
      scales[i * 3 + 2] = 0.01;
    }

    // Rotations (Quaternion: qw, qx, qy, qz)
    if (rot0Off !== undefined && rot1Off !== undefined && rot2Off !== undefined && rot3Off !== undefined) {
      const q0 = dataView.getFloat32(pointOffset + rot0Off, true);
      const q1 = dataView.getFloat32(pointOffset + rot1Off, true);
      const q2 = -dataView.getFloat32(pointOffset + rot2Off, true);
      const q3 = -dataView.getFloat32(pointOffset + rot3Off, true);
      const len = Math.sqrt(q0 * q0 + q1 * q1 + q2 * q2 + q3 * q3) || 1.0;
      rotations[i * 4 + 0] = q0 / len;
      rotations[i * 4 + 1] = q1 / len;
      rotations[i * 4 + 2] = q2 / len;
      rotations[i * 4 + 3] = q3 / len;
    } else {
      rotations[i * 4 + 0] = 1.0;
      rotations[i * 4 + 1] = 0.0;
      rotations[i * 4 + 2] = 0.0;
      rotations[i * 4 + 3] = 0.0;
    }
  }

  let sumX = 0, sumY = 0, sumZ = 0;
  for (let i = 0; i < vertexCount; i++) {
    sumX += positions[i * 3 + 0];
    sumY += positions[i * 3 + 1];
    sumZ += positions[i * 3 + 2];
  }
  const centroidX = sumX / vertexCount;
  const centroidY = sumY / vertexCount;
  const centroidZ = sumZ / vertexCount;

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;
  const dx = maxX - minX;
  const dy = maxY - minY;
  const dz = maxZ - minZ;
  const radius = Math.sqrt(dx * dx + dy * dy + dz * dz) / 2 || 1.0;

  return {
    count: vertexCount,
    positions,
    rotations,
    scales,
    colors,
    bounds: {
      min: [minX, minY, minZ],
      max: [maxX, maxY, maxZ],
      center: [cx, cy, cz],
      centroid: [centroidX, centroidY, centroidZ],
      radius,
    },
  };
}
