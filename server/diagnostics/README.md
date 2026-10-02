# User-authorized diagnostic reports

These are integration sources, **not a deployed endpoint**. The production service lives in a separate project. Do not publish a client with an enabled success promise until this endpoint and retention job are deployed.

1. Apply `db/migrations/20261002_create_ooosplat_error_reports.sql`.
2. Reuse **POST `/api/telemetry/event`** (`https://www.ooolab.cn/api/telemetry/event`). Before the existing anonymous analytics validator, dispatch requests with **`X-OOOSplat-Report: diagnostic-v1`** to `createDiagnosticReportHandler(database, rateLimiter)`. All other requests continue through the existing analytics handler unchanged. This header is a protocol discriminator, not an identity or authentication credential. Diagnostic validation, persistence, consent and retention remain independent; do not pass diagnostic payloads through analytics' no-logs privacy validator or store them as analytics events.
3. Supply a PostgreSQL pool-compatible `query(sql, parameters)` adapter. A receipt is returned only after the insert completes; report UUIDs make retry inserts idempotent.
4. Use the existing production distributed rate limiter with **5 requests/minute per trusted connection IP**. `createReportRateLimiter` is a single-process reference; it retains only server-secret HMACs for one minute. Use a server-only secret. A multi-instance deployment must enforce the same limit at the gateway or shared store. Never trust client-supplied forwarding headers without the hosting provider's trusted-proxy integration.
5. Limit request bodies to **128 KiB** and set a request-body timeout at the gateway. Preserve the handler's streamed size limit, strict field validation and parameterized SQL.
6. Disable request-body logging, error-body logging and diagnostic payload capture in APM, proxies and access logs. Restrict report-table access to support operators. Never associate these reports with anonymous installation IDs, accounts or raw IPs.
7. Schedule `db/queries/cleanup_error_reports.sql` at least daily. Reports expire 30 days after receipt; ensure backup retention and support exports also honor that lifetime.
8. Keep this service on HTTPS. No client-side API secret is needed or embedded. Do not require analytics to be enabled. Validate/report only the supplied allowlisted fields; do not enrich with identifiers.

The request is the exact `DiagnosticReport` shown in the desktop preview. Success must be JSON `{ "accepted": true, "reportId": "<same UUID>" }`; 2xx HTML, redirects, an incorrect report ID, validation failure or a failed insert must not be treated as success. Avoid redirecting this route, including to a login page.

Framework-neutral routing in the existing service:

```ts
const handleDiagnostic = createDiagnosticReportHandler(database, rateLimiter);

export function handleEvent(request: Request, trustedAddress: string) {
  if (request.headers.get("x-ooosplat-report") === "diagnostic-v1") {
    return handleDiagnostic(request, trustedAddress);
  }
  return existingAnalyticsHandler(request, trustedAddress);
}
```

Sharing the URL alone does not enable reports: the production endpoint must add this dispatcher and database integration. The desktop app still requires an explicit matching diagnostic receipt, so an ordinary analytics acknowledgement is not reported as a successful diagnostic upload.

Run the mock integration tests with Node 22.17+:

```sh
npm run test:diagnostics
```

Test deployment with synthetic, non-personal reports before enabling the endpoint. No production deployment or migration is performed by these source changes.
