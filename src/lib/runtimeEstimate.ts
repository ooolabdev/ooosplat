import type { RuntimeEstimate, RuntimeSnapshot } from "../types/pipeline";

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

export function runtimeOutputAgeMs(snapshot: RuntimeSnapshot, running: boolean, now: number): number {
  const updatedAt = Date.parse(snapshot.updatedAt);
  const sinceUpdate = running && Number.isFinite(updatedAt) ? Math.max(0, now - updatedAt) : 0;
  return snapshot.lastOutputAgeMs + sinceUpdate;
}

export function liveTrainingRemainingSeconds(snapshot: RuntimeSnapshot | null, running: boolean, now: number): number | null {
  if (!snapshot || !running || snapshot.phase !== "training" || snapshot.training?.remainingSeconds == null) return null;
  if (runtimeOutputAgeMs(snapshot, running, now) > 10_000) return null;
  const updatedAt = Date.parse(snapshot.updatedAt);
  const elapsedSeconds = Number.isFinite(updatedAt) ? Math.max(0, now - updatedAt) / 1_000 : 0;
  return Math.max(0, snapshot.training.remainingSeconds - elapsedSeconds);
}

export function formatClockDuration(seconds: number): string {
  const wholeSeconds = Math.max(0, Math.ceil(seconds));
  const minutes = Math.floor(wholeSeconds / 60);
  return `${minutes}:${(wholeSeconds % 60).toString().padStart(2, "0")}`;
}
