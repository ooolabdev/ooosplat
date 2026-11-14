//! GPS is a separate, validated refinement. The original visual reconstruction
//! remains the authoritative fallback and is never modified by a GPS attempt.
use super::*;
use crate::project::manager::atomic_write_json;
use crate::reconstruction::gps::{self, Checkpoint, Manifest, Record, Similarity};
use std::ffi::OsString;

fn inputs(metadata: &ProjectMetadata, state: &PipelineStateFile) -> Result<Vec<(String, PathBuf)>> {
    if let Some(mapping) = state
        .panorama
        .as_ref()
        .and_then(|p| p.image_sources.as_ref())
    {
        return Ok(mapping
            .iter()
            .map(|m| {
                (
                    m.prepared_name.clone(),
                    m.original
                        .as_ref()
                        .map(|o| o.path.clone())
                        .unwrap_or_else(|| metadata.source_path.join(&m.source_name)),
                )
            })
            .collect());
    }
    crate::video::list_images(&metadata.source_path)?
        .iter()
        .enumerate()
        .map(|(i, p)| Ok((crate::video::normalized_image_name(i, p)?, p.clone())))
        .collect()
}
pub(super) fn generation_inputs(
    metadata: &ProjectMetadata,
    state: &PipelineStateFile,
) -> Result<Vec<(String, PathBuf)>> {
    inputs(metadata, state)
}

/// Old records are read locally from the source project, never copied to IPC.
pub(super) fn reshoot_records(
    metadata: &ProjectMetadata,
    paths: &ProjectPaths,
    recipe: &crate::pipeline::reshoot::ReshootRecipe,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(Vec<Record>, Option<crate::video::gps::GpsFix>)> {
    let mut records = Vec::new();
    let mut origin = None;
    if let Some(source) = &metadata.reshoot {
        let previous = source.source_project_path.join("work/gps/manifest.json");
        let state = std::fs::read(source.source_project_path.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<PipelineStateFile>(&b).ok());
        if let (Some(checkpoint), Ok(bytes)) = (state.and_then(|s| s.gps), std::fs::read(previous))
        {
            if let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) {
                if manifest.version == gps::POLICY_VERSION
                    && checkpoint.fingerprint == gps::fingerprint(&manifest.records)?
                    && checkpoint.manifest_identity.as_deref()
                        == Some(gps::manifest_identity(&manifest)?.as_str())
                {
                    if checkpoint.status == "complete" || checkpoint.reference_valid {
                        origin = manifest.origin;
                    }
                    records = manifest
                        .records
                        .into_iter()
                        .filter(|r| recipe.source_names.contains(&r.name))
                        .collect();
                }
            }
        }
    }
    if records.is_empty() {
        let candidates = recipe
            .source_names
            .iter()
            .map(|n| (n.clone(), paths.frames.join(n)))
            .collect::<Vec<_>>();
        records = gps::collect(&candidates, cancel)?;
    }
    let files = crate::video::list_images(&metadata.source_path)?;
    if files.len() != recipe.new_names.len() {
        return Err(SplatError::Process(
            "素材文件已发生变化，请重新分析后再生成".into(),
        ));
    }
    records.extend(gps::collect(
        &recipe
            .new_names
            .iter()
            .cloned()
            .zip(files)
            .collect::<Vec<_>>(),
        cancel,
    )?);
    Ok((records, origin))
}

pub(super) fn model_identity(model: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let binary = ["cameras.bin", "images.bin", "points3D.bin"];
    let files = if binary.iter().all(|name| model.join(name).is_file()) {
        binary.iter().map(|name| model.join(name)).collect()
    } else {
        crate::reconstruction::colmap_model::reusable_model_files(model)?
    };
    for file in files {
        hash.update(crate::video::panorama::fingerprint(&file)?);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn native_alignment_rejected(error: &SplatError) -> bool {
    matches!(error,SplatError::EngineProcess(f) if !f.termination.abnormal() && f.termination.exit_code==Some(1)
        && f.classification.kind!=Some("mapper_storage")
        && ["alignment failed","not enough","insufficient"].iter().any(|m|f.detail.to_ascii_lowercase().contains(m)))
}

struct GpsMapperOptions<'a> {
    paths: &'a ProjectPaths,
    visual: &'a Path,
    base: Option<&'a Path>,
    fixed: Option<&'a Path>,
    georeference: bool,
    allow_two_view_tracks: bool,
}

fn gps_command_observer(args: &[OsString]) -> ObserverMode {
    match args.first().and_then(|value| value.to_str()) {
        Some("spatial_matcher") => ObserverMode::Matching,
        Some("mapper" | "pose_prior_mapper") => ObserverMode::Mapper,
        _ => ObserverMode::Generic,
    }
}

/// Keep the GPS reconstruction request independent of BA backend selection.
/// Both initial execution and a Ceres compatibility retry use this same list.
fn gps_mapper_args(options: GpsMapperOptions<'_>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        if options.georeference {
            "pose_prior_mapper"
        } else {
            "mapper"
        }
        .into(),
        "--database_path".into(),
        "gps-v1/database.db".into(),
        "--image_path".into(),
        "../colmap_frames".into(),
        "--input_path".into(),
        options.base.unwrap_or(options.visual).into(),
        "--output_path".into(),
        "gps-v1/native".into(),
        "--Mapper.ba_refine_principal_point".into(),
        "0".into(),
    ];
    if options.allow_two_view_tracks {
        args.extend(["--Mapper.tri_ignore_two_view_tracks".into(), "0".into()]);
    }
    if options.georeference {
        args.extend([
            "--use_robust_loss_on_prior_position".into(),
            "1".into(),
            "--prior_position_loss_scale".into(),
            "7.815".into(),
        ]);
    }
    if options.base.is_some() {
        args.extend(["--Mapper.fix_existing_frames".into(), "1".into()]);
    }
    if let Some(fixed) = options.fixed {
        args.extend(["--Mapper.constant_camera_list_path".into(), fixed.into()]);
    }
    if options.paths.colmap.join("panorama-camera.json").is_file() {
        args.extend([
            "--Mapper.init_min_tri_angle".into(),
            "1".into(),
            "--Mapper.init_max_forward_motion".into(),
            "1".into(),
        ]);
    }
    args
}

