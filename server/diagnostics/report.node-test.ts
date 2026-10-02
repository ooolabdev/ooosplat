import { test } from "node:test";
import assert from "node:assert/strict";
import { createDiagnosticReportHandler, createReportRateLimiter, validateDiagnosticReport, MAX_REPORT_BYTES } from "./report.ts";

function fixture() {
  return { schemaVersion: 1, reportId: "11111111-1111-4111-8111-111111111111", timestamp: "2026-10-02T08:00:00Z", appVersion: "0.5.0", stage: "matching", engine: "colmap", errorCode: "pipeline_failed", reason: "Could not finish", detail: "early eof [REDACTED]", logTail: "last error", logsTruncated: false, environment: { system: { name: "Windows 11", version: "10.0", arch: "x86_64" }, gpus: [{ name: "Intel", driverVersion: "1", totalMemoryMb: null, computeCapability: null }, { name: "NVIDIA", driverVersion: "2", totalMemoryMb: 8192, computeCapability: "8.6" }], actualDevice: "NVIDIA" } };
}
function request(value: unknown) { return new Request("https://example.invalid/api/telemetry/event", { method: "POST", headers: { "Content-Type": "application/json", "X-OOOSplat-Report": "diagnostic-v1" }, body: JSON.stringify(value) }); }

test("strict allowlist excludes identifiers and hardware command extras", () => {
  assert.doesNotThrow(() => validateDiagnosticReport(fixture()));
  assert.throws(() => validateDiagnosticReport({ ...fixture(), installId: "private" }));
  const value = fixture(); Object.assign(value.environment.gpus[0], { serial: "private" });
  assert.throws(() => validateDiagnosticReport(value));
  assert.throws(() => validateDiagnosticReport({ ...fixture(), logTail: "x".repeat(64 * 1024 + 1) }));
  assert.throws(() => validateDiagnosticReport({ ...fixture(), logTail: Array(201).fill("line").join("\n") }));
});
test("only acknowledges after database commit and uses parameterized inserts", async () => {
  const queries: Array<[string, unknown[]]> = [];
  const handler = createDiagnosticReportHandler({ query: async (sql, parameters) => { queries.push([sql, parameters]); } }, () => true);
  const result = await handler(request(fixture()), "trusted-address");
  assert.deepEqual(await result.json(), { accepted: true, reportId: fixture().reportId });
  assert.equal(queries.length, 1); assert.match(queries[0][0], /ON CONFLICT/);
  assert.deepEqual(JSON.parse(queries[0][1][6] as string), fixture());
  const failed = createDiagnosticReportHandler({ query: async () => { throw new Error("private DB details"); } }, () => true);
  const failure = await failed(request(fixture()), "trusted-address");
  assert.equal(failure.status, 503); assert.doesNotMatch(await failure.text(), /private/);
});
test("invalid, oversized and rate-limited reports never reach persistence", async () => {
  let inserts = 0; const database = { query: async () => { inserts += 1; } };
  const handler = createDiagnosticReportHandler(database, () => true);
  assert.equal((await handler(request({ ...fixture(), username: "private" }), "ip")).status, 400);
  assert.equal((await handler(request({ text: "x".repeat(MAX_REPORT_BYTES + 1) }), "ip")).status, 413);
  assert.equal((await createDiagnosticReportHandler(database, () => false)(request(fixture()), "ip")).status, 429);
  assert.equal(inserts, 0);
});
test("rate limiter allows five per minute and recovers without retaining raw addresses", () => {
  let now = 0; const allow = createReportRateLimiter("server-only-test-secret", () => now);
  for (let i = 0; i < 5; i++) assert.equal(allow("address"), true);
  assert.equal(allow("address"), false); now += 60_001; assert.equal(allow("address"), true);
});

test("commit acknowledgement waits and duplicate report IDs remain idempotent", async () => {
  let release!: () => void;
  const committed = new Promise<void>(resolve => { release = resolve; });
  const stored = new Set<string>();
  const handler = createDiagnosticReportHandler({ query: async (_sql, parameters) => {
    await committed;
    stored.add(parameters[0] as string);
  } }, () => true);
  let acknowledged = false;
  const pending = handler(request(fixture()), "ip").then(result => { acknowledged = true; return result; });
  await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(acknowledged, false);
  assert.equal(stored.size, 0);
  release();
  assert.equal((await pending).status, 200);
  assert.equal((await handler(request(fixture()), "ip")).status, 200);
  assert.equal(stored.size, 1);
});

test("wrong methods, content types and malformed UTF-8 are rejected", async () => {
  let inserts = 0;
  const handler = createDiagnosticReportHandler({ query: async () => { inserts += 1; } }, () => true);
  const url = "https://example.invalid/api/telemetry/event";
  assert.equal((await handler(new Request(url), "ip")).status, 405);
  assert.equal((await handler(new Request(url, { method: "POST", body: "text" }), "ip")).status, 415);
  assert.equal((await handler(new Request(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: new Uint8Array([0xff]) }), "ip")).status, 400);
  assert.equal((await handler(new Request(url, { method: "POST", headers: { "Content-Type": "application/json", "X-OOOSplat-Report": "diagnostic-v1" }, body: new Uint8Array([0xff]) }), "ip")).status, 400);
  assert.equal(inserts, 0);
});
