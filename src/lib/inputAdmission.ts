export type InputAdmissionReason = "colmap_disabled" | "unsupported_input";

/** Admission failures are not engine failures. Do not infer them from log text. */
export function inputAdmissionReason(error: unknown): InputAdmissionReason | null {
  if (typeof error !== "object" || error === null || Array.isArray(error)) return null;
  const candidate = error as Record<string, unknown>;
  if (candidate.code !== "UNSUPPORTED_INPUT") return null;
  return candidate.reason === "colmap_disabled" || candidate.reason === "unsupported_input"
    ? candidate.reason
    : null;
}