impl PipelineRunner {
    pub(super) fn gps_log(&self, message: impl Into<String>, warning: bool) {
        self.events.send(
            PipelineStage::Reconstructing,
            Some(PipelineEngine::System),
            EventKind::Log,
            if warning {
                EventLevel::Warning
            } else {
                EventLevel::Info
            },
            None,
            true,
            message.into(),
            None,
            None,
            None,
        );
    }
    /// Returns a private derived model, or None for the unmodified visual path.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn apply_gps(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        records: Vec<Record>,
        inherited_origin: Option<crate::video::gps::GpsFix>,
        visual_model: &Path,
        reshoot_base: Option<&Path>,
        fixed_cameras: Option<&Path>,
    ) -> Result<Option<PathBuf>> {
        if self.process_manager.child_token().is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let Some(checkpoint) = state.gps.as_ref() else {
            return Ok(None);
        };
        if checkpoint.version != gps::POLICY_VERSION {
            return Ok(None);
        }
        if checkpoint.database_recovery_visual_only && checkpoint.status == "fallback" {
            self.gps_log(gps::FALLBACK_MESSAGE, true);
            return Ok(None);
        }
        let fingerprint = gps::fingerprint(&records)?;
        // Priors alone do not identify the visual coordinate system or geometry.
        let input_identity = if records.is_empty() {
            None
        } else {
            use sha2::{Digest, Sha256};
            Some(format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&(
                    model_identity(visual_model)?,
                    &inherited_origin
                ))?)
            ))
        };
        let root = paths.work.join("gps");
        let output = paths.colmap.join("gps-v1/model");
        if checkpoint.fingerprint == fingerprint && checkpoint.input_identity == input_identity {
            if matches!(checkpoint.status.as_str(), "absent" | "fallback") {
                if checkpoint.status == "fallback" {
                    self.gps_log(gps::FALLBACK_MESSAGE, true);
                }
                return Ok(None);
            }
            if matches!(checkpoint.status.as_str(), "complete" | "matching")
                && model_identity(&output).ok() == checkpoint.output_identity
                && checkpoint.output_identity.is_some()
            {
                self.gps_log(
                    "[GPS] Reusing validated local GPS reconstruction cache",
                    false,
                );
                return Ok(Some(output));
            }
        }
        let inherited_reference = inherited_origin.is_some() && reshoot_base.is_some();
        let origin = inherited_origin.or_else(|| {
            records
                .iter()
                .find(|r| r.fix.altitude.is_some())
                .or_else(|| records.first())
                .map(|r| r.fix.clone())
        });
        let manifest = Manifest {
            version: gps::POLICY_VERSION,
            records,
            origin,
        };
        std::fs::create_dir_all(&root)?;
        // Persist refusal to reuse old training before replacing any GPS output.
        state.brush_complete = false;
        state.validated_output = None;
        state.gps = Some(Checkpoint {
            fingerprint,
            input_identity,
            manifest_identity: Some(gps::manifest_identity(&manifest)?),
            reference_valid: inherited_reference,
            status: "pending".into(),
            ..Checkpoint::pending()
        });
        manager.write_state(&paths.state, state).await?;
        atomic_write_json(&root.join("manifest.json"), &manifest).await?;
        if manifest.records.is_empty() {
            state.gps.as_mut().unwrap().status = "absent".into();
            manager.write_state(&paths.state, state).await?;
            return Ok(None);
        }
        self.gps_log(gps::DETECTED_MESSAGE, false);
        let Some(origin) = &manifest.origin else {
            return self
                .gps_fallback(
                    manager,
                    paths,
                    state,
                    "missing compatible height observations",
                )
                .await;
        };
        let georeference = gps::usable(&manifest.records, origin);
        if !georeference && !gps::horizontal_usable(&manifest.records, origin) {
            return self
                .gps_fallback(
                    manager,
                    paths,
                    state,
                    "insufficient GPS geometry or positioning accuracy",
                )
                .await;
        }
        // Native capability has been exercised on Windows x64, including ERP.
        // Other hosts stay on the existing path until their real acceptance.
        if !cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            return self
                .gps_fallback(
                    manager,
                    paths,
                    state,
                    "platform has not passed native GPS acceptance",
                )
                .await;
        }
        let outcome = self
            .gps_attempt(
                paths,
                &manifest,
                visual_model,
                reshoot_base,
                fixed_cameras,
                georeference,
                state.generation_policy.is_some() && state.actual_transparency == Some(true),
                state.ba_policy_version,
            )
            .await?;
        let Some((model, transform)) = outcome else {
            return self
                .gps_fallback(
                    manager,
                    paths,
                    state,
                    "native GPS alignment did not pass validation",
                )
                .await;
        };
        let output_identity = model_identity(&model)?;
        let checkpoint = state.gps.as_mut().unwrap();
        checkpoint.status = if transform.is_some() {
            "complete"
        } else {
            "matching"
        }
        .into();
        checkpoint.reference_valid |= transform.is_some();
        checkpoint.transform = transform;
        checkpoint.output_identity = Some(output_identity);
        manager.write_state(&paths.state, state).await?;
        self.gps_log(if georeference {"[GPS] Validated local metric georeference; north and scale are estimates, not survey guarantees"} else {"[GPS] GPS used only for matching candidates; no new geographic orientation or metric scale"},false);
        Ok(Some(model))
    }
    async fn gps_fallback(
        &self,
        manager: &ProjectManager,
        paths: &ProjectPaths,
        state: &mut PipelineStateFile,
        reason: &str,
    ) -> Result<Option<PathBuf>> {
        state.gps.as_mut().unwrap().status = "fallback".into();
        manager.write_state(&paths.state, state).await?;
        self.gps_log(
            format!("[GPS] Reverting to the original non-GPS reconstruction: {reason}"),
            true,
        );
        self.gps_log(gps::FALLBACK_MESSAGE, true);
        Ok(None)
    }
    #[allow(clippy::too_many_arguments)]
    async fn gps_command(&self, paths: &ProjectPaths, args: Vec<OsString>) -> Result<()> {
        let mode = gps_command_observer(&args);
        colmap::run_colmap(
            &self.engines.colmap,
            args,
            &paths.colmap,
            paths.logs.join("gps.log"),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::Reconstructing,
                PipelineEngine::Colmap,
                None,
                mode,
            )),
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn gps_attempt(
        &self,
        paths: &ProjectPaths,
        manifest: &Manifest,
        visual: &Path,
        base: Option<&Path>,
        fixed: Option<&Path>,
        georeference: bool,
        allow_two_view_tracks: bool,
        ba_policy_version: Option<u32>,
    ) -> Result<Option<(PathBuf, Option<Similarity>)>> {
        let cancel = self.process_manager.child_token();
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let origin = manifest.origin.as_ref().unwrap();
        // Probe the actual executable, not merely its version number.
        for command in [
            if georeference {
                "pose_prior_mapper"
            } else {
                "mapper"
            },
            "spatial_matcher",
            "model_aligner",
            "model_transformer",
        ] {
            let process = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                self.process_manager.run(crate::process::ProcessSpec {
                    executable: self.engines.colmap.clone(),
                    args: vec![command.into(), "-h".into()],
                    working_directory: Some(paths.colmap.clone()),
                    log_path: None,
                    observer: None,
                }),
            )
            .await
            .map_err(|_| SplatError::Process("GPS capability probe timed out".into()))??;
            if !process.success {
                if process.termination.abnormal() {
                    return Err(SplatError::EngineProcess(Box::new(
                        crate::failure::EngineProcessFailure::from_output(
                            PipelineEngine::Colmap,
                            command,
                            &process,
                        ),
                    )));
                }
                return Ok(None);
            }
        }
        let attempt = paths.colmap.join("gps-v1");
        reset_directory(&attempt).await?;
        let db_path = attempt.join("database.db");
        let source = paths.colmap.join("database.db");
        let destination = db_path.clone();
        let backup_cancel = cancel.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let io = |e: rusqlite::Error| SplatError::Io(std::io::Error::other(e.to_string()));
            let database = rusqlite::Connection::open_with_flags(
                source,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(io)?;
            let mut destination = rusqlite::Connection::open(destination).map_err(io)?;
            let backup = rusqlite::backup::Backup::new(&database, &mut destination).map_err(io)?;
            let mut busy_since = None;
            loop {
                if backup_cancel.is_cancelled() {
                    return Err(SplatError::Cancelled);
                }
                match backup.step(128).map_err(io)? {
                    rusqlite::backup::StepResult::Done => break,
                    rusqlite::backup::StepResult::More => busy_since = None,
                    _ => {
                        if busy_since
                            .get_or_insert_with(std::time::Instant::now)
                            .elapsed()
                            .as_secs()
                            >= 30
                        {
                            return Err(SplatError::Io(std::io::Error::new(
                                std::io::ErrorKind::WouldBlock,
                                "GPS database backup remained locked",
                            )));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| SplatError::Process(e.to_string()))??;
        match gps::install_priors(&db_path, &manifest.records, origin, true) {
            Ok(_) => {}
            Err(SplatError::UnsupportedEngine(_)) => return Ok(None),
            Err(e) => return Err(e),
        }
        // Supplement, never replace, the user's sequential/exhaustive pairs.
        self.gps_command(
            paths,
            vec![
                "spatial_matcher".into(),
                "--database_path".into(),
                "gps-v1/database.db".into(),
                "--SpatialMatching.ignore_z".into(),
                "1".into(),
                "--SpatialMatching.max_num_neighbors".into(),
                "50".into(),
                "--SpatialMatching.max_distance".into(),
                "1000000000".into(),
                "--FeatureMatching.use_gpu".into(),
                "0".into(),
            ],
        )
        .await?;
        gps::install_priors(&db_path, &manifest.records, origin, false)?;
        let mut native = attempt.join("native");
        std::fs::create_dir_all(&native)?;
        let args = gps_mapper_args(GpsMapperOptions {
            paths,
            visual,
            base,
            fixed,
            georeference,
            allow_two_view_tracks,
        });
        // Only Mapper enters the common BA selector. The absolute database
        // path is used for statistics; command arguments retain their existing
        // relative paths and the same working directory. A compatibility
        // retry may reset `native`, never the GPS database, priors or visual
        // reconstruction in the rest of this attempt directory.
        let result = colmap::run_mapper_with_ba(
            &self.engines.colmap,
            &db_path,
            args,
            &native,
            &paths.colmap,
            paths.logs.join("gps.log"),
            &self.process_manager,
            Some(self.process_observer(
                PipelineStage::Reconstructing,
                PipelineEngine::Colmap,
                None,
                ObserverMode::Mapper,
            )),
            colmap::MapperBaPolicy {
                policy_version: ba_policy_version,
                automatic_optimization: true,
                gpu_index: self.current_acceleration().and_then(|a| a.gpu_index()),
                panorama: paths.colmap.join("panorama-camera.json").is_file(),
                pose_priors: georeference,
                ..Default::default()
            },
        )
        .await;
        // Consider the GPS-to-visual fallback only after the shared executor
        // has exhausted its one same-scheme Caspar-to-Ceres fallback. The
        // final crash, I/O error or cancellation remains its true failure.
        if let Err(error) = result {
            if native_alignment_rejected(&error) {
                return Ok(None);
            }
            return Err(error);
        }
        if !native.join("images.bin").is_file() {
            return Ok(None);
        }
        if let Some(base) = base {
            if !super::reshoot_runner::coordinates_match(base, &native)? {
                // Pose-prior mapping may change the global gauge even with
                // fixed existing frames. Undo ONLY a common similarity and
                // then demand the original cameras/poses match strictly.
                let references = crate::video::panorama::read_poses(base)?
                    .iter()
                    .map(|p| {
                        let [x, y, z] = gps::center(p);
                        format!("{} {x} {y} {z}", p.name)
                    })
                    .collect::<Vec<_>>();
                if references.len() < 5 {
                    return Ok(None);
                }
                std::fs::write(attempt.join("fixed-reference.txt"), references.join("\n"))?;
                let restored = attempt.join("restored");
                std::fs::create_dir_all(&restored)?;
                let result = self
                    .gps_command(
                        paths,
                        vec![
                            "model_aligner".into(),
                            "--input_path".into(),
                            native.as_os_str().into(),
                            "--output_path".into(),
                            restored.as_os_str().into(),
                            "--ref_images_path".into(),
                            attempt.join("fixed-reference.txt").into_os_string(),
                            "--ref_is_gps".into(),
                            "0".into(),
                            "--alignment_type".into(),
                            "custom".into(),
                            "--min_common_images".into(),
                            "5".into(),
                            "--alignment_max_error".into(),
                            "0.00001".into(),
                        ],
                    )
                    .await;
                if let Err(error) = result {
                    if native_alignment_rejected(&error) {
                        return Ok(None);
                    }
                    return Err(error);
                }
                if !super::reshoot_runner::coordinates_match(base, &restored)? {
                    return Ok(None);
                }
                native = restored;
            }
        }
        let poses = crate::video::panorama::read_poses(&native)?;
        if poses.len() < crate::video::panorama::read_poses(visual)?.len() {
            return Ok(None);
        }
        if !georeference {
            ReconstructionValidator::validate(&paths.frames, &native)?;
            let output = attempt.join("model");
            std::fs::rename(native, &output)?;
            return Ok(Some((output, None)));
        }
        let names: HashSet<_> = poses.iter().map(|p| p.name.as_str()).collect();
        let registered: Vec<_> = manifest
            .records
            .iter()
            .filter(|r| names.contains(r.name.as_str()))
            .cloned()
            .collect();
        if !gps::usable(&registered, origin) {
            return Ok(None);
        }
        let references = registered
            .iter()
            .filter(|r| r.fix.altitude.is_some())
            .map(|r| {
                let [x, y, z] = r.fix.enu(origin);
                format!("{} {x} {y} {z}", r.name)
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(attempt.join("reference.txt"), references)?;
        let threshold = registered
            .iter()
            .map(|r| r.fix.horizontal_std.max(r.fix.vertical_std))
            .fold(1., f64::max)
            * 3.;
        let enu = attempt.join("enu");
        std::fs::create_dir_all(&enu)?;
        if let Err(error) = self
            .gps_command(
                paths,
                vec![
                    "model_aligner".into(),
                    "--input_path".into(),
                    native.as_os_str().into(),
                    "--output_path".into(),
                    "gps-v1/enu".into(),
                    "--ref_images_path".into(),
                    "gps-v1/reference.txt".into(),
                    "--ref_is_gps".into(),
                    "0".into(),
                    "--alignment_type".into(),
                    "custom".into(),
                    "--min_common_images".into(),
                    "5".into(),
                    "--alignment_max_error".into(),
                    threshold.to_string().into(),
                    "--transform_path".into(),
                    "gps-v1/alignment.txt".into(),
                ],
            )
            .await
        {
            if native_alignment_rejected(&error) {
                return Ok(None);
            }
            return Err(error);
        }
        if !gps::verify_alignment(&enu, &manifest.records, origin)? {
            return Ok(None);
        }
        let Some(alignment) = Similarity::read(&attempt.join("alignment.txt")) else {
            return Ok(None);
        };
        let axes = Similarity::enu_to_ply();
        if base.is_some() && alignment.then(axes).engine_transform().validate().is_err() {
            return Ok(None);
        }
        // Sim3d::FromFile in the bundled COLMAP reads eight values, not a matrix.
        let half = std::f64::consts::FRAC_1_SQRT_2;
        std::fs::write(
            attempt.join("axes.txt"),
            format!("1 0 0 {half} {} 0 0 0\n", -half),
        )?;
        let model = attempt.join("model");
        std::fs::create_dir_all(&model)?;
        self.gps_command(
            paths,
            vec![
                "model_transformer".into(),
                "--input_path".into(),
                "gps-v1/enu".into(),
                "--output_path".into(),
                "gps-v1/model".into(),
                "--transform_path".into(),
                "gps-v1/axes.txt".into(),
            ],
        )
        .await?;
        ReconstructionValidator::validate(&paths.frames, &model)?;
        // Verify native Sim3 serialization as well as the ENU-to-PLY mapping.
        let before = crate::video::panorama::read_poses(&enu)?;
        let after = crate::video::panorama::read_poses(&model)?;
        let mapped: HashMap<_, _> = after
            .iter()
            .map(|p| (p.name.as_str(), gps::center(p)))
            .collect();
        if before.iter().any(|p| {
            mapped.get(p.name.as_str()).is_none_or(|actual| {
                let expected = axes.point(gps::center(p));
                (0..3).any(|i| (actual[i] - expected[i]).abs() > 1e-5 * expected[i].abs().max(1.))
            })
        }) {
            return Ok(None);
        }
        Ok(Some((model, Some(alignment.then(axes)))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_matcher_uses_matching_counts_without_switching_the_reconstruction_stage() {
        assert_eq!(
            gps_command_observer(&["spatial_matcher".into()]),
            ObserverMode::Matching
        );
        assert_eq!(
            gps_command_observer(&["pose_prior_mapper".into()]),
            ObserverMode::Mapper
        );
        assert_eq!(
            gps_command_observer(&["model_aligner".into()]),
            ObserverMode::Generic
        );
        assert_eq!(
            gps_command_observer(&["model_transformer".into()]),
            ObserverMode::Generic
        );
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = captured.clone();
        let runner = PipelineRunner::new(
            EnginePaths::from_root(PathBuf::from("missing-engines")),
            move |event| output.lock().unwrap().push(event),
        );
        let observer = runner.process_observer(
            PipelineStage::Reconstructing,
            PipelineEngine::Colmap,
            None,
            gps_command_observer(&["spatial_matcher".into()]),
        );
        observer(crate::process::ProcessUpdate::Started { process_id: 42 });
        observer(crate::process::ProcessUpdate::Line {
            stream: crate::process::ProcessStream::Stderr,
            line: "Indexing image [1/5]".into(),
        });
        let events = captured.lock().unwrap();
        let progress = events
            .iter()
            .find(|event| event.current == Some(1))
            .unwrap();
        assert_eq!(progress.unit.as_deref(), Some("matchingIndexImages"));
        assert_eq!(progress.total, Some(5));
        assert!(progress.indeterminate && progress.stage_progress.is_none());
        assert!(events
            .iter()
            .all(|event| event.stage == PipelineStage::Reconstructing));
    }

    #[tokio::test]
    async fn gps_training_continuation_reuses_cache_after_json_reload_without_engines() {
        use sha2::{Digest, Sha256};

        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        let visual = paths.colmap.join("authoritative");
        let output = paths.colmap.join("gps-v1/model");
        for (directory, contents) in [
            (&visual, b"visual".as_slice()),
            (&output, b"GPS".as_slice()),
        ] {
            std::fs::create_dir_all(directory).unwrap();
            for name in ["cameras.bin", "images.bin", "points3D.bin"] {
                std::fs::write(directory.join(name), contents).unwrap();
            }
        }
        let record = Record {
            name: "synthetic.png".into(),
            source: paths.frames.join("synthetic.png"),
            source_identity: "frozen-source".into(),
            fix: crate::video::gps::GpsFix {
                latitude: 51.248178375505404,
                longitude: -93.31137037688033,
                altitude: Some(52.314008204106244),
                horizontal_std: 10.,
                vertical_std: 20.,
            },
        };
        let manifest = Manifest {
            version: gps::POLICY_VERSION,
            records: vec![record.clone()],
            origin: Some(record.fix),
        };
        let mut state = PipelineStateFile::created_for(Quality::High, ProjectInputType::Images);
        state.stage = PipelineStage::TrainingSplats;
        state.brush_training.dataset_identity = Some("saved-training-data".into());
        state.brush_training.oom_retry_started = true;
        state.brush_training.oom_retry_used = true;
        state.gps = Some(Checkpoint {
            status: "complete".into(),
            fingerprint: gps::fingerprint(&manifest.records).unwrap(),
            manifest_identity: Some(gps::manifest_identity(&manifest).unwrap()),
            input_identity: Some(format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(
                        model_identity(&visual).unwrap(),
                        Option::<crate::video::gps::GpsFix>::None,
                    ))
                    .unwrap()
                )
            )),
            output_identity: Some(model_identity(&output).unwrap()),
            reference_valid: true,
            ..Checkpoint::pending()
        });
        let manager = ProjectManager::for_diagnostics(root.path().into());
        let manifest_path = paths.work.join("gps/manifest.json");
        std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        atomic_write_json(&manifest_path, &manifest).await.unwrap();
        manager.write_state(&paths.state, &state).await.unwrap();
        let expected = serde_json::to_value(&state).unwrap();
        let expected_visual = model_identity(&visual).unwrap();
        let expected_output = model_identity(&output).unwrap();
        let runner = PipelineRunner::new(
            EnginePaths::from_root(root.path().join("engines-must-not-run")),
            |_| {},
        );
        for _ in 0..3 {
            let mut saved = manager.read_state(&paths.state).await.unwrap();
            let manifest: Manifest =
                serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
            let result = runner
                .apply_gps(
                    &manager,
                    &paths,
                    &mut saved,
                    manifest.records,
                    None,
                    &visual,
                    None,
                    None,
                )
                .await
                .unwrap();
            assert_eq!(result.as_deref(), Some(output.as_path()));
            assert_eq!(serde_json::to_value(&saved).unwrap(), expected);
            assert_eq!(model_identity(&visual).unwrap(), expected_visual);
            assert_eq!(model_identity(&output).unwrap(), expected_output);
            assert!(!paths.logs.join("gps.log").exists());
            manager.write_state(&paths.state, &saved).await.unwrap();
        }
    }

    #[test]
    fn gps_mapper_backend_selection_preserves_priors_and_frozen_camera_arguments() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        std::fs::create_dir_all(&paths.colmap).unwrap();
        std::fs::write(paths.colmap.join("panorama-camera.json"), b"{}").unwrap();
        let base = paths.colmap.join("base");
        let visual = paths.colmap.join("visual");
        let fixed = paths.colmap.join("fixed.txt");
        let args = gps_mapper_args(GpsMapperOptions {
            paths: &paths,
            visual: &visual,
            base: Some(&base),
            fixed: Some(&fixed),
            georeference: true,
            allow_two_view_tracks: true,
        });
        assert_eq!(args[0], "pose_prior_mapper");
        let has =
            |name: &str, value: &OsString| args.windows(2).any(|v| v[0] == name && &v[1] == value);
        for (name, value) in [
            ("--database_path", OsString::from("gps-v1/database.db")),
            ("--output_path", OsString::from("gps-v1/native")),
            ("--input_path", base.into_os_string()),
            ("--Mapper.constant_camera_list_path", fixed.into_os_string()),
            ("--Mapper.fix_existing_frames", OsString::from("1")),
            ("--Mapper.ba_refine_principal_point", OsString::from("0")),
            ("--use_robust_loss_on_prior_position", OsString::from("1")),
            ("--prior_position_loss_scale", OsString::from("7.815")),
            ("--Mapper.tri_ignore_two_view_tracks", OsString::from("0")),
            ("--Mapper.init_min_tri_angle", OsString::from("1")),
            ("--Mapper.init_max_forward_motion", OsString::from("1")),
        ] {
            assert!(has(name, &value), "missing {name}");
        }
        assert!(!args
            .iter()
            .any(|v| v == "--Mapper.ba_global_backend" || v == "--Mapper.ba_local_backend"));

        let ordinary = gps_mapper_args(GpsMapperOptions {
            paths: &paths,
            visual: &visual,
            base: None,
            fixed: None,
            georeference: false,
            allow_two_view_tracks: false,
        });
        assert_eq!(ordinary[0], "mapper");
        assert!(!ordinary
            .iter()
            .any(|v| v == "--use_robust_loss_on_prior_position"
                || v == "--Mapper.fix_existing_frames"));
        assert!(ordinary
            .windows(2)
            .any(|v| v[0] == "--input_path" && v[1] == visual.as_os_str()));
    }

    #[test]
    fn gps_visual_fallback_uses_only_the_final_normal_alignment_failure() {
        use crate::failure::{EngineProcessFailure, FailureClassification};
        use crate::process::ProcessTermination;
        let failure = |termination, kind, detail: &str| EngineProcessFailure {
            mapper_geometry_failure: false,
            last_splat_count: None,
            engine: PipelineEngine::Colmap,
            operation: "mapper".into(),
            termination,
            detail: detail.into(),
            classification: FailureClassification {
                engine: Some(PipelineEngine::Colmap),
                kind,
                heuristic: false,
            },
            previous_attempts: vec![],
        };
        let normal = ProcessTermination {
            exit_code: Some(1),
            ..Default::default()
        };
        let mut final_failure = failure(normal.clone(), None, "insufficient visual matches");
        final_failure.previous_attempts.push(failure(
            ProcessTermination {
                exit_code: Some(-1073740940),
                windows_status: Some("0xC0000374".into()),
                ..Default::default()
            },
            Some("colmap_crash"),
            "prior Caspar crash",
        ));
        assert!(native_alignment_rejected(&SplatError::EngineProcess(
            Box::new(final_failure)
        )));
        let crash = failure(
            ProcessTermination {
                exit_code: Some(-1073740940),
                windows_status: Some("0xC0000374".into()),
                ..Default::default()
            },
            Some("colmap_crash"),
            "alignment failed before crash",
        );
        assert!(!native_alignment_rejected(&SplatError::EngineProcess(
            Box::new(crash)
        )));
        let storage = failure(normal, Some("mapper_storage"), "insufficient disk space");
        assert!(!native_alignment_rejected(&SplatError::EngineProcess(
            Box::new(storage)
        )));
        assert!(!native_alignment_rejected(&SplatError::Cancelled));
        assert!(!native_alignment_rejected(&SplatError::Io(
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "alignment failed")
        )));
    }

    #[tokio::test]
    async fn gps_fallback_is_frozen_resumes_and_never_mutates_published_or_source_files() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ProjectManager::for_diagnostics(dir.path().to_path_buf());
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), dir.path().join("project"));
        for p in [&paths.frames, &paths.masks, &paths.brush, &paths.colmap] {
            std::fs::create_dir_all(p).unwrap();
        }
        let model = paths.colmap.join("visual");
        std::fs::create_dir(&model).unwrap();
        for name in ["cameras.bin", "images.bin", "points3D.bin"] {
            std::fs::write(model.join(name), b"visual snapshot").unwrap();
        }
        let protected = [
            paths.frames.join("source.png"),
            paths.masks.join("source.png.png"),
            paths.project.join("final.ply"),
            paths.project.join("edit.ply"),
        ];
        for p in &protected {
            std::fs::write(p, b"unchanged").unwrap();
        }
        let runner = PipelineRunner::new(
            EnginePaths::from_root(dir.path().join("absent-engines")),
            |_| {},
        );
        let mut state = PipelineStateFile::created_for(Quality::Fast, ProjectInputType::Images);
        let record = Record {
            name: "source.png".into(),
            source: protected[0].clone(),
            source_identity: "identity".into(),
            fix: crate::video::gps::GpsFix {
                latitude: 0.,
                longitude: 0.,
                altitude: Some(0.),
                horizontal_std: 10.,
                vertical_std: 20.,
            },
        };
        // Legacy tasks stay on their original route, even when GPS is present.
        state.brush_complete = true;
        assert!(runner
            .apply_gps(
                &manager,
                &paths,
                &mut state,
                vec![record.clone()],
                None,
                &model,
                None,
                None
            )
            .await
            .unwrap()
            .is_none());
        assert!(state.brush_complete);
        state.gps = Some(Checkpoint::pending());
        assert!(runner
            .apply_gps(
                &manager,
                &paths,
                &mut state,
                vec![record.clone()],
                None,
                &model,
                None,
                None
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(state.gps.as_ref().unwrap().status, "fallback");
        assert!(!state.brush_complete);
        state.brush_complete = true;
        let mut state: PipelineStateFile =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        runner
            .apply_gps(
                &manager,
                &paths,
                &mut state,
                vec![record.clone()],
                None,
                &model,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(state.brush_complete); // unchanged fallback doesn't invalidate again
        std::fs::write(model.join("images.bin"), b"new coordinate system").unwrap();
        runner
            .apply_gps(
                &manager,
                &paths,
                &mut state,
                vec![record.clone()],
                None,
                &model,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(!state.brush_complete); // visual identity is part of the cache key
        for p in &protected {
            assert_eq!(std::fs::read(p).unwrap(), b"unchanged");
        }
        runner.process_manager.cancel();
        assert!(matches!(
            runner
                .apply_gps(
                    &manager,
                    &paths,
                    &mut state,
                    vec![record],
                    None,
                    &model,
                    None,
                    None
                )
                .await,
            Err(SplatError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn database_rebuild_does_not_renew_the_same_runs_gps_fallback() {
        let root = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), root.path().join("project"));
        let manager = ProjectManager::for_diagnostics(root.path().into());
        let runner = PipelineRunner::new(
            EnginePaths::from_root(root.path().join("missing-engines")),
            |_| {},
        );
        let mut state = PipelineStateFile::created(Quality::Balanced);
        state.gps = Some(Checkpoint {
            status: "fallback".into(),
            database_recovery_visual_only: true,
            input_identity: Some("old-model-identity".into()),
            ..Checkpoint::pending()
        });
        let before = serde_json::to_value(&state).unwrap();
        assert!(runner
            .apply_gps(
                &manager,
                &paths,
                &mut state,
                vec![],
                None,
                &paths.colmap.join("missing-new-model"),
                None,
                None
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        assert!(!paths.project.exists());
    }

    #[tokio::test]
    #[ignore = "Requires completed public photo smoke; real native horizontal-only matching and cache, no training"]
    async fn native_public_gps_matching_and_cache() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let original = repo.join(".cache/gps-v1/public-projects/20261026-203054_images");
        let owned = repo
            .join(".cache/gps-v1/horizontal")
            .join(uuid::Uuid::new_v4().simple().to_string());
        let paths = ProjectPaths::existing(uuid::Uuid::new_v4(), owned);
        let manager = ProjectManager::for_diagnostics(paths.project.clone());
        for (source, destination) in [
            (original.join("work/frames"), &paths.frames),
            (original.join("work/colmap_frames"), &paths.colmap_frames),
        ] {
            std::fs::create_dir_all(destination).unwrap();
            for file in crate::video::list_images(&source).unwrap() {
                std::fs::hard_link(&file, destination.join(file.file_name().unwrap())).unwrap();
            }
        }
        std::fs::create_dir_all(&paths.colmap).unwrap();
        std::fs::create_dir_all(&paths.logs).unwrap();
        let db = original.join("work/colmap/database.db");
        let before = crate::video::panorama::fingerprint(&db).unwrap();
        std::fs::copy(&db, paths.colmap.join("database.db")).unwrap();
        let manifest: Manifest = serde_json::from_slice(
            &std::fs::read(original.join("work/gps/manifest.json")).unwrap(),
        )
        .unwrap();
        let records: Vec<_> = manifest
            .records
            .into_iter()
            .map(|mut r| {
                r.fix.altitude = None;
                r
            })
            .collect();
        let mut state = PipelineStateFile::created_for(Quality::Fast, ProjectInputType::Images);
        state.gps = Some(Checkpoint::pending());
        let runner = PipelineRunner::new(EnginePaths::from_root(repo.join("engines")), |_| {});
        let visual = original.join("work/colmap/sparse/0");
        let result = runner
            .apply_gps(
                &manager,
                &paths,
                &mut state,
                records.clone(),
                None,
                &visual,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(result.is_some());
        assert_eq!(state.gps.as_ref().unwrap().status, "matching");
        assert!(!state.gps.as_ref().unwrap().reference_valid);
        assert!(state.gps.as_ref().unwrap().transform.is_none());
        state.brush_complete = true;
        runner
            .apply_gps(
                &manager,
                &paths,
                &mut state,
                records.clone(),
                None,
                &visual,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(state.brush_complete);
        std::fs::write(result.unwrap().join("points3D.bin"), b"damaged cache").unwrap();
        assert!(runner
            .apply_gps(&manager, &paths, &mut state, records, None, &visual, None, None)
            .await
            .unwrap()
            .is_some());
        assert!(!state.brush_complete);
        assert_eq!(crate::video::panorama::fingerprint(&db).unwrap(), before);
        println!(
            "Horizontal-only native/cache evidence: {}",
            paths.project.display()
        );
    }

    #[tokio::test]
    #[ignore = "Requires pinned native engines, the BSD-2-Clause Brighton Beach photos and a training GPU; writes an isolated .cache project"]
    async fn native_public_gps_generation() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let input=repo.join(".cache/gps-v1/drone_dataset_brighton_beach-9506a9bf09c9678b7d0b80b014af5b606c8a7255/images");
        let files = crate::video::list_images(&input).unwrap();
        let before: Vec<_> = files
            .iter()
            .map(|p| crate::video::panorama::fingerprint(p).unwrap())
            .collect();
        let quick = std::time::Instant::now();
        assert!(crate::video::gps::detect(&files));
        let quick_ms = quick.elapsed().as_millis();
        let projects = repo.join(".cache/gps-v1/public-projects");
        let peaks = Arc::new(std::sync::Mutex::new((0u64, 0u64)));
        let peak_collector = peaks.clone();
        let started = std::time::Instant::now();
        let runner =
            PipelineRunner::new(EnginePaths::from_root(repo.join("engines")), move |event| {
                if let Some(resources) = event.runtime.as_ref().and_then(|r| r.resources.as_ref()) {
                    let mut peaks = peak_collector.lock().unwrap();
                    peaks.0 = peaks.0.max(resources.memory_bytes.unwrap_or(0));
                    peaks.1 = peaks.1.max(
                        resources
                            .gpus
                            .iter()
                            .filter_map(|g| g.memory_used_mib)
                            .max()
                            .unwrap_or(0),
                    );
                }
                if event.kind == EventKind::Log
                    && (event.message.starts_with("[GPS]")
                        || event.message == gps::FALLBACK_MESSAGE)
                {
                    println!("{}", event.message);
                }
            });
        let result = runner
            .generate_for_diagnostics(&input, Quality::Fast, &projects)
            .await
            .unwrap();
        println!(
            "Native public GPS result: {} ; {} splats",
            result.project_path.display(),
            result.splat_count
        );
        let state: PipelineStateFile =
            serde_json::from_slice(&std::fs::read(result.project_path.join("state.json")).unwrap())
                .unwrap();
        assert_eq!(state.gps.unwrap().status, "complete");
        assert!(result.splat_count > 0);
        let peaks = *peaks.lock().unwrap();
        let measurement = serde_json::json!({"quickSampleMs":quick_ms,"totalMs":started.elapsed().as_millis(),"splats":result.splat_count,"maxSampledProcessMemoryBytes":peaks.0,"maxSampledWholeGpuMemoryMiB":peaks.1});
        std::fs::write(
            result.project_path.join("gps-validation.json"),
            serde_json::to_vec_pretty(&measurement).unwrap(),
        )
        .unwrap();
        println!("Measurements: {measurement}");
        for (file, expected) in files.iter().zip(before) {
            assert_eq!(crate::video::panorama::fingerprint(file).unwrap(), expected);
        }
    }
}
