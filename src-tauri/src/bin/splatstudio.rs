use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ooo_splat::{
    engines::{ffmpeg::extract_uniform_frames, ffprobe::probe_video},
    error::{Result, SplatError},
    pipeline::runner::{default_engine_paths, PipelineRunner},
    presets::Quality,
    process::ProcessManager,
    video::{
        analyze_image_sequence, create_image_plan, filter_frames_at_fps,
        filter_frames_with_masks_at_fps, prepare_image_sequence, FrameSelectionStrategy,
        SmartFrameSelection,
    },
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

#[derive(Debug, Subcommand)]
enum Commands {
    /// Validate FFmpeg, FFprobe, CPU COLMAP and Brush.
    Health,
    /// Read video metadata or image-sequence information.
    Probe { input: PathBuf },
    /// Show the video frame plan or image-sequence plan.
    Plan {
        input: PathBuf,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
    },
    /// Prepare video frames or an image sequence and optional COLMAP masks.
    Extract {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
    },
    /// Run the end-to-end pipeline after all fixed engine CLIs are verified.
    Generate {
        input: PathBuf,
        /// Override the remembered projects root (useful for diagnostics).
        #[arg(long)]
        projects_root: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
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
                let video = probe_video(&engines.ffprobe, &input, None).await?;
                println!("{}", serde_json::to_string_pretty(&video)?);
            }
        }
        Commands::Plan { input, quality } => {
            let plan = if input.is_dir() {
                create_image_plan(&analyze_image_sequence(&input)?, &quality.preset())
            } else {
                let video = probe_video(&engines.ffprobe, &input, None).await?;
                SmartFrameSelection.create_plan(&video, &quality.preset())
            };
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
        Commands::Extract {
            input,
            output,
            quality,
        } => {
            let masks = output
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("masks");
            if input.is_dir() {
                let extraction = prepare_image_sequence(&input, &output, &masks)?;
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
            let video = probe_video(&engines.ffprobe, &input, None).await?;
            let plan = SmartFrameSelection.create_plan(&video, &quality.preset());
            let mut extraction = extract_uniform_frames(
                &engines.ffmpeg,
                &input,
                &output,
                &masks,
                &plan,
                video.has_alpha,
                None,
                &ProcessManager::new(),
                None,
            )
            .await?;
            let filtered_output = output.with_file_name("frames.filtered");
            let config = quality.preset().smart_filter_config;
            let outcome = tokio::task::spawn_blocking({
                let output = output.clone();
                let filtered_output = filtered_output.clone();
                let masks = masks.clone();
                move || {
                    if video.has_alpha {
                        filter_frames_with_masks_at_fps(
                            &output,
                            &filtered_output,
                            &masks,
                            &config,
                            plan.sampling_fps,
                        )
                    } else {
                        filter_frames_at_fps(&output, &filtered_output, &config, plan.sampling_fps)
                    }
                }
            })
            .await
            .map_err(|error| SplatError::Process(format!("智能抽帧过滤任务失败：{error}")))?
            .map_err(|error| SplatError::Process(format!("智能抽帧过滤失败：{error}")))?;
            replace_cli_filtered_frames(&output, &masks, &filtered_output, video.has_alpha).await?;
            extraction.frame_count = outcome.kept_frames as u64;
            if extraction.has_alpha {
                extraction.mask_count = outcome.kept_frames as u64;
            }
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
            input,
            projects_root,
            quality,
        } => {
            let runner = PipelineRunner::new(engines, |event| {
                eprintln!(
                    "{:>6.2}% {:?}: {}",
                    event.progress, event.stage, event.message
                );
            });
            let result = match projects_root {
                Some(root) => {
                    runner
                        .generate_for_diagnostics(&input, quality, &root)
                        .await?
                }
                None => {
                    let root = ooo_splat::project::catalog::load_settings()
                        .await?
                        .projects_root;
                    runner.generate(&input, quality, &root).await?
                }
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}

async fn replace_cli_filtered_frames(
    frames: &std::path::Path,
    masks: &std::path::Path,
    filtered: &std::path::Path,
    has_alpha: bool,
) -> Result<()> {
    let mut entries = tokio::fs::read_dir(frames).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().is_file()
            && entry.path().extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("jpg")
                    || ext.eq_ignore_ascii_case("jpeg")
                    || ext.eq_ignore_ascii_case("png")
            })
        {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    let mut kept = std::collections::HashSet::new();
    let mut filtered_entries = tokio::fs::read_dir(filtered).await?;
    while let Some(entry) = filtered_entries.next_entry().await? {
        let source = entry.path();
        let name = entry.file_name();
        if source.is_file()
            && source.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("jpg")
                    || ext.eq_ignore_ascii_case("jpeg")
                    || ext.eq_ignore_ascii_case("png")
            })
        {
            kept.insert(name.to_string_lossy().into_owned());
            tokio::fs::copy(&source, frames.join(&name)).await?;
        }
    }
    if has_alpha && masks.is_dir() {
        let mut mask_entries = tokio::fs::read_dir(masks).await?;
        while let Some(entry) = mask_entries.next_entry().await? {
            if let Some(frame_name) = entry.file_name().to_string_lossy().strip_suffix(".png") {
                if !kept.contains(frame_name) {
                    tokio::fs::remove_file(entry.path()).await?;
                }
            }
        }
    }
    for name in [
        "metadata.csv",
        "filter_summary.json",
        "filter_forced_keep.log",
    ] {
        let source = filtered.join(name);
        if source.is_file() {
            tokio::fs::copy(&source, frames.join(name)).await?;
        }
    }
    tokio::fs::remove_dir_all(filtered).await?;
    Ok(())
}

fn ensure_engine(path: &std::path::Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(SplatError::EngineMissing(path.display().to_string()))
    }
}
