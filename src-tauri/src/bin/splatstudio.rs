use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ooo_splat::{
    engines::{ffmpeg::extract_selected_frames, ffprobe::probe_video},
    error::{Result, SplatError},
    pipeline::runner::{default_engine_paths, PipelineRunner},
    presets::Quality,
    process::ProcessManager,
    video::{analyze_image_sequence, create_image_plan, prepare_image_sequence},
};

#[derive(Debug, Parser)]
#[command(name = "splatstudio", version, about = "OOOSplat local pipeline CLI")]
struct Cli {
    /// Override the bundled engine directory (also supports OOOSPLAT_ENGINE_DIR).
    #[arg(long, global = true)]
    engine_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Args)]
struct PolicyArgs {
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    auto_retry: bool,
    /// Fraction of source frames for the first extraction (0.001..=1).
    #[arg(long)]
    frame_sampling_ratio: Option<f64>,
    /// Shared video target FPS. Mutually exclusive with the legacy ratio.
    #[arg(long, conflicts_with = "frame_sampling_ratio")]
    frame_sampling_fps: Option<f64>,
}
impl PolicyArgs {
    fn policy(&self) -> Result<ooo_splat::pipeline::generation_policy::GenerationPolicy> {
        ooo_splat::pipeline::generation_policy::GenerationPolicy {
            auto_retry_enabled: self.auto_retry,
            frame_sampling_ratio: self.frame_sampling_ratio,
            frame_sampling_fps: self.frame_sampling_fps,
            ..Default::default()
        }
        .validate()
    }
}

