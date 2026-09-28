-- Observational comparison only: Planner is user-selected, not randomized.
-- Bind :from_timestamp and :to_timestamp in the analytics service.
SELECT
    planner_enabled,
    planner_version,
    quality_preset,
    input_type,
    brush_final_profile,
    COUNT(*) AS runs,
    AVG((outcome = 'completed')::INT) AS completion_rate,
    AVG(total_duration_ms) FILTER (WHERE outcome = 'completed') AS avg_total_duration_ms,
    AVG(final_registered_images::DOUBLE PRECISION / NULLIF(final_input_images, 0))
        FILTER (WHERE outcome = 'completed') AS avg_registration_ratio,
    AVG(final_points_3d::DOUBLE PRECISION / NULLIF(final_input_images, 0))
        FILTER (WHERE outcome = 'completed') AS avg_points_per_input_image,
    AVG(final_splat_count) FILTER (WHERE outcome = 'completed') AS avg_splat_count,
    AVG(final_ply_size_bytes) FILTER (WHERE outcome = 'completed') AS avg_ply_size_bytes
FROM ooosplat_planner_evaluations
WHERE event_timestamp >= :from_timestamp
  AND event_timestamp < :to_timestamp
GROUP BY planner_enabled, planner_version, quality_preset, input_type, brush_final_profile
ORDER BY quality_preset, input_type, planner_enabled;

-- Bridge effectiveness and cost. Gains are calculated from exact stored counts.
SELECT
    quality_preset,
    input_type,
    bridge_status,
    COUNT(*) AS runs,
    AVG(bridge_adopted::INT) AS adoption_rate,
    AVG(bridge_final_registered_images - bridge_initial_registered_images) AS avg_registered_gain,
    AVG(bridge_final_points_3d - bridge_initial_points_3d) AS avg_points_gain,
    AVG(bridge_duration_ms) AS avg_bridge_duration_ms
FROM ooosplat_planner_evaluations
WHERE event_timestamp >= :from_timestamp
  AND event_timestamp < :to_timestamp
  AND bridge_status IS NOT NULL
GROUP BY quality_preset, input_type, bridge_status
ORDER BY quality_preset, input_type, bridge_status;

-- Brush memory-profile and OOM fallback outcomes.
SELECT
    brush_initial_profile,
    brush_final_profile,
    brush_oom_retry_used,
    COUNT(*) AS runs,
    AVG((outcome = 'completed')::INT) AS completion_rate,
    AVG(duration_training_splats_ms) AS avg_training_ms,
    AVG(final_splat_count) FILTER (WHERE outcome = 'completed') AS avg_splat_count
FROM ooosplat_planner_evaluations
WHERE event_timestamp >= :from_timestamp
  AND event_timestamp < :to_timestamp
  AND brush_initial_profile IS NOT NULL
GROUP BY brush_initial_profile, brush_final_profile, brush_oom_retry_used
ORDER BY brush_initial_profile, brush_final_profile, brush_oom_retry_used;
