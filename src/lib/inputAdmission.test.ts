import { describe, expect, it } from "vitest";
import { inputAdmissionReason } from "./inputAdmission";

describe("input admission failures", () => {
  it.each(["colmap_disabled", "unsupported_input"] as const)("recognizes the typed %s rejection", reason => {
    expect(inputAdmissionReason({ code: "UNSUPPORTED_INPUT", reason, message: "localized detail" })).toBe(reason);
  });

  it.each([
    null, undefined, "COLMAP data cannot be imported", new Error("COLMAP failed"),
    { code: "pipeline_failed", reason: "colmap_disabled" },
    { code: "UNSUPPORTED_INPUT", reason: "unknown" },
    { code: "UNSUPPORTED_INPUT", message: "COLMAP" },
    { code: "io_failed", message: "Permission denied" },
  ])("does not guess an admission category from %j", error => {
    expect(inputAdmissionReason(error)).toBeNull();
  });
});
