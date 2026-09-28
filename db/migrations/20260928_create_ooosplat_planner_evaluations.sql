BEGIN;

CREATE TABLE IF NOT EXISTS ooosplat_planner_evaluations (
    id BIGSERIAL PRIMARY KEY,
    event_timestamp TIMESTAMPTZ NOT NULL,
    install_hash CHAR(64) NOT NULL,
    run_hash CHAR(64) NOT NULL UNIQUE,
    app_version TEXT NOT NULL,
    os TEXT NOT NULL,
    arch TEXT NOT NULL,
    gpu_vendor TEXT NOT NULL,
    planner_schema_version INTEGER NOT NULL,

    run_kind TEXT NOT NULL,
    outcome TEXT NOT NULL,
    planner_enabled BOOLEAN NOT NULL,
    planner_version TEXT,
    quality_preset TEXT NOT NULL,
    input_type TEXT NOT NULL,
    failure_stage TEXT,
    error_code TEXT,
    total_duration_ms BIGINT NOT NULL,

    source_width BIGINT,
    source_height BIGINT,
    source_item_count BIGINT,
    configured_target_fps DOUBLE PRECISION,
    configured_candidate_fps DOUBLE PRECISION,
    effective_target_fps DOUBLE PRECISION,
    effective_candidate_fps DOUBLE PRECISION,
    initial_selected_count BIGINT,
    candidate_count BIGINT,
    minimum_frame_override_applied BOOLEAN,
    minimum_frame_target_unreachable BOOLEAN,

    initial_sfm_input_images BIGINT,
    initial_sfm_registered_images BIGINT,
    initial_sfm_points_3d BIGINT,
    initial_sfm_backend TEXT,
    initial_sfm_allow_two_view_tracks BOOLEAN,

    bridge_status TEXT,
    bridge_trigger_ratio DOUBLE PRECISION,
    bridge_available_budget BIGINT,
    bridge_requested_frames BIGINT,
    bridge_added_frames BIGINT,
    bridge_internal_count BIGINT,
    bridge_edge_extension_count BIGINT,
    bridge_duration_ms BIGINT,
    bridge_initial_input_images BIGINT,
    bridge_initial_registered_images BIGINT,
    bridge_initial_points_3d BIGINT,
    bridge_final_input_images BIGINT,
    bridge_final_registered_images BIGINT,
    bridge_final_points_3d BIGINT,
    bridge_adopted BOOLEAN,

    brush_initial_profile TEXT,
    brush_final_profile TEXT,
    brush_detected_total_memory_mb BIGINT,
    brush_max_resolution INTEGER,
    brush_total_steps BIGINT,
    brush_growth_grad_threshold DOUBLE PRECISION,
    brush_growth_select_fraction DOUBLE PRECISION,
    brush_growth_stop_iter INTEGER,
    brush_refine_every INTEGER,
    brush_max_splats INTEGER,
    brush_oom_retry_used BOOLEAN,

    final_input_images BIGINT,
    final_registered_images BIGINT,
    final_points_3d BIGINT,
    final_splat_count BIGINT,
    final_ply_size_bytes BIGINT,

    duration_probing_video_ms BIGINT,
    duration_extracting_frames_ms BIGINT,
    duration_extracting_features_ms BIGINT,
    duration_matching_ms BIGINT,
    duration_reconstructing_ms BIGINT,
    duration_validating_reconstruction_ms BIGINT,
    duration_training_splats_ms BIGINT,
    duration_exporting_ms BIGINT,

    CONSTRAINT planner_eval_install_hash_format CHECK (install_hash ~ '^[0-9a-f]{64}$'),
    CONSTRAINT planner_eval_run_hash_format CHECK (run_hash ~ '^[0-9a-f]{64}$'),
    CONSTRAINT planner_eval_schema_positive CHECK (planner_schema_version > 0),
    CONSTRAINT planner_eval_run_kind CHECK (run_kind IN ('new', 'resume')),
    CONSTRAINT planner_eval_outcome CHECK (outcome IN ('completed', 'failed')),
    CONSTRAINT planner_eval_quality CHECK (quality_preset IN ('fast', 'balanced', 'high')),
    CONSTRAINT planner_eval_input_type CHECK (input_type IN ('video', 'images')),
    CONSTRAINT planner_eval_os CHECK (os IN ('windows', 'macos', 'linux', 'unknown')),
    CONSTRAINT planner_eval_arch CHECK (arch IN ('x86_64', 'aarch64', 'unknown')),
    CONSTRAINT planner_eval_gpu_vendor CHECK (gpu_vendor IN ('nvidia', 'amd', 'intel', 'apple', 'other', 'unknown')),
    CONSTRAINT planner_eval_sfm_backend CHECK (initial_sfm_backend IS NULL OR initial_sfm_backend IN ('cpu', 'gpu')),
    CONSTRAINT planner_eval_bridge_status CHECK (
        bridge_status IS NULL OR bridge_status IN (
            'notApplicable', 'notEvaluated', 'notNeeded', 'noBudget', 'running', 'completed', 'failedRolledBack'
        )
    ),
    CONSTRAINT planner_eval_brush_initial_profile CHECK (
        brush_initial_profile IS NULL OR brush_initial_profile IN (
            'legacy', 'fast', 'balanced', 'highLow', 'highStandard', 'highLarge', 'highEmergency'
        )
    ),
    CONSTRAINT planner_eval_brush_final_profile CHECK (
        brush_final_profile IS NULL OR brush_final_profile IN (
            'legacy', 'fast', 'balanced', 'highLow', 'highStandard', 'highLarge', 'highEmergency'
        )
    ),
    CONSTRAINT planner_eval_failure_stage CHECK (
        failure_stage IS NULL OR failure_stage IN (
            'probing_video', 'extracting_frames', 'extracting_features', 'matching',
            'reconstructing', 'validating_reconstruction', 'training_splats', 'exporting', 'unknown'
        )
    ),
    CONSTRAINT planner_eval_error_code CHECK (
        error_code IS NULL OR error_code IN (
            'engine_unavailable', 'invalid_input', 'ffprobe_failed', 'ffmpeg_failed',
            'colmap_feature_failed', 'colmap_matching_failed', 'colmap_mapper_failed',
            'low_registered_images', 'brush_failed', 'brush_out_of_memory',
            'brush_device_lost', 'disk_space_low', 'io_failed', 'unknown'
        )
    ),
    CONSTRAINT planner_eval_planner_version CHECK (
        (planner_enabled AND planner_version = 'quality_v2_planner_v1')
        OR (NOT planner_enabled AND planner_version IS NULL)
    ),
    CONSTRAINT planner_eval_failure_fields CHECK (
        (outcome = 'failed' AND error_code IS NOT NULL)
        OR (outcome = 'completed' AND failure_stage IS NULL AND error_code IS NULL)
    ),
    CONSTRAINT planner_eval_nonnegative_counts CHECK (
        total_duration_ms >= 0
        AND COALESCE(source_width, 0) >= 0
        AND COALESCE(source_height, 0) >= 0
        AND COALESCE(source_item_count, 0) >= 0
        AND COALESCE(initial_selected_count, 0) >= 0
        AND COALESCE(candidate_count, 0) >= 0
        AND COALESCE(initial_sfm_input_images, 0) >= 0
        AND COALESCE(initial_sfm_registered_images, 0) >= 0
        AND COALESCE(initial_sfm_points_3d, 0) >= 0
        AND COALESCE(final_splat_count, 0) >= 0
        AND COALESCE(final_ply_size_bytes, 0) >= 0
        AND COALESCE(duration_probing_video_ms, 0) >= 0
        AND COALESCE(duration_extracting_frames_ms, 0) >= 0
        AND COALESCE(duration_extracting_features_ms, 0) >= 0
        AND COALESCE(duration_matching_ms, 0) >= 0
        AND COALESCE(duration_reconstructing_ms, 0) >= 0
        AND COALESCE(duration_validating_reconstruction_ms, 0) >= 0
        AND COALESCE(duration_training_splats_ms, 0) >= 0
        AND COALESCE(duration_exporting_ms, 0) >= 0
    )
);

CREATE INDEX IF NOT EXISTS idx_planner_eval_comparison
    ON ooosplat_planner_evaluations (
        planner_version, planner_enabled, quality_preset, input_type, event_timestamp DESC
    );

CREATE INDEX IF NOT EXISTS idx_planner_eval_bridge
    ON ooosplat_planner_evaluations (bridge_status, event_timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_planner_eval_brush_profile
    ON ooosplat_planner_evaluations (brush_final_profile, event_timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_planner_eval_outcome_error
    ON ooosplat_planner_evaluations (outcome, error_code, event_timestamp DESC);

COMMIT;
