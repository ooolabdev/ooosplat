// High-Performance 16-bit Counting Depth Sorter for 3D Gaussian Splatting
// Operates in O(N) linear time (~2-4ms for 1,000,000 points)

export class DepthSorter {
  private count: number;
  private positions: Float32Array;
  private depths: Float32Array;
  private sortedIndices: Uint32Array;
  private counts: Uint32Array;

  constructor(count: number, positions: Float32Array) {
    this.count = count;
    this.positions = positions;
    this.depths = new Float32Array(count);
    this.sortedIndices = new Uint32Array(count);
    this.counts = new Uint32Array(65536);

    for (let i = 0; i < count; i++) {
      this.sortedIndices[i] = i;
    }
  }

  public sort(viewMatrix: Float32Array): Uint32Array {
    const N = this.count;
    const pos = this.positions;
    const depths = this.depths;

    // View matrix row 2 (z axis direction in view space)
    // viewMatrix is column-major in WebGL:
    // element (row 2, col 0) = viewMatrix[2]
    // element (row 2, col 1) = viewMatrix[6]
    // element (row 2, col 2) = viewMatrix[10]
    // element (row 2, col 3) = viewMatrix[14]
    const r20 = viewMatrix[2];
    const r21 = viewMatrix[6];
    const r22 = viewMatrix[10];
    const r23 = viewMatrix[14];

    let minDepth = Infinity;
    let maxDepth = -Infinity;

    // 1. Calculate view-space z for every point
    for (let i = 0; i < N; i++) {
      const idx = i * 3;
      const z = r20 * pos[idx] + r21 * pos[idx + 1] + r22 * pos[idx + 2] + r23;
      depths[i] = z;
      if (z < minDepth) minDepth = z;
      if (z > maxDepth) maxDepth = z;
    }

    const range = maxDepth - minDepth || 1.0;
    const invRange = 65535 / range;
    const counts = this.counts;
    counts.fill(0);

    // 2. Histogram counts (0 to 65535)
    for (let i = 0; i < N; i++) {
      const bucket = Math.min(65535, Math.max(0, Math.floor((depths[i] - minDepth) * invRange)));
      counts[bucket]++;
    }

    // 3. Cumulative sum for back-to-front order (farthest z first)
    let sum = 0;
    for (let i = 0; i < 65536; i++) {
      const c = counts[i];
      counts[i] = sum;
      sum += c;
    }

    // 4. Scatter into sortedIndices
    const sorted = this.sortedIndices;
    for (let i = 0; i < N; i++) {
      const bucket = Math.min(65535, Math.max(0, Math.floor((depths[i] - minDepth) * invRange)));
      sorted[counts[bucket]++] = i;
    }

    return sorted;
  }
}
