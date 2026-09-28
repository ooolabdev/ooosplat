import { createHmac } from "node:crypto";

import {
  plannerEvaluationEnvelopeSchema,
  type PlannerEvaluationEnvelope,
} from "./planner-evaluation.schema";

export function plannerRunHash(secret: string, installId: string, runId: string): string {
  return createHmac("sha256", secret)
    .update(installId, "utf8")
    .update(":", "utf8")
    .update(runId, "utf8")
    .digest("hex");
}

export function toPlannerEvaluationRow(
  input: unknown,
  installHash: string,
  hmacSecret: string,
) {
  const event: PlannerEvaluationEnvelope = plannerEvaluationEnvelopeSchema.parse(input);
  const p = event.properties;
  const frame = p.framePlan;
  const sfm = p.initialReconstruction;
  const bridge = p.bridge;
  const brush = p.brush;
  const finalResult = p.finalResult;
  const durations = p.stageDurations;

  // Raw installId and runId intentionally do not appear in the returned row.
  return {
    event_timestamp: event.timestamp,
    install_hash: installHash,
    run_hash: plannerRunHash(hmacSecret, event.installId, p.runId),
    app_version: event.appVersion,
    os: event.os,
    arch: event.arch,
    gpu_vendor: p.gpuVendor,
    planner_schema_version: p.plannerSchemaVersion,
    run_kind: p.runKind,
    outcome: p.outcome,
    planner_enabled: p.plannerEnabled,
    planner_version: p.plannerVersion,
    quality_preset: p.qualityPreset,
    input_type: p.inputType,
    failure_stage: p.failureStage,
    error_code: p.errorCode,
    total_duration_ms: p.totalDurationMs,

    source_width: frame?.sourceWidth ?? null,
    source_height: frame?.sourceHeight ?? null,
    source_item_count: frame?.sourceItemCount ?? null,
    configured_target_fps: frame?.configuredTargetFps ?? null,
    configured_candidate_fps: frame?.configuredCandidateFps ?? null,
    effective_target_fps: frame?.effectiveTargetFps ?? null,
    effective_candidate_fps: frame?.effectiveCandidateFps ?? null,
    initial_selected_count: frame?.initialSelectedCount ?? null,
    candidate_count: frame?.candidateCount ?? null,
    minimum_frame_override_applied: frame?.minimumFrameOverrideApplied ?? null,
    minimum_frame_target_unreachable: frame?.minimumFrameTargetUnreachable ?? null,

    initial_sfm_input_images: sfm?.inputImages ?? null,
    initial_sfm_registered_images: sfm?.registeredImages ?? null,
    initial_sfm_points_3d: sfm?.points3d ?? null,
    initial_sfm_backend: sfm?.backend ?? null,
    initial_sfm_allow_two_view_tracks: sfm?.allowTwoViewTracks ?? null,

    bridge_status: bridge?.status ?? null,
    bridge_trigger_ratio: bridge?.triggerRatio ?? null,
    bridge_available_budget: bridge?.availableBudget ?? null,
    bridge_requested_frames: bridge?.requestedFrames ?? null,
    bridge_added_frames: bridge?.addedFrames ?? null,
    bridge_internal_count: bridge?.internalBridgeCount ?? null,
    bridge_edge_extension_count: bridge?.edgeExtensionCount ?? null,
    bridge_duration_ms: bridge?.durationMs ?? null,
    bridge_initial_input_images: bridge?.initialInputImages ?? null,
    bridge_initial_registered_images: bridge?.initialRegisteredImages ?? null,
    bridge_initial_points_3d: bridge?.initialPoints3d ?? null,
    bridge_final_input_images: bridge?.finalInputImages ?? null,
    bridge_final_registered_images: bridge?.finalRegisteredImages ?? null,
    bridge_final_points_3d: bridge?.finalPoints3d ?? null,
    bridge_adopted: bridge?.adopted ?? null,

    brush_initial_profile: brush?.initialProfile ?? null,
    brush_final_profile: brush?.finalProfile ?? null,
    brush_detected_total_memory_mb: brush?.detectedTotalMemoryMb ?? null,
    brush_max_resolution: brush?.maxResolution ?? null,
    brush_total_steps: brush?.totalSteps ?? null,
    brush_growth_grad_threshold: brush?.growthGradThreshold ?? null,
    brush_growth_select_fraction: brush?.growthSelectFraction ?? null,
    brush_growth_stop_iter: brush?.growthStopIter ?? null,
    brush_refine_every: brush?.refineEvery ?? null,
    brush_max_splats: brush?.maxSplats ?? null,
    brush_oom_retry_used: brush?.oomRetryUsed ?? null,

    final_input_images: finalResult?.inputImages ?? null,
    final_registered_images: finalResult?.registeredImages ?? null,
    final_points_3d: finalResult?.points3d ?? null,
    final_splat_count: finalResult?.splatCount ?? null,
    final_ply_size_bytes: finalResult?.plySizeBytes ?? null,

    duration_probing_video_ms: durations.probingVideoMs,
    duration_extracting_frames_ms: durations.extractingFramesMs,
    duration_extracting_features_ms: durations.extractingFeaturesMs,
    duration_matching_ms: durations.matchingMs,
    duration_reconstructing_ms: durations.reconstructingMs,
    duration_validating_reconstruction_ms: durations.validatingReconstructionMs,
    duration_training_splats_ms: durations.trainingSplatsMs,
    duration_exporting_ms: durations.exportingMs,
  };
}

// The endpoint's database adapter should insert the returned fixed-column row with
// `ON CONFLICT (run_hash) DO NOTHING`. This makes retries idempotent without ever
// persisting the raw runId.
