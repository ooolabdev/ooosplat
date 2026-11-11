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

  it.each(['IO_ERROR', 'INVALID_DATA', 'TASK_ERROR'])('reads typed preparation details through a %s service error', code => {
    const error = { code, message: 'outer service message', task_id: 'task', run_id: 'stopped-run', details: {
      pipelineError: { code: 'pipeline_failed', message: '文件读写失败; retry_preparation_operation=write_retry_reuse_barrier; path_role=project_state; os_error=5',
        failedStage: null, engine: 'system', classificationVersion: 1, classificationIsHeuristic: false,
        projectId: 'project', failureId: 'report', projectPath: 'E:/private/project', logsDirectory: 'E:/private/project/logs' },
      preparation: { operation: 'write_retry_reuse_barrier', pathRole: 'project_state', osError: 5 },
    } };
    expect(pipelineCommandError(error)).toMatchObject({
      code: 'pipeline_failed', engine: 'system', projectId: 'project', failureId: 'report',
      taskId: 'task', runId: 'stopped-run', classificationVersion: 1, classificationIsHeuristic: false,
    });
    expect(pipelineCommandError(error)?.failedStage).toBeUndefined();
    expect(pipelineCommandError(error)?.projectPath).toBeUndefined();
    expect(pipelineCommandError(error)?.logsDirectory).toBeUndefined();
    expect(pipelineErrorCode(error)).toBe('pipeline_failed');
    expect(pipelineErrorMessage(error)).toContain('retry_preparation_operation=write_retry_reuse_barrier');
    expect(pipelineErrorMessage(error)).not.toContain('outer service message');
  });

  it('preserves the classified native envelope instead of inferring from the outer message', () => {
    const error = { code: 'TASK_ERROR', message: 'COLMAP mapper source message', task_id: 'outer-task', run_id: 'outer-run', details: {
      pipelineError: { code: 'pipeline_failed', message: 'GPU OOM root cause', engine: 'brush', failureKind: 'brush_out_of_memory',
        classificationVersion: 1, classificationIsHeuristic: false, taskId: 'exact-task', runId: 'exact-run',
        termination: { exitCode: 101 } },
    } };
    expect(pipelineCommandError(error)).toMatchObject({ failureKind: 'brush_out_of_memory', engine: 'brush',
      taskId: 'exact-task', runId: 'exact-run', termination: { exitCode: 101 } });
    expect(pipelineErrorMessage(error)).toBe('GPU OOM root cause');
  });

  it.each([null, [], { pipelineError: null }, { pipelineError: { code: 'unknown', message: 'untrusted' } },
    { pipelineError: { code: 'pipeline_failed', message: 3 } }])('ignores malformed service details without hiding the original message: %j', details => {
    const error = { code: 'IO_ERROR', message: 'original error', details };
    expect(pipelineCommandError(error)).toBeNull();
    expect(pipelineErrorMessage(error)).toBe('original error');
  });
});
