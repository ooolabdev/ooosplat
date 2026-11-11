//! Local evidence that a reshoot still connects old and newly supplied views.
//! This is not a quality score or permission to retry. It never reads a source
//! project: only the derived project's saved names, model and candidate matter.

use std::{collections::BTreeSet, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{
    error::{Result, SplatError},
    project::PipelineStateFile,
    reconstruction::colmap_model::{read_registered_images, reusable_model_files},
};

pub const DISCONNECTED_MESSAGE: &str = "补拍素材未能与原场景建立联系，已停止生成。";
const MISSING_EVIDENCE: &str =
    "旧任务的历史参数或初始化数据不完整，无法安全继续。原项目和已有结果保持不变。";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProof {
    pub version: u32,
    pub source_identity: String,
    pub reconstruction_identity: String,
    pub old_registered: u64,
    pub new_registered: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_sha256: Option<String>,
}

pub fn required(state: &PipelineStateFile) -> bool {
    state.reshoot.is_some()
        || state
            .inputs
            .as_ref()
            .and_then(|inputs| inputs.execution_plan.as_ref())
            .is_some_and(|plan| plan.success.require_any_new_source)
}

fn missing_evidence() -> SplatError {
    SplatError::Process(MISSING_EVIDENCE.into())
}

fn groups(state: &PipelineStateFile) -> Result<(BTreeSet<String>, BTreeSet<String>, String)> {
    let (old, new) = if let Some(inputs) = &state.inputs {
        let old = inputs
            .sources
            .iter()
            .find(|source| source.id == "__source")
            .ok_or_else(missing_evidence)?
            .names
            .clone();
        let new = inputs
            .sources
            .iter()
            .filter(|source| source.id != "__source")
            .flat_map(|source| source.names.clone())
            .collect();
        (old, new)
    } else {
        let recipe = state
            .reshoot
            .as_ref()
            .and_then(|reshoot| reshoot.recipe.as_ref())
            .ok_or_else(missing_evidence)?;
        (recipe.source_names.clone(), recipe.new_names.clone())
    };
    let mut all = BTreeSet::new();
    for name in old.iter().chain(&new) {
        let path = Path::new(name);
        if name.is_empty()
            || name.contains(['\0', '\n', '\r', ':', '\\'])
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || !all.insert(name.clone())
        {
            return Err(missing_evidence());
        }
    }
    let old = old.into_iter().collect::<BTreeSet<_>>();
    let new = new.into_iter().collect::<BTreeSet<_>>();
    if old.is_empty() || new.is_empty() {
        return Err(missing_evidence());
    }
    let identity = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            "reshoot-connection-v1",
            &old,
            &new,
            state
                .reshoot
                .as_ref()
                .and_then(|reshoot| reshoot.snapshot_v3_identity.as_ref()),
            state
                .inputs
                .as_ref()
                .and_then(|inputs| inputs.execution_plan.as_ref()),
            state.inputs.as_ref().map(|inputs| &inputs.request),
            state
                .reshoot
                .as_ref()
                .and_then(|reshoot| reshoot.recipe.as_ref())
                .map(|recipe| (
                    recipe.actual,
                    &recipe.options.source_identity,
                    &recipe.options.input_identity,
                    &recipe.batch_id
                )),
        ))?)
    );
    Ok((old, new, identity))
}

fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

fn check_counts(old: u64, new: u64) -> Result<()> {
    if new == 0 {
        return Err(SplatError::Process(
            "补拍画面没有成功加入原项目。请确保新旧素材有足够重叠。".into(),
        ));
    }
    if old == 0 {
        return Err(SplatError::Process(DISCONNECTED_MESSAGE.into()));
    }
    Ok(())
}

/// Validate the actual final model, not a temporary mapper submodel or an
/// overall registration ratio. Counts never consume a fallback allowance.
pub fn validate_model(
    state: &PipelineStateFile,
    model: &Path,
    cancel: &CancellationToken,
) -> Result<Option<ConnectionProof>> {
    check_cancel(cancel)?;
    if !required(state) {
        return Ok(None);
    }
    let (old, new, source_identity) = groups(state)?;
    let images = read_registered_images(model)?;
    let mut registered = BTreeSet::new();
    for image in images {
        if !registered.insert(image.name) {
            return Err(missing_evidence());
        }
    }
    let old_registered = old.intersection(&registered).count() as u64;
    let new_registered = new.intersection(&registered).count() as u64;
    check_counts(old_registered, new_registered)?;
    let mut hash = Sha256::new();
    for file in reusable_model_files(model)? {
        check_cancel(cancel)?;
        hash.update(file.file_name().unwrap_or_default().as_encoded_bytes());
        hash.update(crate::video::panorama::fingerprint(&file)?);
    }
    check_cancel(cancel)?;
    Ok(Some(ConnectionProof {
        version: 1,
        source_identity,
        reconstruction_identity: format!("{:x}", hash.finalize()),
        old_registered,
        new_registered,
        candidate_sha256: None,
    }))
}

