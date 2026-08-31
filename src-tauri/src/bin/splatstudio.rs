use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ooo_splat::{
    engines::{ffmpeg::extract_uniform_frames, ffprobe::probe_video},
    error::{Result, SplatError},
    pipeline::runner::{default_engine_paths, PipelineRunner},
    presets::Quality,
    process::ProcessManager,
    video::{FrameSelectionStrategy, UniformRatioFrameSelection},
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
    /// Read video metadata through FFprobe JSON.
    Probe { input: PathBuf },
    /// Show the uniform frame plan without extracting images.
    Plan {
        input: PathBuf,
        #[arg(long, value_enum, default_value_t = Quality::Balanced)]
        quality: Quality,
    },
    /// Extract uniformly sampled frames and, for alpha video, COLMAP masks.
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
            let video = probe_video(&engines.ffprobe, &input, None).await?;
            println!("{}", serde_json::to_string_pretty(&video)?);
        }
        Commands::Plan { input, quality } => {
            let video = probe_video(&engines.ffprobe, &input, None).await?;
            let plan = UniformRatioFrameSelection.create_plan(&video, &quality.preset());
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
        Commands::Extract {
            input,
            output,
            quality,
        } => {
            ensure_engine(&engines.ffprobe)?;
            ensure_engine(&engines.ffmpeg)?;
            let video = probe_video(&engines.ffprobe, &input, None).await?;
            let plan = UniformRatioFrameSelection.create_plan(&video, &quality.preset());
            let masks = output
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("masks");
            let extraction = extract_uniform_frames(
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

fn ensure_engine(path: &std::path::Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(SplatError::EngineMissing(path.display().to_string()))
    }
}
