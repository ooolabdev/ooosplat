import { beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "./appStore";
import type { PipelineEvent, RuntimeSnapshot } from "../types/pipeline";
import type { SharedTask } from "../types/tasks";

const event = (sequence: number, progress: number): PipelineEvent => ({
  sequence,
  timestamp: new Date().toISOString(),
  kind: "progress",
  level: "info",
  stage: "ExtractingFrames",
  engine: "ffmpeg",
  progress,
  stageProgress: progress,
  indeterminate: false,
  message: `event ${sequence}`,
  current: sequence,
  total: 500,
  unit: "张",
  elapsedMs: sequence * 100,
  acceleration: null,
});

describe("app store", () => {
  beforeEach(() => {
    useAppStore.setState({
      inputPath: null, inputType: "video", projectsRoot: "", projects: [], quality: "balanced", colmapAcceleration: null, taskColmapAcceleration: null, video: null, imageSequence: null,
      plan: null, estimate: null, engines: [], phase: "idle", progress: 0, progressMessage: "",
      latestEvent: null, latestRuntime: null, lastEventSequence: 0, events: [], liveTask: null, result: null, error: null, errorAt: null,
    });
  });

  it("uses Balanced by default", () => {
    expect(useAppStore.getState().quality).toBe("balanced");
  });

  it("commits a whole ordered task batch once without losing logs before the latest runtime", () => {
    const task = { task_id: "task-1", run_id: "run-1", status: "running", estimated_progress: 42,
      elapsed_ms: 15_000, updated_at: new Date().toISOString(), result: null } as SharedTask;
    const snapshot: RuntimeSnapshot = { processId: 42, phase: "running", updatedAt: task.updated_at,
      lastOutputAgeMs: 0, training: null, device: null, backend: null, config: {}, resources: null };
    const batch = Array.from({ length: 200 }, (_, index) => ({ ...event(index + 1, 42), taskId: task.task_id, runId: task.run_id }));
    batch.push({ ...event(201, 42), taskId: task.task_id, runId: task.run_id, kind: "runtime", runtime: snapshot });
    const changed = { count: 0 };
    const unsubscribe = useAppStore.subscribe(() => { changed.count++; });
    useAppStore.getState().receiveTaskBatch(task, batch.reverse(), true);
    unsubscribe();
    expect(changed.count).toBe(1);
    expect(useAppStore.getState().events.map(item => item.sequence)).toEqual(Array.from({ length: 200 }, (_, index) => index + 1));
    expect(useAppStore.getState().latestRuntime).toEqual(snapshot);
    expect(useAppStore.getState().liveTask?.elapsed_ms).toBe(15_000);
    expect(useAppStore.getState().lastEventSequence).toBe(201);
  });

  it("applies the terminal snapshot after its final logs and ignores cross-run entries", () => {
    const task = { task_id: "task-1", run_id: "run-1", status: "running", estimated_progress: 63,
      elapsed_ms: 15_000, updated_at: new Date().toISOString(), result: null } as SharedTask;
    useAppStore.getState().receiveTaskBatch(task, [{ ...event(1, 63), taskId: task.task_id, runId: task.run_id }], true);
    useAppStore.getState().receiveTaskBatch({ ...task, status: "cancelled", elapsed_ms: 16_000 }, [
      { ...event(2, 63), taskId: task.task_id, runId: task.run_id, kind: "log", message: "last useful line" },
      { ...event(999, 100), taskId: task.task_id, runId: "old-run", message: "foreign line" },
    ], false);
    expect(useAppStore.getState()).toMatchObject({ phase: "cancelled", progress: 63, lastEventSequence: 2 });
    expect(useAppStore.getState().events.at(-1)?.message).toBe("last useful line");
    expect(useAppStore.getState().liveTask?.elapsed_ms).toBe(16_000);
  });

  it("rejects older events after a runtime-only batch and advances snapshots after merging their logs", () => {
    const task = { task_id: "task-1", run_id: "run-1", status: "running", sequence: 10, estimated_progress: 42,
      elapsed_ms: 15_000, updated_at: new Date().toISOString(), result: null } as SharedTask;
    const snapshot: RuntimeSnapshot = { processId: 42, phase: "running", updatedAt: task.updated_at,
      lastOutputAgeMs: 0, training: null, device: null, backend: null, config: {}, resources: null };
    useAppStore.getState().receiveTaskBatch(task, [{ ...event(10, 42), taskId: task.task_id,
      runId: task.run_id, kind: "runtime", runtime: snapshot }], true);
    useAppStore.getState().receiveTaskBatch(task, [{ ...event(9, 42), taskId: task.task_id, runId: task.run_id }], false);
    expect(useAppStore.getState().events).toHaveLength(0);
    expect(useAppStore.getState().lastEventSequence).toBe(10);
    useAppStore.getState().receiveTaskBatch({ ...task, sequence: 20 }, [
      { ...event(11, 42), taskId: task.task_id, runId: task.run_id },
      { ...event(12, 42), taskId: task.task_id, runId: task.run_id },
    ], false);
    expect(useAppStore.getState().events.map(item => item.sequence)).toEqual([11, 12]);
    expect(useAppStore.getState().lastEventSequence).toBe(20);
  });

  it("timestamps global errors and clears their timestamp with the message", () => {
    useAppStore.getState().setError("latest failure");
    expect(useAppStore.getState().errorAt).toEqual(expect.any(Number));
    useAppStore.getState().setError(null);
    expect(useAppStore.getState()).toMatchObject({ error: null, errorAt: null });
  });

  it("keeps runtime snapshots out of logs and rejects stale or post-terminal updates", () => {
    const snapshot: RuntimeSnapshot = {
      processId: 42,
      phase: "training",
      updatedAt: new Date().toISOString(),
      lastOutputAgeMs: 0,
      training: null,
      device: null,
      backend: null,
      config: {},
      resources: null,
    };
    useAppStore.getState().beginRun();
    useAppStore.getState().receiveEvent(event(1, 20));
    for (let sequence = 2; sequence < 510; sequence += 1) {
      useAppStore.getState().receiveEvent({ ...event(sequence, 20), kind: "runtime", runtime: snapshot });
    }
    expect(useAppStore.getState().events).toHaveLength(1);
    expect(useAppStore.getState().progressMessage).toBe("event 1");
    expect(useAppStore.getState().latestRuntime?.processId).toBe(42);
    useAppStore.getState().receiveEvent({ ...event(508, 20), kind: "runtime", runtime: { ...snapshot, processId: 1 } });
    expect(useAppStore.getState().latestRuntime?.processId).toBe(42);
    useAppStore.getState().receiveEvent({ ...event(510, 20), stage: "failed" });
    useAppStore.getState().receiveEvent({ ...event(511, 20), kind: "runtime", runtime: { ...snapshot, processId: 2 } });
    expect(useAppStore.getState().latestRuntime?.processId).toBe(42);
    useAppStore.getState().beginRun();
    expect(useAppStore.getState().latestRuntime).toBeNull();
    expect(useAppStore.getState().lastEventSequence).toBe(0);
  });

  it("replaces all training fields when a retry starts", () => {
    useAppStore.getState().beginRun();
    const snapshot: RuntimeSnapshot = {
      processId: 42,
      phase: "training",
      updatedAt: new Date().toISOString(),
      lastOutputAgeMs: 0,
      training: { iteration: 20, total: 100, startIter: 0, lod: 0, stepsPerSecond: 10, remainingSeconds: 8, splatCount: 200, psnr: 20, ssim: 0.9 },
      device: "RTX",
      backend: "Vulkan",
      config: { seed: "42" },
      resources: null,
    };
    useAppStore.getState().receiveEvent({ ...event(1, 20), kind: "runtime", runtime: snapshot });
    useAppStore.getState().receiveEvent({ ...event(2, 20), kind: "runtime", runtime: { ...snapshot, processId: 43, phase: "starting", training: null, config: {}, device: null } });
    expect(useAppStore.getState().latestRuntime).toMatchObject({ processId: 43, training: null, config: {}, device: null });
  });

  it("invalidates a plan when quality changes", () => {
    useAppStore.setState({
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 900 },
      estimate: { estimatedMs: 100_000, lowerBoundMs: 60_000, upperBoundMs: 160_000, confidence: "low", sampleCount: 1, basis: "test" },
    });
    useAppStore.getState().setQuality("high");
    expect(useAppStore.getState().quality).toBe("high");
    expect(useAppStore.getState().plan).toBeNull();
    expect(useAppStore.getState().estimate).toBeNull();
  });

  it("switches input type and clears the previous analysis", () => {
    useAppStore.setState({
      video: { duration: 1, width: 1, height: 1, fps: 30, totalFrames: 30, codec: "h264", rotation: 0, pixelFormat: "yuv420p", hasAlpha: false },
      plan: { retentionRatio: 1, samplingFps: 30, estimatedFrames: 30 },
      phase: "failed",
      progress: 63,
      progressMessage: "old task failed",
      latestEvent: event(2, 63),
      events: [event(1, 42), event(2, 63)],
      error: "old error",
    });
    useAppStore.getState().setInputPath("E:\\Photos", "images");
    expect(useAppStore.getState()).toMatchObject({
      inputType: "images",
      inputPath: "E:\\Photos",
      video: null,
      plan: null,
      phase: "idle",
      progress: 0,
      progressMessage: "",
      latestEvent: null,
      events: [],
      error: null,
    });
  });

  it("keeps progress monotonic and ignores stale sequenced events", () => {
    useAppStore.getState().receiveEvent(event(2, 42));
    useAppStore.getState().receiveEvent(event(1, 12));
    expect(useAppStore.getState().progress).toBe(42);
    expect(useAppStore.getState().events).toHaveLength(1);
  });

  it("does not let failed or cancelled terminal events force progress to 100%", () => {
    useAppStore.getState().receiveEvent(event(1, 63));
    useAppStore.getState().receiveEvent({ ...event(2, 100), stage: "failed", stageProgress: null });
    expect(useAppStore.getState().progress).toBe(63);
    useAppStore.getState().receiveEvent({ ...event(3, 100), stage: "cancelled", stageProgress: null });
    expect(useAppStore.getState().progress).toBe(63);
  });

  it("caps the friendly live log at 500 entries", () => {
    for (let index = 1; index <= 530; index += 1) useAppStore.getState().receiveEvent(event(index, index / 10));
    expect(useAppStore.getState().events).toHaveLength(500);
    expect(useAppStore.getState().events[0].sequence).toBe(31);
  });

  it("keeps task acceleration separate from the system probe", () => {
    useAppStore.getState().receiveEvent({
      ...event(1, 0),
      kind: "capability",
      acceleration: {
        backend: "gpu",
        detectionState: "ready",
        reasonCode: "gpuReady",
        reason: "GPU ready",
        device: { index: 0, name: "RTX 3060 Ti", driverVersion: "560.81", computeCapability: "8.6" },
        requirements: { minimumDriverVersion: "528.33", minimumComputeCapability: "5.0" },
      },
    });
    expect(useAppStore.getState().colmapAcceleration).toBeNull();
    expect(useAppStore.getState().taskColmapAcceleration?.backend).toBe("gpu");
    expect(useAppStore.getState().taskColmapAcceleration?.device?.index).toBe(0);
  });
});