/// A previously bound proof permits dataset-independent publication. A bare
/// count or unbound proof must be re-established from the owned final model.
pub fn candidate_is_proven(state: &PipelineStateFile, candidate_sha256: &str) -> Result<bool> {
    if !required(state) {
        return Ok(true);
    }
    let (old, new, identity) = groups(state)?;
    let Some(proof) = state
        .reshoot
        .as_ref()
        .and_then(|reshoot| reshoot.connection_proof.as_ref())
    else {
        return Ok(false);
    };
    if proof.version != 1
        || proof.source_identity != identity
        || proof.reconstruction_identity.is_empty()
        || proof.old_registered > old.len() as u64
        || proof.new_registered > new.len() as u64
        || proof.candidate_sha256.as_deref() != Some(candidate_sha256)
    {
        return Ok(false);
    }
    check_counts(proof.old_registered, proof.new_registered)?;
    Ok(true)
}

pub fn bind_candidate(proof: &mut ConnectionProof, sha256: String) {
    proof.candidate_sha256 = Some(sha256);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{presets::Quality, project::ReshootState};

    fn state() -> PipelineStateFile {
        let mut state = PipelineStateFile::created(Quality::Fast);
        let recipe = serde_json::from_value(serde_json::json!({
            "version": 2,
            "options": { "condition": "geometry", "allowFallback": false,
                "sourceIdentity": "source", "inputIdentity": "input", "projection": "perspective" },
            "initial": { "reconstruction": "full", "training": "cold" },
            "actual": { "reconstruction": "full", "training": "cold" },
            "batchId": "new", "sourceNames": ["frame_0001.png"],
            "newNames": ["reshoot_new_0001.png"]
        }))
        .unwrap();
        state.reshoot = Some(ReshootState {
            recipe: Some(recipe),
            ..Default::default()
        });
        state
    }

    fn model(root: &Path, names: &[&str]) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("cameras.txt"), "1 PINHOLE 4 4 2 2 2 2\n").unwrap();
        let images = names
            .iter()
            .enumerate()
            .map(|(i, name)| format!("{} 1 0 0 0 0 0 0 1 {name}\n\n", i + 1))
            .collect::<String>();
        std::fs::write(root.join("images.txt"), images).unwrap();
        std::fs::write(root.join("points3D.txt"), "").unwrap();
    }

    #[test]
    fn disconnected_full_reshoot_stops_without_changing_recipe_or_allowances() {
        let state = state();
        let before = serde_json::to_vec(&state).unwrap();
        let root = tempfile::tempdir().unwrap();
        model(root.path(), &["reshoot_new_0001.png"]);
        let error = validate_model(&state, root.path(), &CancellationToken::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("外部进程执行失败：{DISCONNECTED_MESSAGE}")
        );
        assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    }

    #[test]
    fn only_source_images_do_not_count_as_successful_reshoot() {
        let root = tempfile::tempdir().unwrap();
        model(root.path(), &["frame_0001.png"]);
        assert!(validate_model(&state(), root.path(), &CancellationToken::new()).is_err());
    }

    #[test]
    fn a_bound_proof_survives_restart_without_the_disposable_model() {
        let mut state = state();
        let root = tempfile::tempdir().unwrap();
        model(root.path(), &["frame_0001.png", "reshoot_new_0001.png"]);
        let mut proof = validate_model(&state, root.path(), &CancellationToken::new())
            .unwrap()
            .unwrap();
        assert_eq!((proof.old_registered, proof.new_registered), (1, 1));
        assert!(!candidate_is_proven(&state, "candidate").unwrap());
        bind_candidate(&mut proof, "candidate".into());
        state.reshoot.as_mut().unwrap().connection_proof = Some(proof);
        let saved: PipelineStateFile =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        drop(root);
        assert!(candidate_is_proven(&saved, "candidate").unwrap());
        assert!(!candidate_is_proven(&saved, "different").unwrap());
    }

    #[test]
    fn changed_source_grouping_or_reconstruction_plan_invalidates_proof() {
        let mut state = state();
        let root = tempfile::tempdir().unwrap();
        model(root.path(), &["frame_0001.png", "reshoot_new_0001.png"]);
        let mut proof = validate_model(&state, root.path(), &CancellationToken::new())
            .unwrap()
            .unwrap();
        bind_candidate(&mut proof, "candidate".into());
        state.reshoot.as_mut().unwrap().connection_proof = Some(proof);
        state
            .reshoot
            .as_mut()
            .unwrap()
            .recipe
            .as_mut()
            .unwrap()
            .actual
            .reconstruction = crate::pipeline::reshoot::ReconstructionMode::Incremental;
        assert!(!candidate_is_proven(&state, "candidate").unwrap());
        state
            .reshoot
            .as_mut()
            .unwrap()
            .recipe
            .as_mut()
            .unwrap()
            .source_names = vec!["other.png".into()];
        assert!(!candidate_is_proven(&state, "candidate").unwrap());
    }

    #[test]
    fn proof_does_not_guess_missing_or_duplicate_source_groups() {
        let mut state = state();
        state
            .reshoot
            .as_mut()
            .unwrap()
            .recipe
            .as_mut()
            .unwrap()
            .new_names = vec!["frame_0001.png".into()];
        assert!(candidate_is_proven(&state, "candidate").is_err());
        state.reshoot.as_mut().unwrap().recipe = None;
        assert!(candidate_is_proven(&state, "candidate").is_err());
    }

    #[test]
    fn normal_generation_and_cancellation_are_unchanged() {
        let normal = PipelineStateFile::created(Quality::Fast);
        assert!(
            validate_model(&normal, Path::new("missing"), &CancellationToken::new())
                .unwrap()
                .is_none()
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            validate_model(&state(), Path::new("missing"), &cancel),
            Err(SplatError::Cancelled)
        ));
    }
}
