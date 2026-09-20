use super::{CapturePrior, GraphDecision, MapperBackend, MapperPlan, ViewGraphReport};

#[derive(Debug, Clone, Copy, Default)]
pub struct MapperSelector;

impl MapperSelector {
    pub fn select(
        &self,
        prior: &CapturePrior,
        graph: &ViewGraphReport,
        graph_decision: GraphDecision,
    ) -> MapperPlan {
        let dense_redundant = graph.largest_component_ratio >= 0.92
            && graph.two_core_ratio >= 0.58
            && graph.bridge_ratio <= 0.28
            && graph.long_range_edge_ratio >= 0.08;
        let backend = if graph_decision != GraphDecision::NeedRescue && dense_redundant {
            MapperBackend::Global
        } else {
            MapperBackend::Incremental
        };
        let mut reasons = vec![if backend == MapperBackend::Global {
            "dense_redundant_view_graph"
        } else {
            "chain_or_fragile_view_graph"
        }
        .into()];
        if prior.loop_prior > 0.6 {
            reasons.push("capture_loop_prior_supports_global".into());
        }
        MapperPlan {
            backend,
            calibrate_view_graph: backend == MapperBackend::Global,
            reason_codes: reasons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_evidence_dominates_capture_prior() {
        let prior = CapturePrior {
            loop_prior: 1.0,
            ..CapturePrior::default()
        };
        let graph = ViewGraphReport {
            largest_component_ratio: 1.0,
            two_core_ratio: 0.0,
            bridge_ratio: 1.0,
            ..ViewGraphReport::default()
        };
        assert_eq!(
            MapperSelector
                .select(&prior, &graph, GraphDecision::Warning)
                .backend,
            MapperBackend::Incremental
        );
    }
}
