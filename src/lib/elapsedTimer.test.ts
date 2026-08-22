import { afterEach, describe, expect, it, vi } from "vitest";
import { elapsedSince, startElapsedTicker } from "./elapsedTimer";

describe("elapsed timer", () => {
  afterEach(() => vi.useRealTimers());

  it("uses wall-clock time so delayed ticks do not drift", () => {
    expect(elapsedSince(1_000, 8_500)).toBe(7_500);
    expect(elapsedSince(8_500, 1_000)).toBe(0);
  });

  it("keeps updating without pipeline or log events and stops cleanly", () => {
    vi.useFakeTimers();
    vi.setSystemTime(10_000);
    const values: number[] = [];
    const stop = startElapsedTicker(10_000, (value) => values.push(value));

    vi.advanceTimersByTime(2_000);
    expect(values.at(-1)).toBe(2_000);

    vi.setSystemTime(20_000);
    vi.advanceTimersByTime(1_000);
    expect(values.at(-1)).toBe(11_000);

    stop();
    vi.setSystemTime(30_000);
    vi.advanceTimersByTime(8_000);
    expect(values.at(-1)).toBe(11_000);
  });
});
