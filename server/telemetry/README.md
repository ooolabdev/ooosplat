# Planner evaluation backend integration

The desktop client posts `planner_evaluation` to the existing
`/api/telemetry/event` endpoint. Deploy the database migration before releasing a
client that contains this event.

Backend integration order:

1. Apply `db/migrations/20260928_create_ooosplat_planner_evaluations.sql`.
2. Add `plannerEvaluationEnvelopeSchema` to the endpoint's existing Zod
   `discriminatedUnion("event", ...)`.
3. For this event only, call `toPlannerEvaluationRow` and insert the returned
   fixed-column row into `ooosplat_planner_evaluations` with
   `ON CONFLICT (run_hash) DO NOTHING`.
4. Keep all other event variants on the existing validation and table path.

The endpoint must use the same server-side HMAC secret and install-hash helper as
the current telemetry service. The raw `installId` and `runId` are request-only
values and must never be logged or persisted. Unknown fields are rejected by all
`.strict()` schemas.

The files in this directory are backend integration sources. They are not part of
the desktop TypeScript build because the production endpoint is deployed from a
separate service.
