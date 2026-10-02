BEGIN;
CREATE TABLE IF NOT EXISTS ooosplat_error_reports (
    report_id UUID PRIMARY KEY,
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '30 days'),
    event_timestamp TIMESTAMPTZ NOT NULL,
    app_version TEXT NOT NULL,
    failure_stage TEXT,
    engine TEXT,
    error_code TEXT NOT NULL,
    report JSONB NOT NULL CHECK (jsonb_typeof(report) = 'object')
);
CREATE INDEX IF NOT EXISTS ooosplat_error_reports_expiry ON ooosplat_error_reports (expires_at);
COMMENT ON TABLE ooosplat_error_reports IS 'User-authorized redacted diagnostics; no install IDs, media, device serials or client IPs. Retain for 30 days.';
COMMIT;
