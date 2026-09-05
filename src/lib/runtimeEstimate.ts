import type { RuntimeEstimate } from "../types/pipeline";

export interface ProjectedRuntime {
  projectedTotalMs: number;
  remainingMs: number;
}

export function projectRuntime(
  estimate: RuntimeEstimate | null,
  progressPercent: number,
  elapsedMs: number,
  running: boolean,
): ProjectedRuntime | null {
  if (!estimate) return null;

  let projectedTotalMs = estimate.estimatedMs;
  const progressFraction = Math.min(1, Math.max(0, progressPercent / 100));
  if (running && elapsedMs >= 10_000 && progressFraction >= 0.05) {
    const observedTotal = elapsedMs / progressFraction;
    projectedTotalMs = Math.min(
      estimate.upperBoundMs * 1.25,
      Math.max(
        estimate.lowerBoundMs * 0.8,
        estimate.estimatedMs * 0.55 + observedTotal * 0.45,
      ),
    );
  }

  return {
    projectedTotalMs,
    remainingMs: Math.max(0, projectedTotalMs - elapsedMs),
  };
}
