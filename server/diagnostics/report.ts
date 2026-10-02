// Independent diagnostic handler mounted at the existing POST /api/telemetry/event.
// Route X-OOOSplat-Report: diagnostic-v1 here before anonymous telemetry validation.
import { createHmac } from "node:crypto";

export const MAX_REPORT_BYTES = 128 * 1024;
const encoder = new TextEncoder();
const stages = new Set(["created", "probingVideo", "planningFrames", "extractingFrames", "extractingFeatures", "matching", "reconstructing", "validatingReconstruction", "trainingSplats", "exporting"]);
const engines = new Set(["system", "ffmpeg", "colmap", "brush"]);
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

function object(value: unknown, keys: string[]): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid object");
  const record = value as Record<string, unknown>;
  if (Object.keys(record).length !== keys.length || keys.some(key => !(key in record))) throw new Error("Invalid fields");
  return record;
}
function text(value: unknown, max: number): string {
  if (typeof value !== "string" || encoder.encode(value).length > max || /[\u0000-\u0008\u000b\u000c\u000e-\u001f]/.test(value)) throw new Error("Invalid text");
  return value;
}
function nullableText(value: unknown, max: number): string | null { return value === null ? null : text(value, max); }
function member(value: unknown, choices: Set<string>) { if (value !== null && (typeof value !== "string" || !choices.has(value))) throw new Error("Invalid enum"); }

export function validateDiagnosticReport(value: unknown) {
  const report = object(value, ["schemaVersion", "reportId", "timestamp", "appVersion", "stage", "engine", "errorCode", "reason", "detail", "logTail", "logsTruncated", "environment"]);
  if (report.schemaVersion !== 1 || !uuid.test(text(report.reportId, 36))) throw new Error("Invalid report identifier");
  if (!Number.isFinite(Date.parse(text(report.timestamp, 64)))) throw new Error("Invalid timestamp");
  text(report.appVersion, 64); member(report.stage, stages); member(report.engine, engines);
  text(report.errorCode, 64); text(report.reason, 512); text(report.detail, 16 * 1024); text(report.logTail, 64 * 1024);
  if ((report.logTail as string).split(/\r?\n/).length > 200) throw new Error("Too many log lines");
  if (typeof report.logsTruncated !== "boolean") throw new Error("Invalid truncation flag");
  const environment = object(report.environment, ["system", "gpus", "actualDevice"]);
  const system = object(environment.system, ["name", "version", "arch"]);
  text(system.name, 1024); nullableText(system.version, 1024); text(system.arch, 64);
  if (!Array.isArray(environment.gpus) || environment.gpus.length > 16) throw new Error("Invalid GPU list");
  const names = environment.gpus.map(value => {
    const gpu = object(value, ["name", "driverVersion", "totalMemoryMb", "computeCapability"]);
    const name = text(gpu.name, 1024); nullableText(gpu.driverVersion, 1024); nullableText(gpu.computeCapability, 32);
    if (gpu.totalMemoryMb !== null && (typeof gpu.totalMemoryMb !== "number" || !Number.isSafeInteger(gpu.totalMemoryMb) || gpu.totalMemoryMb < 0)) throw new Error("Invalid VRAM");
    return name;
  });
  const actual = nullableText(environment.actualDevice, 1024);
  if (actual !== null && !names.includes(actual)) throw new Error("Unknown actual GPU");
  if (encoder.encode(JSON.stringify(report)).length > MAX_REPORT_BYTES) throw new Error("Report too large");
  return report;
}

export interface ReportDatabase { query(sql: string, parameters: unknown[]): Promise<unknown> }

export function createReportRateLimiter(secret: string, now: () => number = Date.now) {
  if (!secret) throw new Error("A server-only rate-limit secret is required");
  const entries = new Map<string, { until: number; count: number }>();
  return (address: string) => {
    const time = now();
    for (const [key, value] of entries) if (value.until <= time) entries.delete(key);
    const key = createHmac("sha256", secret).update(address).digest("hex");
    const entry = entries.get(key) ?? { until: time + 60_000, count: 0 };
    if (entries.size >= 10_000 && !entries.has(key)) return false;
    entry.count += 1; entries.set(key, entry);
    return entry.count <= 5;
  };
}

async function limitedBody(request: Request): Promise<string> {
  const reader = request.body?.getReader();
  if (!reader) throw new Error("Empty body");
  const parts: Uint8Array[] = []; let total = 0;
  try {
    while (true) {
      const { value, done } = await reader.read(); if (done) break;
      total += value.byteLength;
      if (total > MAX_REPORT_BYTES) { await reader.cancel(); throw new RangeError("Body too large"); }
      parts.push(value);
    }
  } finally { reader.releaseLock(); }
  const bytes = new Uint8Array(total); let offset = 0;
  for (const part of parts) { bytes.set(part, offset); offset += part.length; }
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
}
const response = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json", "Cache-Control": "no-store" } });

export function createDiagnosticReportHandler(database: ReportDatabase, allow: (trustedAddress: string) => boolean | Promise<boolean>) {
  // Supply the connection address from the trusted hosting adapter, never arbitrary X-Forwarded-For.
  return async (request: Request, trustedAddress: string): Promise<Response> => {
    if (request.method !== "POST") return response(405, { error: "method_not_allowed" });
    if (!request.headers.get("content-type")?.toLowerCase().startsWith("application/json")) return response(415, { error: "json_required" });
    if (request.headers.get("x-ooosplat-report") !== "diagnostic-v1") return response(400, { error: "diagnostic_type_required" });
    if (!await allow(trustedAddress)) return response(429, { error: "rate_limited" });
    if (Number(request.headers.get("content-length")) > MAX_REPORT_BYTES) return response(413, { error: "too_large" });
    let report: ReturnType<typeof validateDiagnosticReport>;
    try { report = validateDiagnosticReport(JSON.parse(await limitedBody(request))); }
    catch (error) { return response(error instanceof RangeError ? 413 : 400, { error: "invalid_report" }); }
    try {
      await database.query(
        "INSERT INTO ooosplat_error_reports (report_id, event_timestamp, app_version, failure_stage, engine, error_code, report) VALUES ($1,$2,$3,$4,$5,$6,$7::jsonb) ON CONFLICT (report_id) DO NOTHING",
        [report.reportId, report.timestamp, report.appVersion, report.stage, report.engine, report.errorCode, JSON.stringify(report)],
      );
      return response(200, { accepted: true, reportId: report.reportId });
    } catch { return response(503, { error: "storage_unavailable" }); }
  };
}
