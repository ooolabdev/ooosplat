use super::{GraphDecision, ViewGraphReport};

#[derive(Debug, Clone, Copy)]
pub struct GraphQualityThresholds {
    pub minimum_largest_component_ratio: f32,
    pub warning_largest_component_ratio: f32,
    pub minimum_two_core_ratio: f32,
    pub maximum_bridge_ratio: f32,
    pub minimum_median_inliers: f32,
}

impl Default for GraphQualityThresholds {
    fn default() -> Self {
        Self {
            minimum_largest_component_ratio: 0.70,
            warning_largest_component_ratio: 0.88,
            minimum_two_core_ratio: 0.35,
            maximum_bridge_ratio: 0.62,
            minimum_median_inliers: 20.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GraphQualityGate {
    pub thresholds: GraphQualityThresholds,
}

impl GraphQualityGate {
    pub fn decide(&self, report: &ViewGraphReport) -> GraphDecision {
        let thresholds = self.thresholds;
        if report.image_count == 0
            || report.connected_images < 2
            || report.largest_component_ratio < thresholds.minimum_largest_component_ratio
            || report.median_inliers < thresholds.minimum_median_inliers
        {
            GraphDecision::NeedRescue
        } else if report.largest_component_ratio < thresholds.warning_largest_component_ratio
            || report.two_core_ratio < thresholds.minimum_two_core_ratio
            || report.bridge_ratio > thresholds.maximum_bridge_ratio
            || !report.weak_runs.is_empty()
        {
            GraphDecision::Warning
        } else {
            GraphDecision::Healthy
        }
    }
}
