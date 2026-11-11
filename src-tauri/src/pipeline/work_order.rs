//! The shared, fixed-order generation scheduler. Decisions use validated work,
//! never the reason a previous invocation stopped. The project state remains
//! the only persisted work order; this module creates no ticket files.

use serde::{Deserialize, Serialize};

use crate::project::PipelineStateFile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationAction {
    PrepareInputs,
    ExtractFeatures,
    MatchImages,
    Reconstruct,
    ReconcileReconstruction,
    PrepareTraining,
    Train,
    Publish,
}

/// Invocation-local validated objects. They are rebuilt from saved artifacts
/// after restart, not another set of durable completion flags.
#[derive(Debug, Default, Clone, Copy)]
pub struct Readiness {
    pub reconstruction: bool,
    pub training_dataset: bool,
    pub candidate: bool,
}

pub fn next_action(state: &PipelineStateFile, ready: Readiness) -> GenerationAction {
    // A validated candidate owns its provenance. Re-publishing it does not
    // require regenerating disposable training images or re-running engines.
    if ready.candidate {
        return GenerationAction::Publish;
    }
    let Some(inputs) = state.inputs.as_ref().filter(|inputs| inputs.prepared) else {
        return GenerationAction::PrepareInputs;
    };
    if inputs.imported_reconstruction && !state.reconstruction_complete {
        // Repair the imported artifact, never silently run SfM in its place.
        return GenerationAction::PrepareInputs;
    }
    if !inputs.imported_reconstruction {
        if !state.features_complete {
            return GenerationAction::ExtractFeatures;
        }
        if !state.matching_complete {
            return GenerationAction::MatchImages;
        }
        if !state.reconstruction_complete {
            return GenerationAction::Reconstruct;
        }
    }
    if !ready.reconstruction {
        return GenerationAction::ReconcileReconstruction;
    }
    if !ready.training_dataset {
        return GenerationAction::PrepareTraining;
    }
    GenerationAction::Train
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pipeline::{
            inputs::{GenerationInputs, InputCheckpoint},
            PipelineStage,
        },
        presets::Quality,
    };

    fn state() -> PipelineStateFile {
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.inputs = Some(InputCheckpoint::new(GenerationInputs::Materials {
            items: vec![],
        }));
        state
    }

    #[test]
    fn the_same_work_has_the_same_next_step_regardless_of_stop_reason() {
        for stage in [
            PipelineStage::Cancelled,
            PipelineStage::Failed,
            PipelineStage::Created,
        ] {
            let mut state = state();
            state.stage = stage;
            assert_eq!(
                next_action(&state, Readiness::default()),
                GenerationAction::PrepareInputs
            );
            state.inputs.as_mut().unwrap().prepared = true;
            assert_eq!(
                next_action(&state, Readiness::default()),
                GenerationAction::ExtractFeatures
            );
            state.features_complete = true;
            assert_eq!(
                next_action(&state, Readiness::default()),
                GenerationAction::MatchImages
            );
            state.matching_complete = true;
            assert_eq!(
                next_action(&state, Readiness::default()),
                GenerationAction::Reconstruct
            );
            state.reconstruction_complete = true;
            assert_eq!(
                next_action(&state, Readiness::default()),
                GenerationAction::ReconcileReconstruction
            );
            assert_eq!(
                next_action(
                    &state,
                    Readiness {
                        reconstruction: true,
                        ..Default::default()
                    }
                ),
                GenerationAction::PrepareTraining
            );
            assert_eq!(
                next_action(
                    &state,
                    Readiness {
                        reconstruction: true,
                        training_dataset: true,
                        candidate: false
                    }
                ),
                GenerationAction::Train
            );
        }
    }

    #[test]
    fn imported_reconstruction_skips_sfm_and_validated_candidate_skips_every_engine() {
        let mut state = state();
        state.inputs.as_mut().unwrap().prepared = true;
        state.inputs.as_mut().unwrap().imported_reconstruction = true;
        assert_eq!(
            next_action(&state, Readiness::default()),
            GenerationAction::PrepareInputs
        );
        state.reconstruction_complete = true;
        assert_eq!(
            next_action(&state, Readiness::default()),
            GenerationAction::ReconcileReconstruction
        );
        assert_eq!(
            next_action(
                &state,
                Readiness {
                    candidate: true,
                    ..Default::default()
                }
            ),
            GenerationAction::Publish
        );
    }
}
