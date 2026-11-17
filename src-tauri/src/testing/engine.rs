use super::*;
use crate::process::{
    ProcessObserver, ProcessOutput, ProcessSpec, ProcessStream, ProcessTermination, ProcessUpdate,
};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, io::Write};
use tokio_util::sync::CancellationToken;

pub(super) fn argument<'a>(args: &'a [String], key: &str) -> Result<&'a str> {
    args.iter()
        .position(|s| s == key)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .ok_or_else(|| invalid(format!("missing {key}")))
}
fn brush_dataset(args: &[String]) -> Result<&str> {
    let mut positional = None;
    let mut n = 0;
    while n < args.len() {
        if args[n].starts_with("--") {
            n += 2;
        } else {
            if positional.replace(args[n].as_str()).is_some() {
                return Err(invalid("multiple Brush datasets"));
            }
            n += 1;
        }
    }
    positional.ok_or_else(|| invalid("missing Brush dataset"))
}
fn fixture<'a>(h: &'a Harness, path: &str) -> Result<&'a InputFixture> {
    let path = safe_path(&h.root, Path::new(path))?;
    h.scenario
        .inputs
        .iter()
        .find(|i| i.kind == "video" && h.root.join("fixtures").join(&i.name) == path)
        .ok_or_else(|| invalid("undeclared video input"))
}
fn operation(spec: &ProcessSpec) -> Result<(String, String)> {
    let name = spec
        .executable
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let args = spec
        .args
        .iter()
        .map(|s| s.to_string_lossy())
        .collect::<Vec<_>>();
    let engine = if name.starts_with("ffprobe") {
        "ffprobe"
    } else if name.starts_with("ffmpeg") {
        "ffmpeg"
    } else if name.starts_with("colmap") {
        "colmap"
    } else if name.starts_with("brush_app") {
        "brush"
    } else {
        return Err(invalid(format!("undeclared executable {name}")));
    };
    let op = if args
        .iter()
        .any(|a| a == "-h" || a == "--help" || a == "-version")
    {
        "help".into()
    } else {
        match engine {
            "ffprobe" => if args.iter().any(|a| a == "-show_frames") {
                "timestamps"
            } else {
                "probe"
            }
            .into(),
            "ffmpeg" => if args.iter().any(|a| a == "-frames:v") {
                "preview"
            } else {
                "extract"
            }
            .into(),
            "brush" => "train".into(),
            _ => args
                .first()
                .ok_or_else(|| invalid("empty COLMAP command"))?
                .to_string(),
        }
    };
    Ok((engine.into(), op))
}
fn emit(spec: &ProcessSpec, lifecycle: &Option<ProcessObserver>, event: ProcessUpdate, both: bool) {
    if both {
        if let Some(o) = lifecycle {
            o(event.clone());
        }
    }
    if let Some(o) = &spec.observer {
        o(event);
    }
}
fn help() -> String {
    format!("COLMAP 4.2.1 (Commit {})\n--FeatureExtraction.use_gpu\n--FeatureExtraction.gpu_index\n--FeatureMatching.use_gpu\n--FeatureMatching.gpu_index\n--Mapper.ba_local_backend\n--Mapper.ba_global_backend\n--Mapper.ba_global_gpu_index\n",crate::engines::colmap::LOCKED_COMMIT)
}