#[derive(Debug, Subcommand)]
// Clap constructs this one command value once; boxing its path flags is not useful.
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Validate FFmpeg, FFprobe, CPU COLMAP and Brush.
    Health,
    /// Read video metadata or image-sequence information.
    Probe { input: PathBuf },
    /// Show the video frame plan or image-sequence plan.
    Plan {
        #[command(flatten)]
        policy: PolicyArgs,
        input: PathBuf,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
    },
    /// Prepare video frames or an image sequence and optional COLMAP masks.
    Extract {
        #[command(flatten)]
        policy: PolicyArgs,
        input: PathBuf,
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
    },
    /// Run the end-to-end pipeline after all fixed engine CLIs are verified.
    Generate {
        #[command(flatten)]
        policy: PolicyArgs,
        input: Vec<PathBuf>,
        /// Ordered shared input JSON, including per-set projections and confirmed answers.
        #[arg(long,conflicts_with_all=["input","colmap_model"])]
        input_manifest: Option<PathBuf>,
        #[arg(long, requires = "images", conflicts_with = "input")]
        /// Temporarily unavailable: direct COLMAP imports are disabled.
        colmap_model: Option<PathBuf>,
        #[arg(long, requires = "colmap_model")]
        images: Option<PathBuf>,
        #[arg(long, requires = "colmap_model")]
        database: Option<PathBuf>,
        #[arg(long,value_parser=["sequential","exhaustive"])]
        image_order: Option<String>,
        #[arg(long)]
        accept_small_image_warning: bool,
        #[arg(long)]
        source_project: Option<uuid::Uuid>,
        #[arg(long,value_parser=["good","appearance","geometry","unsure"])]
        model_condition: Option<String>,
        #[arg(long,value_parser=["same","different","unsure"])]
        camera_relation: Option<String>,
        /// Print metadata/plans only; no task or generation process is created.
        #[arg(long)]
        inspect: bool,
        /// Override the remembered projects root (useful for diagnostics).
        #[arg(long)]
        projects_root: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
        #[arg(long, value_enum, default_value_t = ooo_splat::video::panorama::VideoProjection::Auto)]
        video_projection: ooo_splat::video::panorama::VideoProjection,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();
    if let Err(error) = execute(Cli::parse()).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn execute(cli: Cli) -> Result<()> {
    let engines = default_engine_paths(cli.engine_dir);
    match cli.command {
        Commands::Health => {
            println!(
                "{}",
                serde_json::to_string_pretty(&engines.check_all().await)?
            );
        }
        Commands::Probe { input } => {
            if input.is_dir() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&analyze_image_sequence(&input)?)?
                );
            } else {
                let video =
                    probe_video(&engines.ffprobe, &input, None, &ProcessManager::new()).await?;
                println!("{}", serde_json::to_string_pretty(&video)?);
            }
        }
        Commands::Plan {
            input,
            quality,
            policy,
        } => {
            let policy = policy.policy()?;
            let plan = if input.is_dir() {
                create_image_plan(&analyze_image_sequence(&input)?, &quality.preset())
            } else {
                let video =
                    probe_video(&engines.ffprobe, &input, None, &ProcessManager::new()).await?;
                policy.frame_plan(&video, quality)
            };
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
        Commands::Extract {
            policy,
            input,
            output,
            quality,
        } => {
            let policy = policy.policy()?;
            let masks = output
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("masks");
            let colmap_frames = output
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("colmap_frames");
            if input.is_dir() {
                let extraction = prepare_image_sequence(&input, &output, &colmap_frames, &masks)?;
                println!(
                    "prepared {} images in {} and {} masks in {}",
                    extraction.image_count,
                    output.display(),
                    extraction.mask_count,
                    masks.display()
                );
                return Ok(());
            }
            ensure_engine(&engines.ffprobe)?;
            ensure_engine(&engines.ffmpeg)?;
            let video = probe_video(&engines.ffprobe, &input, None, &ProcessManager::new()).await?;
            let plan = policy.frame_plan(&video, quality);
            let (w, h) = if video.rotation.rem_euclid(180) == 90 {
                (video.height, video.width)
            } else {
                (video.width, video.height)
            };
            let resolution = policy.resolution(quality, None, w, h, true);
            let extraction = extract_selected_frames(
                &engines.ffmpeg,
                &input,
                &output,
                &colmap_frames,
                &masks,
                &plan.selected_frames,
                video.has_alpha,
                Some((resolution.working_width, resolution.working_height)),
                None,
                &ProcessManager::new(),
                None,
            )
            .await?;
            if extraction.has_alpha {
                println!(
                    "extracted {} RGBA frames to {} and {} masks to {}",
                    extraction.frame_count,
                    output.display(),
                    extraction.mask_count,
                    masks.display()
                );
            } else {
                println!(
                    "extracted {} frames to {}",
                    extraction.frame_count,
                    output.display()
                );
            }
        }
        Commands::Generate {
            policy,
            input,
            projects_root,
            quality,
            video_projection,
            input_manifest,
            colmap_model,
            images,
            database,
            image_order,
            accept_small_image_warning,
            source_project,
            model_condition,
            camera_relation,
            inspect,
        } => {
            use ooo_splat::pipeline::{inputs::*, reshoot::ReshootOptions};
            let policy = policy.policy()?;
            let mut inputs = if let Some(manifest) = input_manifest {
                serde_json::from_slice::<GenerationInputs>(&tokio::fs::read(manifest).await?)?
            } else if let Some(model) = colmap_model {
                GenerationInputs::Colmap {
                    input: ColmapInput {
                        model,
                        images: images.unwrap(),
                        database,
                        identity: None,
                    },
                }
            } else {
                GenerationInputs::Materials {
                    items: ooo_splat::commands::classify_material_paths(input).await?,
                }
            };
            ooo_splat::pipeline::input_admission::ensure_new_inputs_supported(&inputs)?;
            inputs = inputs.canonicalized()?;
            inputs.authorized_paths()?;
            if source_project.is_some() && matches!(inputs, GenerationInputs::Colmap { .. }) {
                return Err(SplatError::Process(
                    "COLMAP direct training cannot append inputs".into(),
                ));
            }
            let mut analyses = vec![];
            if let GenerationInputs::Materials { items } = &mut inputs {
                for item in items.iter_mut() {
                    if video_projection != ooo_splat::video::panorama::VideoProjection::Auto {
                        item.projection = video_projection;
                    }
                    let a = inspect_material(&engines, item, quality, policy).await?;
                    item.identity = Some(a.identity.clone());
                    if !a.projection_detection.requires_choice
                        && item.projection == ooo_splat::video::panorama::VideoProjection::Auto
                    {
                        item.projection = if a.projection_detection.confirmed_erp {
                            ooo_splat::video::panorama::VideoProjection::Equirectangular
                        } else {
                            ooo_splat::video::panorama::VideoProjection::Perspective
                        };
                    }
                    if let Some(order) = &image_order {
                        item.image_matching_strategy =
                            serde_json::from_value(serde_json::json!(order))?;
                        item.confirmed_image_order = true;
                    }
                    item.confirmed_small_images |=
                        accept_small_image_warning || source_project.is_some();
                    if let Some(relation) = &camera_relation {
                        item.camera_relation =
                            Some(serde_json::from_value(serde_json::json!(relation))?);
                    }
                    analyses.push(a);
                }
            }
            if inspect {
                let mut summaries = serde_json::to_value(&analyses)?;
                for analysis in summaries.as_array_mut().into_iter().flatten() {
                    if let Some(plan) = analysis
                        .get_mut("plan")
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        plan.remove("selectedFrames");
                        plan.remove("candidateFrames");
                    }
                }
                let colmap = match &inputs {
                    GenerationInputs::Colmap { input } => {
                        Some(ooo_splat::reconstruction::direct_input::inspect(input)?)
                    }
                    _ => None,
                };
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"inputs":inputs,"materials":summaries,"colmap":colmap,"generationPolicy":policy,"planIsEstimate":true})
                    )?
                );
                return Ok(());
            }
            validate(&engines, &inputs, quality, policy).await?;
            if source_project.is_some() {
                let GenerationInputs::Materials { items } = &inputs else {
                    return Err(SplatError::Process(
                        "COLMAP direct training cannot append inputs".into(),
                    ));
                };
                if items.iter().any(|i| {
                    i.projection != ooo_splat::video::panorama::VideoProjection::Equirectangular
                        && i.camera_relation.is_none()
                }) {
                    return Err(SplatError::Process("CONFIRMATION_REQUIRED: --camera-relation is required for every ordinary source".into()));
                }
            }
            let options = if let Some(id) = source_project {
                let source = PipelineRunner::new(engines.clone(), |_| {})
                    .inspect_reshoot_source_v2(id)
                    .await?;
                let condition = model_condition.ok_or_else(|| {
                    SplatError::Process(
                        "CONFIRMATION_REQUIRED: --model-condition is required".into(),
                    )
                })?;
                Some(ReshootOptions {
                    condition: serde_json::from_value(serde_json::json!(condition))?,
                    camera_relation: None,
                    allow_fallback: policy.auto_retry_enabled,
                    source_identity: source.source_identity,
                    input_identity: serde_json::to_string(
                        &analyses.iter().map(|a| &a.identity).collect::<Vec<_>>(),
                    )?,
                    projection: ooo_splat::video::panorama::VideoProjection::Perspective,
                    image_matching_strategy: Default::default(),
                })
            } else {
                None
            };
            let root = match projects_root {
                Some(root) => root,
                None => {
                    ooo_splat::project::catalog::load_settings()
                        .await?
                        .projects_root
                }
            };
            let registry = root.join(".cli-task-records");
            let _registry_owner = ooo_splat::tasks::cli_guard::claim(&registry)
                .map_err(|e| SplatError::Process(e.to_string()))?;
            let service = ooo_splat::tasks::TaskService::at(registry)
                .with_run_lock(ooo_splat::tasks::TaskService::desktop_execution_lock()?);
            let request = serde_json::json!({"inputs":inputs,"generationPolicy":policy,"source_project_id":source_project,"reshootOptions":options});
            let task = service
                .create(
                    None,
                    inputs.primary_path().to_path_buf(),
                    quality,
                    root,
                    true,
                    "cli",
                    None,
                    Some(request),
                )
                .await
                .map_err(|e| SplatError::Process(e.to_string()))?;
            service
                .start(task.task_id, engines, None)
                .await
                .map_err(|e| SplatError::Process(e.to_string()))?;
            let mut sequence = 0;
            loop {
                let task = service
                    .get(task.task_id)
                    .await
                    .map_err(|e| SplatError::Process(e.to_string()))?;
                for event in task.recent_events.iter().filter(|e| e.sequence > sequence) {
                    eprintln!(
                        "{:>6.2}% {:?}: {}",
                        event.progress, event.stage, event.message
                    );
                }
                sequence = task.sequence;
                if !task.status.active() && service.execution_settled(task.run_id) {
                    if task.status == ooo_splat::tasks::TaskStatus::Completed {
                        println!("{}", serde_json::to_string_pretty(&task.result)?);
                        break;
                    }
                    return Err(SplatError::Process(
                        task.error
                            .map(|e| e.message)
                            .unwrap_or_else(|| format!("Task {:?}", task.status)),
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
    }
    Ok(())
}

fn ensure_engine(path: &std::path::Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(SplatError::EngineMissing(path.display().to_string()))
    }
}

#[cfg(test)]
mod input_admission_tests {
    use super::*;

    #[tokio::test]
    async fn new_colmap_flags_and_manifest_are_rejected_before_engine_or_task_work() {
        let root = tempfile::tempdir().unwrap();
        let model = root.path().join("missing-model");
        let images = root.path().join("missing-images");
        let manifest = root.path().join("input.json");
        let inputs = ooo_splat::pipeline::inputs::GenerationInputs::Colmap {
            input: ooo_splat::pipeline::inputs::ColmapInput {
                model: model.clone(),
                images: images.clone(),
                database: None,
                identity: None,
            },
        };
        std::fs::write(&manifest, serde_json::to_vec(&inputs).unwrap()).unwrap();
        let cases = [
            vec![
                "splatstudio".to_owned(),
                "generate".to_owned(),
                "--colmap-model".to_owned(),
                model.to_string_lossy().into_owned(),
                "--images".to_owned(),
                images.to_string_lossy().into_owned(),
                "--inspect".to_owned(),
            ],
            vec![
                "splatstudio".to_owned(),
                "generate".to_owned(),
                "--input-manifest".to_owned(),
                manifest.to_string_lossy().into_owned(),
                "--inspect".to_owned(),
            ],
        ];
        for args in cases {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(matches!(
                execute(cli).await,
                Err(SplatError::InputAdmission(
                    ooo_splat::error::InputAdmissionReason::ColmapDisabled
                ))
            ));
        }
        assert!(!model.exists() && !images.exists());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
