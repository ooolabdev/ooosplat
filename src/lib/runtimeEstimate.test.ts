import { describe, expect, it } from "vitest";
import type { RuntimeEstimate } from "../types/pipeline";
import { projectRuntime } from "./runtimeEstimate";

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
