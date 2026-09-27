import { describe, expect, it } from "vitest";
import { pipelineCommandError, pipelineErrorCode, pipelineErrorMessage, pipelineWasCancelled } from "./pipelineError";

describe("pipeline command errors", () => {
  it("reads structured cancellation errors", () => {
    const error = { code: "cancelled", message: "任务已取消" };
    expect(pipelineErrorCode(error)).toBe("cancelled");
    expect(pipelineErrorMessage(error)).toBe("任务已取消");
    expect(pipelineWasCancelled(error)).toBe(true);
  });

  it("keeps legacy string errors readable", () => {
    expect(pipelineErrorCode("legacy failure")).toBeNull();
    expect(pipelineErrorMessage("legacy failure")).toBe("legacy failure");
    expect(pipelineWasCancelled("旧版取消消息", "cancelled")).toBe(true);
    expect(pipelineWasCancelled("cancelled without a terminal event")).toBe(false);
  });

  it("preserves optional failure guidance context", () => {
    expect(pipelineCommandError({
      code: "pipeline_failed",
      message: "IO error while loading dataset: early eof",
      failedStage: "trainingSplats",
      engine: "brush",
      failureKind: "brush_dataset",
      projectId: "11111111-1111-1111-1111-111111111111",
      logsDirectory: "E:\\Projects\\scene\\logs",
    })).toMatchObject({
      failedStage: "trainingSplats",
      engine: "brush",
      failureKind: "brush_dataset",
      projectId: "11111111-1111-1111-1111-111111111111",
    });
  });

  it("preserves the dedicated Brush device-lost guidance kind", () => {
    expect(pipelineCommandError({
      code: "pipeline_failed",
      message: "VK_ERROR_DEVICE_LOST",
      failedStage: "trainingSplats",
      engine: "brush",
      failureKind: "brush_device_lost",
    })?.failureKind).toBe("brush_device_lost");
  });

  it("drops unknown engines and failure kinds", () => {
    expect(pipelineCommandError({
      code: "pipeline_failed",
      message: "failed",
      engine: "other",
      failureKind: "other",
    })).toEqual({ code: "pipeline_failed", message: "failed" });
  });
});
