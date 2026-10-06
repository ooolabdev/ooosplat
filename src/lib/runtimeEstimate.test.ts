import { describe, expect, it } from "vitest";
import type { RuntimeEstimate, RuntimeSnapshot } from "../types/pipeline";
import { formatClockDuration, liveTrainingRemainingSeconds, projectRuntime } from "./runtimeEstimate";

const estimate: RuntimeEstimate = {
  estimatedMs: 100_000,
  lowerBoundMs: 60_000,
  upperBoundMs: 160_000,
  confidence: "low",
  sampleCount: 0,
  basis: "test",
};

describe("runtime projection", () => {
  it("uses the initial estimate before enough live progress is available", () => {
    expect(projectRuntime(estimate, 4, 20_000, true)).toEqual({
      projectedTotalMs: 100_000,
      remainingMs: 80_000,
    });
  });

  it("blends observed progress without exceeding the safety bounds", () => {
    const projected = projectRuntime(estimate, 10, 20_000, true);
    expect(projected?.projectedTotalMs).toBe(145_000);
    expect(projected?.remainingMs).toBe(125_000);

    expect(projectRuntime(estimate, 5, 1_000_000, true)?.projectedTotalMs).toBe(200_000);
  });

  it("returns null when no estimate exists", () => {
    expect(projectRuntime(null, 50, 30_000, true)).toBeNull();
  });
});

const snapshot = (overrides: Partial<RuntimeSnapshot> = {}): RuntimeSnapshot => ({
  processId: 42,
  phase: "training",
  updatedAt: "2026-10-05T10:00:00.000Z",
  lastOutputAgeMs: 500,
  training: {
    iteration: 800,
    total: 8_000,
    startIter: 0,
    lod: 0,
    stepsPerSecond: 10,
    remainingSeconds: 90,
    splatCount: null,
    psnr: null,
    ssim: null,
  },
  device: null,
  backend: null,
  config: {},
  resources: null,
  ...overrides,
});

describe("live training estimate", () => {
  it("counts down between runtime snapshots", () => {
    expect(liveTrainingRemainingSeconds(snapshot(), true, Date.parse("2026-10-05T10:00:04.000Z"))).toBe(86);
    expect(formatClockDuration(86)).toBe("1:26");
  });

  it("hides estimates outside training or after output becomes stale", () => {
    expect(liveTrainingRemainingSeconds(snapshot({ phase: "exporting" }), true, Date.parse("2026-10-05T10:00:01.000Z"))).toBeNull();
    expect(liveTrainingRemainingSeconds(snapshot(), true, Date.parse("2026-10-05T10:00:10.000Z"))).toBeNull();
    expect(liveTrainingRemainingSeconds(snapshot(), false, Date.parse("2026-10-05T10:00:01.000Z"))).toBeNull();
  });
});
