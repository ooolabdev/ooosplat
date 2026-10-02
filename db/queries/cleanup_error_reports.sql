-- Run at least daily using the deployment's database scheduler. Do not log deleted payloads.
DELETE FROM ooosplat_error_reports WHERE expires_at <= NOW();
