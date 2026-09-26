use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    presets::QualityPreset, reconstruction::validator::ReconstructionReport, video::FramePlan,
};

pub const PLANNER_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureType {
    ObjectOrbit,
    SceneWalkthrough,
    Turntable,
    UnorderedPhotos,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MotionLevel {
    Low,
    #[default]
    Medium,
    High,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CapturePrior {
    pub capture_type: CaptureType,
    pub temporal_order_confidence: f32,
    pub loop_prior: f32,
    pub motion_level: MotionLevel,
    pub motion_variance: f32,
    pub blur_level: f32,
    pub exposure_variance: f32,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameCandidate {
    pub frame_index: i64,
    pub timestamp: f64,
    pub sharpness_score: f32,
    pub motion_score: f32,
    pub view_change_score: f32,
    pub exposure_score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PairingStrategy {
    Exhaustive,
    Sequential,
    SequentialWithLoopClosure,
    Prefilter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingPlan {
    pub strategy: PairingStrategy,
    pub sequential_overlap: u32,
    pub prefilter_neighbors: u32,
    pub estimated_pairs: u64,
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MapperBackend {
    Incremental,
    Global,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapperPlan {
    pub backend: MapperBackend,
    pub calibrate_view_graph: bool,
    pub reason_codes: Vec<String>,
}

impl MapperPlan {
    /// Production P0 deliberately keeps mapper choice deterministic. The
    /// Global backend remains available to explicit experiments only.
    pub fn production_incremental() -> Self {
        Self {
            backend: MapperBackend::Incremental,
            calibrate_view_graph: false,
            reason_codes: vec!["production_incremental_only".into()],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GraphDecision {
    Healthy,
    Warning,
    NeedRescue,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WeakRegion {
    pub start_image_id: u32,
    pub end_image_id: u32,
    pub severity: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bottleneck {
    pub image_id_a: u32,
    pub image_id_b: u32,
    pub severity: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ViewGraphReport {
    pub image_count: usize,
    pub connected_images: usize,
    pub connected_components: usize,
    pub largest_component_ratio: f32,
    pub median_degree: f32,
    pub normalized_degree: f32,
    pub two_core_ratio: f32,
    pub bridge_ratio: f32,
    pub temporal_edge_ratio: f32,
    pub long_range_edge_ratio: f32,
    pub median_inliers: f32,
    pub weak_runs: Vec<WeakRegion>,
    pub bottlenecks: Vec<Bottleneck>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReconstructionDecision {
    Pass,
    #[default]
    Warning,
    NeedRescue,
    Critical,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReconstructionViability {
    Viable,
    DegradedButViable,
    #[default]
    NotViable,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReconstructionMetrics {
    pub input_images: u64,
    pub registered_images: u64,
    pub registered_ratio: f64,
    pub points_3d: u64,
    pub observations: u64,
    pub mean_track_length: Option<f64>,
    pub mean_observations_per_image: Option<f64>,
    pub mean_reprojection_error: Option<f64>,
    pub model_count: u32,
    pub finite_geometry: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconstructionCandidate {
    pub id: String,
    pub mapper: MapperBackend,
    pub model_path: PathBuf,
    pub metrics: ReconstructionMetrics,
    pub decision: ReconstructionDecision,
    pub viability: ReconstructionViability,
    pub rescue_round: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GeometryScreeningDecision {
    #[default]
    NoProbe,
    ProbeRecommended,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeometryImageMetric {
    pub image_id: u32,
    pub source_frame_index: u64,
    pub timestamp_seconds: Option<f64>,
    pub detected_features: u64,
    pub observed_points: u64,
    pub triangulation_ratio: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WeakGeometryInterval {
    pub start_source_frame_index: u64,
    pub end_source_frame_index: u64,
    pub start_timestamp: Option<f64>,
    pub end_timestamp: Option<f64>,
    pub image_count: usize,
    pub median_observed_points: f64,
    pub median_triangulation_ratio: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeometryScreeningReport {
    pub threshold_profile: String,
    pub registered_images: usize,
    pub points_3d: u64,
    pub observations: u64,
    pub mean_track_length: f64,
    pub point_diversity_ratio: f64,
    pub median_triangulation_ratio: f64,
    pub p25_triangulation_ratio: f64,
    pub minimum_triangulation_ratio: f64,
    pub median_observed_points: f64,
    pub weak_geometry_intervals: Vec<WeakGeometryInterval>,
    pub per_image: Vec<GeometryImageMetric>,
    pub triangulation_underfilled: bool,
    pub track_redundancy_high: bool,
    pub continuous_weak_region: bool,
    pub decision: GeometryScreeningDecision,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GeometryProbeStatus {
    #[default]
    NotEvaluated,
    NotNeeded,
    RecommendedButNoBudget,
    Running,
    Completed,
    FailedRolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GeometryProbeReason {
    TriangulationUnderfilled,
    TrackRedundancy,
    ContinuousWeakRegion,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeometryProbeMetrics {
    pub triggered_reasons: Vec<GeometryProbeReason>,
    pub requested_additional_frames: usize,
    pub actual_additional_frames: usize,
    pub baseline_points: u64,
    pub probe_points: Option<u64>,
    pub point_gain_ratio: Option<f64>,
    pub baseline_observations: u64,
    pub probe_observations: Option<u64>,
    pub observation_gain_ratio: Option<f64>,
    pub baseline_track_length: Option<f64>,
    pub probe_track_length: Option<f64>,
    pub baseline_reprojection_error: Option<f64>,
    pub probe_reprojection_error: Option<f64>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlannerRecoveryMode {
    #[default]
    Normal,
    SuccessRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RescueAction {
    FrameBackfill,
    ExpandSequential,
    Prefilter,
    LocalExhaustive,
    SfmEscalation,
    AlternateMapper,
    LegacySafeFallback,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RescueRecord {
    pub round: u32,
    pub mode: PlannerRecoveryMode,
    pub action: RescueAction,
    pub effective: Option<bool>,
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuccessRecoveryPolicy {
    pub max_rounds: u32,
    pub allow_frame_backfill_beyond_quality: bool,
    pub allow_sfm_escalation_beyond_quality: bool,
    pub allow_pairing_escalation_beyond_quality: bool,
    pub allow_alternate_mapper: bool,
    pub allow_legacy_safe_fallback: bool,
    pub allow_full_frame_backfill: bool,
    pub max_recovery_fps: Option<f64>,
    #[serde(default = "default_recovery_image_size")]
    pub max_sfm_image_size: u32,
    #[serde(default = "default_recovery_features")]
    pub max_sfm_features: u32,
}

const fn default_recovery_image_size() -> u32 {
    4_096
}

const fn default_recovery_features() -> u32 {
    32_768
}

impl Default for SuccessRecoveryPolicy {
    fn default() -> Self {
        Self {
            max_rounds: 4,
            allow_frame_backfill_beyond_quality: true,
            allow_sfm_escalation_beyond_quality: true,
            allow_pairing_escalation_beyond_quality: true,
            allow_alternate_mapper: false,
            allow_legacy_safe_fallback: true,
            allow_full_frame_backfill: true,
            max_recovery_fps: Some(15.0),
            max_sfm_image_size: default_recovery_image_size(),
            max_sfm_features: default_recovery_features(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlannerCheckpoint {
    pub planner_enabled: bool,
    pub planner_version: u32,
    pub quality_budget_snapshot: Option<QualityPreset>,
    pub success_recovery_policy_snapshot: Option<SuccessRecoveryPolicy>,
    pub capture_prior: Option<CapturePrior>,
    pub frame_plan: Option<FramePlan>,
    pub actual_selected_frames: Vec<u64>,
    pub pairing_plan: Option<PairingPlan>,
    pub pairing_actual: Option<PairingStrategy>,
    pub view_graph_report: Option<ViewGraphReport>,
    pub graph_decision: Option<GraphDecision>,
    pub mapper_plan: Option<MapperPlan>,
    pub mapper_actual: Option<MapperBackend>,
    pub reconstruction_candidates: Vec<ReconstructionCandidate>,
    pub best_reconstruction_id: Option<String>,
    pub reconstruction_report: Option<ReconstructionReport>,
    pub rescue_history: Vec<RescueRecord>,
    pub recovery_mode: PlannerRecoveryMode,
    pub normal_budget_exhausted: bool,
    pub success_recovery_entered: bool,
    pub budget_overridden_for_success: bool,
    pub normal_duration_ms: u64,
    pub recovery_duration_ms: u64,
    pub geometry_screening_complete: bool,
    pub geometry_screening_report: Option<GeometryScreeningReport>,
    pub geometry_probe_attempted: bool,
    pub geometry_probe_added_frames: usize,
    pub geometry_probe_frame_indices: Vec<u64>,
    pub geometry_probe_before_candidate_id: Option<String>,
    pub geometry_probe_after_candidate_id: Option<String>,
    pub geometry_probe_status: GeometryProbeStatus,
    pub geometry_probe_metrics: Option<GeometryProbeMetrics>,
}

impl PlannerCheckpoint {
    pub fn new(quality: QualityPreset, policy: SuccessRecoveryPolicy) -> Self {
        Self {
            planner_enabled: true,
            planner_version: PLANNER_VERSION,
            quality_budget_snapshot: Some(quality),
            success_recovery_policy_snapshot: Some(policy),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Quality;

    #[test]
    fn older_recovery_policy_snapshots_receive_robust_sfm_ceilings() {
        let policy: SuccessRecoveryPolicy = serde_json::from_str(
            r#"{
                "maxRounds": 4,
                "allowFrameBackfillBeyondQuality": true,
                "allowSfmEscalationBeyondQuality": true,
                "allowPairingEscalationBeyondQuality": true,
                "allowAlternateMapper": true,
                "allowLegacySafeFallback": true,
                "allowFullFrameBackfill": true,
                "maxRecoveryFps": 15.0
            }"#,
        )
        .unwrap();

        assert_eq!(policy.max_sfm_image_size, 4_096);
        assert_eq!(policy.max_sfm_features, 32_768);
    }

    #[test]
    fn production_mapper_plan_is_incremental_without_global_calibration() {
        let plan = MapperPlan::production_incremental();
        assert_eq!(plan.backend, MapperBackend::Incremental);
        assert!(!plan.calibrate_view_graph);
        assert_eq!(
            plan.reason_codes,
            vec!["production_incremental_only".to_string()]
        );
        assert!(!SuccessRecoveryPolicy::default().allow_alternate_mapper);
    }

    #[test]
    fn geometry_probe_frame_list_round_trips_for_exact_resume() {
        let mut checkpoint =
            PlannerCheckpoint::new(Quality::High.budget(), SuccessRecoveryPolicy::default());
        checkpoint.geometry_screening_complete = true;
        checkpoint.geometry_probe_attempted = true;
        checkpoint.geometry_probe_status = GeometryProbeStatus::Running;
        checkpoint.geometry_probe_frame_indices = vec![12, 48, 96, 144];

        let encoded = serde_json::to_vec(&checkpoint).unwrap();
        let restored: PlannerCheckpoint = serde_json::from_slice(&encoded).unwrap();
        assert!(restored.geometry_screening_complete);
        assert!(restored.geometry_probe_attempted);
        assert_eq!(restored.geometry_probe_status, GeometryProbeStatus::Running);
        assert_eq!(restored.geometry_probe_frame_indices, vec![12, 48, 96, 144]);
    }
}
