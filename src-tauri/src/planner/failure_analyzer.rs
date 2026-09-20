use serde::{Deserialize, Serialize};

use super::{
    GraphDecision, ReconstructionCandidate, ReconstructionDecision, ReconstructionViability,
    ViewGraphReport,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureClass {
    FragmentedViewGraph,
    WeakTemporalCoverage,
    BridgeBottleneck,
    PoorRegistration,
    PoorGeometry,
    InvalidGeometry,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FailureAnalyzer;

impl FailureAnalyzer {
    pub fn graph_failures(
        &self,
        report: &ViewGraphReport,
        decision: GraphDecision,
    ) -> Vec<FailureClass> {
        if decision != GraphDecision::NeedRescue {
            return Vec::new();
        }
        let mut failures = Vec::new();
        if report.connected_components > 1 || report.largest_component_ratio < 0.8 {
            failures.push(FailureClass::FragmentedViewGraph);
        }
        if !report.weak_runs.is_empty() {
            failures.push(FailureClass::WeakTemporalCoverage);
        }
        if report.bridge_ratio > 0.6 {
            failures.push(FailureClass::BridgeBottleneck);
        }
        failures
    }

    pub fn reconstruction_failures(
        &self,
        candidate: &ReconstructionCandidate,
    ) -> Vec<FailureClass> {
        let mut failures = Vec::new();
        if !candidate.metrics.finite_geometry
            || candidate.viability == ReconstructionViability::NotViable
        {
            failures.push(FailureClass::InvalidGeometry);
        } else if candidate.metrics.registered_ratio < 0.60 {
            failures.push(FailureClass::PoorRegistration);
        }
        if candidate.decision == ReconstructionDecision::Critical
            || candidate
                .metrics
                .mean_reprojection_error
                .is_some_and(|value| value > 4.0)
        {
            failures.push(FailureClass::PoorGeometry);
        }
        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_graph_has_a_stable_failure_class() {
        let report = ViewGraphReport {
            connected_components: 3,
            largest_component_ratio: 0.5,
            ..ViewGraphReport::default()
        };
        assert!(FailureAnalyzer
            .graph_failures(&report, GraphDecision::NeedRescue)
            .contains(&FailureClass::FragmentedViewGraph));
    }
}