pub async fn execute(
    spec: ProcessSpec,
    environment: &[(OsString, OsString)],
    lifecycle: Option<ProcessObserver>,
    cancel: CancellationToken,
    complete_limit: Option<usize>,
) -> Result<ProcessOutput> {
    let h = host()?;
    safe_path(&h.root, &spec.executable)?;
    if let Some(p) = &spec.working_directory {
        safe_path(&h.root, p)?;
    }
    if let Some(p) = &spec.log_path {
        safe_path(&h.root, p)?;
    }
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    let (engine, operation) = operation(&spec)?;
    let device_name = if engine == "brush" && operation == "train" {
        environment_device_name(environment)?
    } else {
        String::new()
    };
    let args = spec
        .args
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if !h
        .scenario
        .allowed_operations
        .contains(&format!("{engine}:{operation}"))
    {
        return Err(invalid(format!(
            "undeclared operation {engine}:{operation}"
        )));
    }
    // Validate all explicit paths before the fixture implementation can create
    // anything. Dataset paths are positional on Brush, output paths on FFmpeg.
    for key in [
        "--database_path",
        "--image_path",
        "--output_path",
        "--input_path",
        "--image_list_path",
        "--match_list_path",
        "--Mapper.image_list_path",
        "--Mapper.constant_camera_list_path",
        "--database_image_list_path",
        "--query_image_list_path",
        "--vocab_tree_path",
        "--export-path",
        "-i",
        "-filter_script:v",
        "-filter_complex_script",
        "-stats_enc_pre",
    ] {
        for (i, arg) in args.iter().enumerate() {
            if arg == key {
                let p = args
                    .get(i + 1)
                    .ok_or_else(|| invalid("missing path argument"))?;
                safe_path(&h.root, Path::new(p))?;
            }
        }
    }
    if engine == "brush" {
        safe_path(&h.root, Path::new(brush_dataset(&args)?))?;
    }
    let mut init_files: Vec<String> = if engine == "brush" {
        std::fs::read_dir(brush_dataset(&args)?)?
            .map(|e| e.map_err(SplatError::from))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter(|e| e.path().extension().is_some_and(|s| s == "ply"))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    } else {
        Vec::new()
    };
    init_files.sort();
    let init_sha256 = init_files
        .iter()
        .map(|name| {
            let path = Path::new(brush_dataset(&args)?).join(name);
            safe_path(&h.root, &path)?;
            if path.metadata()?.len() > 4 * 1024 * 1024 {
                return Err(invalid("initialization fixture exceeds 4 MiB"));
            }
            Ok((
                name.clone(),
                format!("{:x}", Sha256::digest(std::fs::read(path)?)),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let (sequence, fault) = {
        let mut calls = h.calls.lock().unwrap();
        let occurrence = calls
            .iter()
            .filter(|c| c.engine == engine && c.operation == operation)
            .count()
            + 1;
        let fault = h
            .scenario
            .faults
            .iter()
            .find(|f| f.operation == format!("{engine}:{operation}") && f.occurrence == occurrence)
            .cloned();
        let sequence = calls.len() + 1;
        if sequence > 4096 {
            return Err(invalid("scenario invocation budget exceeded"));
        }
        calls.push(Invocation {
            sequence,
            engine: engine.clone(),
            operation: operation.clone(),
            args: args.clone(),
            environment: environment
                .iter()
                .filter(|(k, _)| {
                    matches!(
                        k.to_str(),
                        Some(
                            "CUBECL_WGPU_DEFAULT_DEVICE"
                                | "DISABLE_LAYER_AMD_SWITCHABLE_GRAPHICS_1"
                                | "RUST_LOG"
                                | "RUST_BACKTRACE"
                        )
                    )
                })
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
                .collect(),
            working_directory: spec.working_directory.clone(),
            log_path: spec.log_path.clone(),
            outcome: "running".into(),
            dataset_init_files: init_files,
            dataset_init_sha256: init_sha256,
            run_id: None,
            pipeline_operation: None,
        });
        std::fs::write(
            h.root.join("invocations.json"),
            serde_json::to_vec_pretty(&*calls)?,
        )?;
        (sequence, fault)
    };
    emit(
        &spec,
        &lifecycle,
        ProcessUpdate::Started {
            process_id: 1_000_000 + sequence as u32,
        },
        false,
    );
    emit(
        &spec,
        &lifecycle,
        ProcessUpdate::Resources(crate::process::resources::ProcessResources {
            process_id: 1_000_000 + sequence as u32,
            sampled_at: chrono::Utc::now(),
            cpu_percent: Some(0.),
            memory_bytes: Some(1_048_576),
            gpus: vec![],
            gpu_status: "e2e-simulated".into(),
        }),
        false,
    );
    let mut log = if let Some(path) = &spec.log_path {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?;
        emit(
            &spec,
            &lifecycle,
            ProcessUpdate::LogOpened {
                path: path.clone(),
                offset: file.metadata()?.len(),
            },
            true,
        );
        writeln!(
            file,
            "E2E simulated {engine}:{operation}\narguments: {}",
            args.join(" ")
        )?;
        Some(file)
    } else {
        None
    };
    let result=async {
        if let Some(gate)=fault.as_ref().and_then(|f|f.gate.as_ref()) {
            h.pending.lock().unwrap().insert(gate.clone());
            loop {
                let changed=h.changed.notified();
                if h.released.lock().unwrap().contains(gate){break;}
                tokio::select!{ _=changed=>{},_=cancel.cancelled()=>{h.pending.lock().unwrap().remove(gate);return Err(SplatError::Cancelled);} }
            }
            h.pending.lock().unwrap().remove(gate);
        }
        let outcome=fault.as_ref().map(|f|f.outcome.as_str()).unwrap_or("success");
        if outcome=="io" {return Err(SplatError::Io(std::io::Error::from_raw_os_error(5)));}
        let mut code=0;
        let mut stderr=String::new();
        let mut stdout=match outcome {
            "geometry"=>{code=1;stderr="No good initial image pair found.\nFailed to create any sparse model\n".into();String::new()},
            "oom"=>{code=1;stderr="GPU out of memory: failed to allocate device memory\n".into();"Refine iter 200, 1000 splats.\n".into()},
            "export"=>{stderr=format!("Export at iteration {} failed: Export failed: final.ply.tmp: native storage error (os error 112)\n",argument(&args,"--total-train-iters")?);String::new()},
            "zero"=>{
                let path=PathBuf::from(argument(&args,"--export-path")?).join(argument(&args,"--export-name")?);
                safe_path(&h.root,&path)?;artifacts::gaussian(&path,0)?;String::new()
            },
            "crash"=>{code = 0xC0000005_u32 as i32;stderr="engine process terminated abnormally\n".into();String::new()},
            "success"=>success(h,&engine,&operation,&args,fault.as_ref().and_then(|f|f.registered),&device_name)?,
            _=>return Err(invalid(format!("unknown fault outcome {outcome}"))),
        };
        if engine=="brush" && outcome!="success" {
            stdout=format!("Created wgpu compute server on device AdapterInfo {{ name: \"{device_name}\" }}\n{stdout}");
        }
        for (stream,text)in [(ProcessStream::Stdout,&stdout),(ProcessStream::Stderr,&stderr)] {
            if let Some(file)=&mut log {file.write_all(text.as_bytes())?;}
            if !(matches!(stream,ProcessStream::Stdout)&&complete_limit.is_some()) {
                for line in text.lines(){emit(&spec,&lifecycle,ProcessUpdate::Line{stream,line:line.into()},false);}
            }
        }
        if complete_limit.is_some_and(|limit|stdout.len()>limit){return Err(invalid("structured stdout limit exceeded"));}
        Ok(ProcessOutput{success:code==0,exit_code:Some(code),termination:ProcessTermination{exit_code:Some(code),windows_status:cfg!(windows).then(||format!("0x{:08X}",code as u32)),..Default::default()},stdout,stderr})
    }.await;
    let status = match &result {
        Ok(output) => {
            if output.success {
                "success"
            } else {
                "failed"
            }
        }
        Err(SplatError::Cancelled) => "cancelled",
        Err(_) => "error",
    };
    {
        let mut calls = h.calls.lock().unwrap();
        calls[sequence - 1].outcome = status.into();
        std::fs::write(
            h.root.join("invocations.json"),
            serde_json::to_vec_pretty(&*calls)?,
        )?;
    }
    if let Some(mut file) = log {
        writeln!(file, "E2E execution {status}")?;
        file.flush()?;
        let termination = result
            .as_ref()
            .map(|o| o.termination.clone())
            .unwrap_or_default();
        emit(
            &spec,
            &lifecycle,
            ProcessUpdate::LogClosed {
                path: spec.log_path.clone().unwrap(),
                end: file.metadata()?.len(),
                success: status == "success",
                termination,
            },
            true,
        );
    }
    emit(
        &spec,
        &lifecycle,
        ProcessUpdate::Exited {
            executable: spec.executable.clone(),
            exit_code: result.as_ref().ok().and_then(|o| o.exit_code),
        },
        true,
    );
    result
}

fn success(
    h: &Harness,
    engine: &str,
    op: &str,
    args: &[String],
    registered: Option<usize>,
    device_name: &str,
) -> Result<String> {
    if engine == "colmap" && op == "help" {
        return Ok(help());
    }
    match (engine, op) {
        ("colmap", "feature_extractor") => artifacts::features(args),
        ("colmap", "vocab_tree_retriever") => {
            let candidates =
                std::fs::read_to_string(argument(args, "--database_image_list_path")?)?;
            let queries = std::fs::read_to_string(argument(args, "--query_image_list_path")?)?;
            let limit = argument(args, "--num_images")?
                .parse::<usize>()
                .map_err(invalid)?;
            Ok(queries
                .lines()
                .enumerate()
                .map(|(n, query)| {
                    format!(
                        "Querying for image {query} [{n}]\n{}",
                        candidates
                            .lines()
                            .filter(|name| *name != query)
                            .take(limit)
                            .map(|name| format!("image_name={name}, score=1.0\n"))
                            .collect::<String>()
                    )
                })
                .collect())
        }
        (
            "colmap",
            "sequential_matcher" | "exhaustive_matcher" | "matches_importer" | "vocab_tree_matcher",
        ) => artifacts::matches(args),
        ("colmap", "mapper" | "pose_prior_mapper") => artifacts::mapper(args, registered),
        ("brush", "train") => {
            let output = PathBuf::from(argument(args, "--export-path")?);
            let output = output.join(argument(args, "--export-name")?);
            safe_path(&h.root, &output)?;
            artifacts::gaussian(&output, 64)?;
            let device = device_name;
            // Deliberately differs from the 64-point published PLY: publication
            // must count the validated file, not borrow a progress-log count.
            Ok(format!("Created wgpu compute server on device AdapterInfo {{ name: \"{device}\", vendor: 4318 }}\nInitial splats initialized in 1ms: 64\nRefine iter {}, 128 splats.\n",argument(args,"--total-train-iters")?))
        }
        ("ffprobe", "probe" | "timestamps") => {
            let i = fixture(
                h,
                args.last().ok_or_else(|| invalid("probe input missing"))?,
            )?;
            let count = (i.fps * i.duration).round() as usize;
            if op == "timestamps" {
                return Ok((0..count)
                    .map(|n| {
                        format!(
                            "best_effort_timestamp={}|best_effort_timestamp_time={:.12}\n",
                            (i.timestamps.get(n).copied().unwrap_or(n as f64 / i.fps) * 1_000_000.)
                                .round() as i64,
                            i.timestamps.get(n).copied().unwrap_or(n as f64 / i.fps)
                        )
                    })
                    .collect());
            }
            Ok(json!({"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":i.width,"height":i.height,"time_base":"1/1000000","r_frame_rate":format!("{}/1000",(i.fps*1000.).round()),"avg_frame_rate":format!("{}/1000",(i.fps*1000.).round()),"duration":i.duration.to_string(),"nb_frames":count.to_string(),"pix_fmt":if i.alpha{"rgba"}else{"yuv420p"},"side_data_list":if i.erp{vec![json!({"side_data_type":"Spherical Mapping","projection":"equirectangular"})]}else{vec![]}}],"format":{"duration":i.duration.to_string(),"format_name":"mov,mp4"}}).to_string())
        }
        ("ffmpeg", "preview") => {
            let i = fixture(h, argument(args, "-i")?)?;
            let path = PathBuf::from(args.last().unwrap());
            safe_path(&h.root, &path)?;
            artifacts::write_image(&path, i.width, i.height, false, false)?;
            Ok(String::new())
        }
        ("ffmpeg", "extract") => extract(h, args),
        _ => Err(invalid(format!("no fixture adapter for {engine}:{op}"))),
    }
}
fn environment_device_name(environment: &[(OsString, OsString)]) -> Result<String> {
    let device = environment
        .iter()
        .find(|(key, _)| key == "CUBECL_WGPU_DEFAULT_DEVICE")
        .and_then(|(_, value)| value.to_str());
    match device {
        Some("DiscreteGpu(0)") => Ok("E2E GPU 0".into()),
        Some("DiscreteGpu(1)") => Ok("E2E GPU 1".into()),
        _ => Err(invalid("missing or undeclared Brush device binding")),
    }
}
fn extract(h: &Harness, args: &[String]) -> Result<String> {
    let i = fixture(h, argument(args, "-i")?)?;
    let script =
        argument(args, "-filter_script:v").or_else(|_| argument(args, "-filter_complex_script"))?;
    let text = std::fs::read_to_string(script)?;
    let mut indices = Vec::new();
    let mut rest = text.as_str();
    while let Some((_, tail)) = rest.split_once("eq(n\\,") {
        let value = tail
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        indices.push(value.parse::<u64>().map_err(invalid)?);
        rest = tail;
    }
    indices.sort_unstable();
    indices.dedup();
    let source_count = (i.fps * i.duration).round() as u64;
    if indices.iter().any(|index| *index >= source_count) {
        return Err(invalid(
            "extraction requested a frame outside the declared source; no frame is invented",
        ));
    }
    if indices.is_empty() {
        return Err(invalid(
            "only exact source-index extraction is supported by this fixture adapter",
        ));
    }
    let (width, height) = if let Some((_, tail)) = text.split_once("scale=") {
        let dims = tail
            .split(':')
            .take(2)
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(invalid)?;
        (dims[0], dims[1])
    } else {
        (i.width, i.height)
    };
    let patterns = args
        .iter()
        .filter(|s| s.contains("%06d"))
        .collect::<Vec<_>>();
    if patterns.is_empty() {
        return Err(invalid("missing exact extraction output pattern"));
    }
    let stats = args
        .windows(2)
        .filter(|w| w[0] == "-stats_enc_pre")
        .map(|w| w[1].as_str())
        .collect::<Vec<_>>();
    for pattern in patterns {
        let mask = pattern.ends_with(".png.png");
        for (n, _) in indices.iter().enumerate() {
            let path = PathBuf::from(pattern.replace("%06d", &format!("{:06}", n + 1)));
            safe_path(&h.root, &path)?;
            if mask {
                std::fs::create_dir_all(path.parent().unwrap())?;
                image::GrayImage::from_fn(width, height, |x, _| {
                    image::Luma([if i.transparent && x < width / 4 {
                        0
                    } else {
                        255
                    }])
                })
                .save(path)
                .map_err(invalid)?;
            } else {
                artifacts::write_image(&path, width, height, i.alpha, i.transparent)?;
            }
        }
    }
    let data = indices
        .iter()
        .enumerate()
        .map(|(n, source)| {
            format!(
                "{n}\t{source}\t{}\t1/1000000\n",
                (i.timestamps
                    .get(*source as usize)
                    .copied()
                    .unwrap_or(*source as f64 / i.fps)
                    * 1_000_000.)
                    .round() as i64
            )
        })
        .collect::<String>();
    for path in stats {
        std::fs::write(path, &data)?;
    }
    Ok(format!(
        "frame={}\ndrop_frames=0\nprogress=end\n",
        indices.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_identity_is_full_path_and_extraction_never_invents_missing_frames() {
        let dir = tempfile::tempdir().unwrap();
        let h=Harness{root:dir.path().into(),scenario:serde_json::from_value(json!({"name":"bounds","inputs":[{"name":"one/clip.mp4","kind":"video","fps":30},{"name":"two/clip.mp4","kind":"video","fps":60}],"allowedOperations":["ffmpeg:extract"]})).unwrap(),calls:Mutex::default(),released:Mutex::default(),pending:Mutex::default(),changed:Notify::new()};
        let input = h.root.join("fixtures/two/clip.mp4");
        assert_eq!(fixture(&h, &input.display().to_string()).unwrap().fps, 60.);
        let script = h.root.join("select.txt");
        std::fs::write(&script, "select=eq(n\\,9999)").unwrap();
        let output = h.root.join("frames/%06d.jpg");
        let args = vec![
            "-i".into(),
            input.display().to_string(),
            "-filter_script:v".into(),
            script.display().to_string(),
            output.display().to_string(),
        ];
        assert!(extract(&h, &args).is_err());
        assert!(!h.root.join("frames").exists());
    }
    #[test]
    fn dataset_is_positional_even_when_options_follow_it() {
        let args =
            ["--total-train-iters", "8000", "dataset", "--sh-degree", "0"].map(str::to_owned);
        assert_eq!(brush_dataset(&args).unwrap(), "dataset");
        assert!(brush_dataset(&["one".into(), "two".into()]).is_err());
    }
    #[test]
    fn device_binding_is_not_invented_or_borrowed_from_a_previous_call() {
        assert!(environment_device_name(&[]).is_err());
        assert!(environment_device_name(&[(
            "CUBECL_WGPU_DEFAULT_DEVICE".into(),
            "DiscreteGpu(9)".into()
        )])
        .is_err());
        assert_eq!(
            environment_device_name(&[(
                "CUBECL_WGPU_DEFAULT_DEVICE".into(),
                "DiscreteGpu(1)".into()
            )])
            .unwrap(),
            "E2E GPU 1"
        );
    }
    #[test]
    fn unknown_executable_has_no_real_process_fallback() {
        let spec = ProcessSpec {
            executable: "unexpected.exe".into(),
            args: vec![],
            working_directory: None,
            log_path: None,
            observer: None,
        };
        assert!(operation(&spec).is_err());
        assert!(host().is_err());
    }
}
