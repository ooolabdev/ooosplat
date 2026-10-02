import { describe, expect, it } from "vitest";
import { errorSummaryKey, shortErrorMessage } from "./errorSummary";

describe("compact error summaries", () => {
  it.each([
    ["Brush: IO error while loading dataset: early eof", "error.summaryDataset"],
    ["Brush: VK_ERROR_DEVICE_LOST", "error.summaryDevice"],
    ["Brush: out of memory", "error.summaryGraphicsMemory"],
    ["out of memory", "error.summaryMemory"],
    ["COLMAP database: permission denied", "error.summaryPermission"],
    ["FFmpeg: no space left on device", "error.summaryDisk"],
    ["COLMAP Mapper Failed", "error.summaryReconstruction"],
    ["Brush exited with code 1", "error.summaryTraining"],
    ["FFprobe failed", "error.summaryMedia"],
    ["未能打开文件夹：等待系统响应超时", "error.summaryTimeout"],
  ])("summarizes %s without exposing stack traces", (message, key) => {
    expect(errorSummaryKey(message)).toBe(key);
  });
  it("uses only the first line and bounds unknown errors without splitting unicode", () => {
    expect(shortErrorMessage("Short error\nlong stack trace", 36)).toBe("Short error");
    expect(shortErrorMessage("🚀".repeat(100), 36)).toBe("🚀".repeat(35) + "…");
    expect(errorSummaryKey("Unknown failure")).toBeNull();
  });
});
