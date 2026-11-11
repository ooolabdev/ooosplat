//! Multi-input admission uses the same inspections, identities and Runner as the desktop.
use super::*;
use crate::pipeline::{
    generation_policy::GenerationPolicy,
    inputs::*,
    reshoot::{CameraRelation, ModelCondition, ReshootOptions},
};
use crate::project::ImageMatchingStrategy;
use crate::video::panorama::VideoProjection;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::OnceLock,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    inputs: GenerationInputs,
    quality: Quality,
    #[serde(default)]
    generation_policy: Option<GenerationPolicy>,
    #[serde(default)]
    source_project_id: Option<Uuid>,
}
#[derive(Clone)]
struct Preflight {
    id: Uuid,
    identity: String,
    inputs: GenerationInputs,
    quality: Quality,
    policy: GenerationPolicy,
    source: Option<crate::pipeline::runner::ReshootSourceInfo>,
    materials: Vec<MaterialAnalysis>,
    colmap: Option<crate::reconstruction::direct_input::Inspection>,
    estimate: Option<crate::pipeline::estimate::RuntimeEstimate>,
}
fn cache() -> &'static Mutex<VecDeque<Preflight>> {
    static CACHE: OnceLock<Mutex<VecDeque<Preflight>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
pub(super) fn schema() -> Value {
    let path = json!({"type":"string","minLength":1});
    let source = json!({"oneOf":[{"type":"object","additionalProperties":false,"required":["kind","path"],"properties":{"kind":{"enum":["video","imageFolder"]},"path":path}},{"type":"object","additionalProperties":false,"required":["kind","paths"],"properties":{"kind":{"const":"images"},"paths":{"type":"array","minItems":2,"items":path}}}]});
    let mut schema = json!({"oneOf":[{"type":"object","additionalProperties":false,"required":["mode","items"],"properties":{"mode":{"const":"materials"},"items":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,"required":["id","source"],"properties":{"id":{"type":"string","minLength":1},"source":source,"identity":{"type":["string","null"]},"projection":{"enum":["auto","perspective","equirectangular"],"default":"auto"},"imageMatchingStrategy":{"enum":["sequential","exhaustive"]},"cameraRelation":{"enum":["same","different","unsure"]},"confirmedSmallImages":{"type":"boolean"},"confirmedImageOrder":{"type":"boolean"},"frameRemovalFps":{"type":["number","null"],"minimum":0.0}}}}}},{"type":"object","additionalProperties":false,"required":["mode","input"],"properties":{"mode":{"const":"colmap"},"input":{"type":"object","additionalProperties":false,"required":["model","images"],"properties":{"model":path,"images":path,"database":{"type":["string","null"]},"identity":{"type":["string","null"]}}}}}]});
    if !crate::pipeline::input_admission::COLMAP_IMPORT_ENABLED {
        schema["oneOf"].as_array_mut().unwrap().retain(|branch| {
            branch
                .pointer("/properties/mode/const")
                .and_then(Value::as_str)
                != Some("colmap")
        });
    }
    schema
}
pub(super) fn extend_schema(schema: &mut Value) {
    schema["properties"]["inputs"] = self::schema();
    if let Some(required) = schema["required"].as_array_mut() {
        required.retain(|v| v != "input_path");
    }
    schema["oneOf"] = json!([{"required":["input_path"],"not":{"required":["inputs"]}},{"required":["inputs"],"not":{"required":["input_path"]}}]);
    if schema["properties"].get("answers").is_some() {
        schema["properties"]["answers"]["properties"]["inputs"] = json!({"type":"object","additionalProperties":{"type":"object","additionalProperties":false,"properties":{"input_projection":{"enum":["perspective","equirectangular"]},"small_images":{"type":"boolean"},"image_order":{"enum":["sequential","exhaustive"]},"camera_relation":{"enum":["same","different","unsure"]}}}});
    }
}
pub(super) fn authorize(tools: &McpTools, inputs: &GenerationInputs) -> Result<()> {
    // Authorize declared roots before enumerating members or parsing model data.
    match inputs {
        GenerationInputs::Colmap { input } => {
            authorize_input(&input.model, &tools.settings.input_roots)?;
            for name in [
                "cameras.bin",
                "images.bin",
                "points3D.bin",
                "cameras.txt",
                "images.txt",
                "points3D.txt",
            ] {
                let file = input.model.join(name);
                if file.exists() {
                    authorize_input(&file, &tools.settings.input_roots)?;
                }
            }
            authorize_input(&input.images, &tools.settings.input_roots)?;
            if let Some(db) = &input.database {
                authorize_input(
                    if db.exists() {
                        db
                    } else {
                        db.parent().unwrap_or(Path::new(""))
                    },
                    &tools.settings.input_roots,
                )?;
            }
        }
        GenerationInputs::Materials { items } => {
            for item in items {
                match &item.source {
                    InputSource::Images { paths } => {
                        for path in paths {
                            authorize_input(path, &tools.settings.input_roots)?;
                        }
                    }
                    source => {
                        authorize_input(source.path(), &tools.settings.input_roots)?;
                    }
                }
            }
        }
    }
    // This helper admits new MCP input only. Check capability/type after the
    // declared paths are authorized, before Video's extension validation can
    // turn a database selection into an unrelated INVALID_PATH rejection.
    crate::pipeline::input_admission::ensure_new_inputs_supported(inputs)?;
    for path in inputs.authorized_paths()? {
        authorize_input(&path, &tools.settings.input_roots)?;
    }
    Ok(())
}
async fn preflight(tools: &McpTools, mut r: Request) -> Result<Preflight> {
    if matches!(&r.inputs, GenerationInputs::Colmap { .. }) {
        // Capability rejection does not read any caller-supplied path.
        crate::pipeline::input_admission::ensure_colmap_import_enabled()?;
    }
    authorize(tools, &r.inputs)?;
    r.inputs = r.inputs.canonicalized()?;
    let policy = normalized_new_policy(r.generation_policy)?;
    let source = if let Some(id) = r.source_project_id {
        Some(
            crate::pipeline::runner::PipelineRunner::new(tools.engines.clone(), |_| {})
                .with_invocation_access(Some(
                    crate::pipeline::access::InvocationAccess::restricted(
                        tools.settings.input_roots.clone(),
                    ),
                ))
                .inspect_reshoot_source_v2(id)
                .await?,
        )
    } else {
        None
    };
    if source.is_some() && matches!(r.inputs, GenerationInputs::Colmap { .. }) {
        return Err(ServiceError::new("INVALID_ARGUMENT", INVALID_COLMAP));
    }
    let mut materials = vec![];
    let mut colmap = None;
    match &r.inputs {
        GenerationInputs::Materials { items } => {
            for item in items {
                materials.push(inspect_material(&tools.engines, item, r.quality, policy).await?);
            }
        }
        GenerationInputs::Colmap { input } => {
            colmap = Some(crate::reconstruction::direct_input::inspect(input)?)
        }
    }
    let key = serde_json::to_vec(&json!([
        r.inputs,
        r.quality,
        policy,
        source.as_ref().map(|s| &s.source_identity),
        materials.iter().map(|a| &a.identity).collect::<Vec<_>>(),
        colmap.as_ref().map(|c| &c.identity)
    ]))?;
    let identity = format!("{:x}", Sha256::digest(key));
    let mut cache = cache().lock().await;
    if let Some(p) = cache.iter().find(|p| p.identity == identity) {
        return Ok(p.clone());
    }
    let p = Preflight {
        id: Uuid::new_v4(),
        identity,
        inputs: r.inputs,
        quality: r.quality,
        policy,
        source,
        estimate: crate::pipeline::inputs::estimate(
            &materials,
            r.quality,
            &crate::project::catalog::runtime_samples().await,
        ),
        materials,
        colmap,
    };
    if cache.len() >= 64 {
        cache.pop_front();
    }
    cache.push_back(p.clone());
    Ok(p)
}

fn normalized_new_policy(policy: Option<GenerationPolicy>) -> Result<GenerationPolicy> {
    let policy = policy.unwrap_or_default().validate()?;
    Ok(GenerationPolicy {
        version: crate::pipeline::generation_policy::VERSION,
        ..policy
    }
    .validate()?)
}

/// Legacy transport facade: one stable material ID, the same canonicalization,
/// inspection and preflight identity as the ordered-input endpoint.
pub(super) async fn inspect_single(
    tools: &McpTools,
    path: PathBuf,
    quality: Quality,
    policy: GenerationPolicy,
    source_project_id: Option<Uuid>,
) -> Result<(
    Uuid,
    MaterialAnalysis,
    Option<crate::pipeline::runner::ReshootSourceInfo>,
)> {
    let source = if path.is_dir() {
        InputSource::ImageFolder { path }
    } else {
        InputSource::Video { path }
    };
    let p = preflight(
        tools,
        Request {
            inputs: GenerationInputs::Materials {
                items: vec![MaterialInput {
                    id: "input".into(),
                    source,
                    identity: None,
                    projection: VideoProjection::Auto,
                    image_matching_strategy: Default::default(),
                    camera_relation: None,
                    confirmed_small_images: false,
                    confirmed_image_order: false,
                    frame_removal_fps: None,
                }],
            },
            quality,
            generation_policy: Some(policy),
            source_project_id,
        },
    )
    .await?;
    Ok((
        p.id,
        p.materials.into_iter().next().expect("one material"),
        p.source,
    ))
}

/// Preserve the old inspection tool's response shape, while resolving the
/// incoming material independently of the source's folded projection field.
pub(super) async fn inspect_reshoot_single(
    tools: &McpTools,
    path: PathBuf,
    quality: Quality,
    source_project_id: Uuid,
) -> Result<Value> {
    let source = if path.is_dir() {
        InputSource::ImageFolder { path }
    } else {
        InputSource::Video { path }
    };
    let p = preflight(
        tools,
        Request {
            inputs: GenerationInputs::Materials {
                items: vec![MaterialInput {
                    id: "input".into(),
                    source,
                    identity: None,
                    projection: VideoProjection::Auto,
                    image_matching_strategy: Default::default(),
                    camera_relation: None,
                    confirmed_small_images: false,
                    confirmed_image_order: false,
                    frame_removal_fps: None,
                }],
            },
            quality,
            generation_policy: None,
            source_project_id: Some(source_project_id),
        },
    )
    .await?;
    let a = &p.materials[0];
    let video = a.video.as_ref();
    let images = a.image_sequence.as_ref();
    let (width, height, alpha) = if let Some(v) = video {
        (v.width, v.height, v.has_alpha)
    } else {
        let i = images.expect("image material");
        (i.width, i.height, i.has_alpha)
    };
    let count = a.plan.as_ref().map_or_else(
        || images.map_or(0, |i| i.image_count),
        |f| f.estimated_frames,
    );
    Ok(json!({"source":p.source,"input":{
        "video":a.video,"sourceIdentity":a.legacy_identity,"projectionDetection":a.projection_detection,
        "imageSequence":a.image_sequence,"inputType":if video.is_some(){"video"}else{"images"},
        "imageCount":images.map(|i|i.image_count),"duration":video.map(|v|v.duration),
        "preparedWidth":width,"preparedHeight":height,"estimatedFrames":count,"hasAlpha":alpha,
        "maskCount":if alpha{count}else{0},"compatible":true,"incompatibilityReason":null,"estimate":p.estimate
    }}))
}

pub(super) fn single_questions(
    inspection: &generation::Inspection,
    answers: Option<&generation::Answers>,
) -> Vec<Value> {
    let source = if inspection.analysis.input_type == crate::project::ProjectInputType::Images {
        InputSource::ImageFolder {
            path: inspection.path.clone(),
        }
    } else {
        InputSource::Video {
            path: inspection.path.clone(),
        }
    };
    let mapped = answers.map(|a| Answers {
        model_condition: a.model_condition,
        inputs: BTreeMap::from([(
            "input".into(),
            SourceAnswers {
                input_projection: a.input_projection,
                small_images: a.small_images,
                image_order: a.image_order,
                camera_relation: a.camera_relation,
            },
        )]),
    });
    let p = Preflight {
        id: inspection.id,
        identity: inspection.identity.clone(),
        inputs: GenerationInputs::Materials {
            items: vec![MaterialInput {
                id: "input".into(),
                source,
                identity: None,
                projection: VideoProjection::Auto,
                image_matching_strategy: Default::default(),
                camera_relation: None,
                confirmed_small_images: false,
                confirmed_image_order: false,
                frame_removal_fps: None,
            }],
        },
        quality: inspection.quality,
        policy: inspection.policy,
        source: inspection.source.clone(),
        colmap: None,
        estimate: None,
        materials: vec![MaterialAnalysis {
            id: "input".into(),
            identity: inspection.identity.clone(),
            legacy_identity: None,
            video: inspection.analysis.video.clone(),
            image_sequence: inspection.analysis.image_sequence.clone(),
            dimensions: vec![],
            projection_detection: inspection.analysis.projection_detection.clone(),
            plan: None,
            frame_sampling_range: None,
        }],
    };
    questions(&p, mapped.as_ref())
        .into_iter()
        .map(|mut q| {
            q.as_object_mut().unwrap().remove("source_id");
            q
        })
        .collect()
}
fn question(
    id: &str,
    key: &str,
    title: &str,
    description: Option<&str>,
    options: &[(&str, &str)],
    count: u64,
) -> Value {
    json!({"source_id":id,"key":key,"title":generation::text(title,count),"description":description.map(|s|generation::text(s,count)),"options":options.iter().map(|(value,label)|json!({"value":if key=="small_images"{json!(true)}else{json!(value)},"label":generation::text(label,count)})).collect::<Vec<_>>()})
}
fn questions(p: &Preflight, answers: Option<&Answers>) -> Vec<Value> {
    let mut questions = vec![];
    if p.source.is_some() && answers.is_none_or(|a| a.model_condition.is_none()) {
        questions.push(question(
            "",
            "model_condition",
            "reshoot.conditionTitle",
            None,
            &[
                ("good", "reshoot.condition.good"),
                ("appearance", "reshoot.condition.appearance"),
                ("geometry", "reshoot.condition.geometry"),
                ("unsure", "reshoot.condition.unsure"),
            ],
            0,
        ));
        if let Some(options) = questions
            .last_mut()
            .and_then(|q| q["options"].as_array_mut())
        {
            for option in options {
                option["description"] = generation::text(
                    &format!(
                        "reshoot.condition.{}Hint",
                        option["value"].as_str().unwrap()
                    ),
                    0,
                );
            }
        }
    }
    if let GenerationInputs::Materials { items } = &p.inputs {
        for (item, a) in items.iter().zip(&p.materials) {
            let answer = answers.and_then(|answers| answers.inputs.get(&item.id));
            let count = a.image_sequence.as_ref().map_or(0, |i| i.image_count);
            let images = a.image_sequence.is_some();
            let projection = answer.and_then(|a| a.input_projection).unwrap_or(
                if a.projection_detection.confirmed_erp {
                    VideoProjection::Equirectangular
                } else if !a.projection_detection.requires_choice {
                    VideoProjection::Perspective
                } else {
                    item.projection
                },
            );
            if a.projection_detection.requires_choice && projection == VideoProjection::Auto {
                let mut options = vec![(
                    "perspective",
                    if images {
                        "source.images"
                    } else {
                        "panorama.ordinaryDetected"
                    },
                )];
                if a.projection_detection.supported_erp {
                    options.push((
                        "equirectangular",
                        if images {
                            "source.panoramaImages"
                        } else {
                            "source.fullPanorama"
                        },
                    ));
                }
                questions.push(question(
                    &item.id,
                    "input_projection",
                    if images {
                        "source.chooseImageType"
                    } else {
                        "source.chooseType"
                    },
                    Some(if images {
                        "source.ambiguousImages"
                    } else {
                        "panorama.ambiguous"
                    }),
                    &options,
                    count,
                ));
            }
            if p.source.is_some()
                && projection == VideoProjection::Perspective
                && answer
                    .and_then(|a| a.camera_relation)
                    .or(item.camera_relation)
                    .is_none()
            {
                questions.push(question(
                    &item.id,
                    "camera_relation",
                    "reshoot.cameraTitle",
                    Some("reshoot.cameraDescription"),
                    &[
                        ("same", "reshoot.camera.same"),
                        ("different", "reshoot.camera.different"),
                        ("unsure", "reshoot.camera.unsure"),
                    ],
                    count,
                ));
            }
            if images
                && count < 30
                && p.source.is_none()
                && !item.confirmed_small_images
                && answer.and_then(|a| a.small_images) != Some(true)
            {
                questions.push(question(
                    &item.id,
                    "small_images",
                    "dialog.smallSequenceTitle",
                    Some("dialog.smallSequence"),
                    &[("accepted", "preflight.acceptWarning")],
                    count,
                ));
            }
            if images
                && count > 100
                && !item.confirmed_image_order
                && answer.and_then(|a| a.image_order).is_none()
            {
                questions.push(question(
                    &item.id,
                    "image_order",
                    "preflight.imageOrderTitle",
                    Some("preflight.imageOrderDescription"),
                    &[
                        ("sequential", "preflight.ordered"),
                        ("exhaustive", "preflight.unordered"),
                    ],
                    count,
                ));
            }
        }
    }
    questions
}
fn public(p: &Preflight) -> Result<Value> {
    let mut analyses = serde_json::to_value(&p.materials)?;
    for a in analyses.as_array_mut().into_iter().flatten() {
        if let Some(plan) = a.get_mut("plan").and_then(Value::as_object_mut) {
            plan.remove("selectedFrames");
            plan.remove("candidateFrames");
        }
        if let Some(d) = a
            .get_mut("projectionDetection")
            .and_then(Value::as_object_mut)
        {
            d.remove("previewJpeg");
        }
    }
    Ok(
        json!({"preflight_id":p.id,"input_source_identity":p.identity,"input":{"inputs":p.inputs,"materials":snake(analyses),"colmap":p.colmap.as_ref().map(|v|snake(serde_json::to_value(v).unwrap()))},"source":p.source.as_ref().map(|v|snake(serde_json::to_value(v).unwrap())),"generation_policy":snake(serde_json::to_value(p.policy)?),"plan":{"estimated_frames":p.colmap.as_ref().map(|c|c.image_count).unwrap_or_else(||p.materials.iter().map(|a|a.plan.as_ref().map_or_else(||a.image_sequence.as_ref().map_or(0,|i|i.image_count),|f|f.estimated_frames)).sum::<u64>())},"estimate":p.estimate,"plan_is_estimate":true,"max_splats":null,"questions":questions(p,None),"gps":{"status":"pending"}}),
    )
}
pub(super) async fn inspect(tools: &McpTools, args: Value) -> Result<Value> {
    public(&preflight(tools, parse(args)?).await?)
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceAnswers {
    input_projection: Option<VideoProjection>,
    small_images: Option<bool>,
    image_order: Option<ImageMatchingStrategy>,
    camera_relation: Option<CameraRelation>,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answers {
    model_condition: Option<ModelCondition>,
    #[serde(default)]
    inputs: BTreeMap<String, SourceAnswers>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    inputs: GenerationInputs,
    quality: Quality,
    client_request_id: String,
    generation_policy: Option<GenerationPolicy>,
    source_project_id: Option<Uuid>,
    preflight_id: Option<Uuid>,
    input_source_identity: Option<String>,
    #[serde(default)]
    answers: Answers,
}
pub(super) async fn create(tools: &McpTools, args: Value) -> Result<Value> {
    let mut intent = args.clone();
    let object = intent
        .as_object_mut()
        .ok_or_else(|| ServiceError::new("INVALID_ARGUMENT", "Expected object"))?;
    object.remove("preflight_id");
    object.remove("client_request_id");
    let request: Create = parse(args)?;
    let legacy_intent = intent.clone();
    let normalized_policy = normalized_new_policy(request.generation_policy)?;
    intent["generation_policy"] = serde_json::to_value(normalized_policy)?;
    if let Some(old) = tools
        .tasks
        .all()
        .await?
        .into_iter()
        .find(|t| t.client_request_id.as_ref() == Some(&request.client_request_id))
    {
        let saved_intent = old
            .request_parameters
            .as_ref()
            .and_then(|p| p.get("mcp_intent"));
        if saved_intent == Some(&intent) || saved_intent == Some(&legacy_intent) {
            return public_task(old);
        }
        return Err(ServiceError::new(
            "IDEMPOTENCY_CONFLICT",
            "client_request_id already identifies different parameters",
        ));
    }
    let p = preflight(
        tools,
        Request {
            inputs: request.inputs,
            quality: request.quality,
            generation_policy: Some(normalized_policy),
            source_project_id: request.source_project_id,
        },
    )
    .await?;
    if request.answers.inputs.keys().any(|id|!matches!(&p.inputs,GenerationInputs::Materials{items} if items.iter().any(|i|&i.id==id))){return Err(ServiceError::new("INPUT_CHANGED","Answers refer to another input"));}
    if request.preflight_id.is_some_and(|id| id != p.id)
        || request
            .input_source_identity
            .as_ref()
            .is_some_and(|id| id != &p.identity)
    {
        return Err(ServiceError::new(
            "INPUT_CHANGED",
            "Input or configuration changed; inspect and confirm again",
        ));
    }
    let mut inputs = p.inputs.clone();
    if let GenerationInputs::Materials { items } = &mut inputs {
        for (item, a) in items.iter_mut().zip(&p.materials) {
            let answer = request.answers.inputs.get(&item.id);
            let count = a.image_sequence.as_ref().map(|i| i.image_count);
            item.identity = Some(a.identity.clone());
            item.projection = answer.and_then(|a| a.input_projection).unwrap_or(
                if a.projection_detection.confirmed_erp {
                    VideoProjection::Equirectangular
                } else if !a.projection_detection.requires_choice {
                    VideoProjection::Perspective
                } else {
                    item.projection
                },
            );
            item.confirmed_small_images = p.source.is_some()
                || answer.and_then(|a| a.small_images) == Some(true)
                || item.confirmed_small_images;
            item.confirmed_image_order =
                answer.and_then(|a| a.image_order).is_some() || item.confirmed_image_order;
            item.image_matching_strategy = if count.is_some_and(|n| n > 100) {
                answer
                    .and_then(|a| a.image_order)
                    .unwrap_or(item.image_matching_strategy)
            } else {
                ImageMatchingStrategy::Exhaustive
            };
            item.camera_relation = answer
                .and_then(|a| a.camera_relation)
                .or(item.camera_relation);
            if item.projection == VideoProjection::Equirectangular {
                item.camera_relation = None;
            }
        }
    } else if let GenerationInputs::Colmap { input } = &mut inputs {
        input.identity = p.colmap.as_ref().map(|c| c.identity.clone());
    }
    let pending = questions(&p, Some(&request.answers));
    if !pending.is_empty() {
        let mut error = ServiceError::new(
            "CONFIRMATION_REQUIRED",
            "Ask the user the pending input questions before creating a task",
        );
        error.details = Some(
            json!({"preflight_id":p.id,"input_source_identity":p.identity,"questions":pending}),
        );
        return Err(error);
    }
    crate::pipeline::inputs::validate(&tools.engines, &inputs, p.quality, p.policy).await?;
    let options = p.source.as_ref().map(|source| ReshootOptions {
        condition: request.answers.model_condition.unwrap(),
        camera_relation: None,
        allow_fallback: p.policy.auto_retry_enabled,
        source_identity: source.source_identity.clone(),
        input_identity: serde_json::to_string(
            &p.materials.iter().map(|a| &a.identity).collect::<Vec<_>>(),
        )
        .unwrap(),
        projection: VideoProjection::Perspective,
        image_matching_strategy: ImageMatchingStrategy::Exhaustive,
    });
    let parameters = json!({"inputs":inputs,"generationPolicy":p.policy,"source_project_id":request.source_project_id,"reshootOptions":options,"mcp_intent":intent});
    let root = crate::project::catalog::load_settings()
        .await?
        .projects_root;
    public_task(
        tools
            .tasks
            .create(
                None,
                inputs.primary_path().to_path_buf(),
                p.quality,
                root,
                true,
                "mcp",
                Some(request.client_request_id),
                Some(parameters),
            )
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tools(root: &Path) -> McpTools {
        McpTools {
            app: None,
            tasks: TaskService::at(root.join("registry")),
            engines: crate::pipeline::runner::default_engine_paths(Some(root.join("no-engines"))),
            settings: McpSettings {
                enabled: true,
                port: 39877,
                input_roots: vec![std::fs::canonicalize(root).unwrap()],
            },
            telemetry: None,
        }
    }
    fn folder(path: &Path, count: usize) {
        std::fs::create_dir_all(path).unwrap();
        for i in 0..count {
            image::RgbImage::new(4, 3)
                .save(path.join(format!("{i}.png")))
                .unwrap();
        }
    }
    #[tokio::test]
    async fn disabled_colmap_preflight_and_creation_create_no_task() {
        let root = tempfile::tempdir().unwrap();
        let tools = tools(root.path());
        let inputs = json!({"mode":"colmap","input":{"model":root.path().join("missing-model"),"images":root.path().join("missing-images")}});
        let inspected = inspect(&tools, json!({"inputs":inputs,"quality":"balanced"}))
            .await
            .unwrap_err();
        assert_eq!(inspected.code, "UNSUPPORTED_INPUT");
        assert_eq!(
            inspected.reason,
            Some(crate::error::InputAdmissionReason::ColmapDisabled)
        );
        let created = create(
            &tools,
            json!({"inputs":inputs,"quality":"balanced","client_request_id":"disabled-colmap"}),
        )
        .await
        .unwrap_err();
        assert_eq!(created.code, "UNSUPPORTED_INPUT");
        assert_eq!(created.reason, inspected.reason);
        assert!(tools.tasks.all().await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn multi_input_admission_is_per_source_gated_idempotent_and_configuration_bound() {
        let root = tempfile::tempdir().unwrap();
        let tools = tools(root.path());
        folder(&root.path().join("left"), 101);
        folder(&root.path().join("right"), 101);
        let inputs = json!({"mode":"materials","items":[{"id":"left","source":{"kind":"imageFolder","path":root.path().join("left")}},{"id":"right","source":{"kind":"imageFolder","path":root.path().join("right")}}]});
        let request = json!({"inputs":inputs,"quality":"fast","generation_policy":{"version":5}});
        let p = inspect(&tools, request.clone()).await.unwrap();
        assert_eq!(
            p["generation_policy"]["version"],
            crate::pipeline::generation_policy::VERSION
        );
        assert_eq!(p["questions"].as_array().unwrap().len(), 2);
        assert_eq!(p["questions"][0]["source_id"], "left");
        assert!(p["input"]["materials"][0]
            .pointer("/projection_detection/preview_jpeg")
            .is_none());
        let mut create_request = request;
        create_request["client_request_id"] = json!("multi-1");
        create_request["preflight_id"] = p["preflight_id"].clone();
        create_request["input_source_identity"] = p["input_source_identity"].clone();
        let error = create(&tools, create_request.clone()).await.unwrap_err();
        assert_eq!(error.code, "CONFIRMATION_REQUIRED");
        assert!(tools.tasks.all().await.unwrap().is_empty());
        create_request["answers"] = json!({"inputs":{"left":{"image_order":"sequential"},"right":{"image_order":"exhaustive"}}});
        let created = create(&tools, create_request.clone()).await.unwrap();
        assert_eq!(created["status"], "created");
        let task = tools.tasks.all().await.unwrap().pop().unwrap();
        assert_eq!(
            task.request_parameters
                .as_ref()
                .unwrap()
                .pointer("/generationPolicy/version")
                .unwrap(),
            crate::pipeline::generation_policy::VERSION
        );
        assert_eq!(
            task.request_parameters
                .as_ref()
                .unwrap()
                .pointer("/inputs/items/0/imageMatchingStrategy")
                .unwrap(),
            "sequential"
        );
        assert_eq!(
            task.request_parameters
                .as_ref()
                .unwrap()
                .pointer("/inputs/items/1/imageMatchingStrategy")
                .unwrap(),
            "exhaustive"
        );
        assert_eq!(
            create(&tools, create_request.clone()).await.unwrap()["task_id"],
            created["task_id"]
        );
        assert_eq!(tools.tasks.all().await.unwrap().len(), 1);
        create_request["client_request_id"] = json!("multi-2");
        create_request["quality"] = json!("high");
        assert_eq!(
            create(&tools, create_request).await.unwrap_err().code,
            "INPUT_CHANGED"
        );
    }

    #[tokio::test]
    async fn legacy_multi_input_policy_is_normalized_before_preflight_identity() {
        let root = tempfile::tempdir().unwrap();
        let tools = tools(root.path());
        folder(&root.path().join("images"), 30);
        let inputs = json!({"mode":"materials","items":[{"id":"images","source":{"kind":"imageFolder","path":root.path().join("images")}}]});
        let legacy = inspect(
            &tools,
            json!({"inputs":inputs,"quality":"balanced","generation_policy":{"version":5}}),
        )
        .await
        .unwrap();
        let current = inspect(
            &tools,
            json!({"inputs":inputs,"quality":"balanced","generation_policy":{"version":crate::pipeline::generation_policy::VERSION}}),
        )
        .await
        .unwrap();
        assert_eq!(legacy["preflight_id"], current["preflight_id"]);
        assert_eq!(
            legacy["generation_policy"]["version"],
            crate::pipeline::generation_policy::VERSION
        );
    }

    #[test]
    fn ambiguous_reshoot_projection_defers_camera_question_until_perspective_answer() {
        let material = MaterialInput {
            id: "ambiguous".into(),
            source: InputSource::Video {
                path: PathBuf::from("ambiguous.mov"),
            },
            identity: None,
            projection: VideoProjection::Auto,
            image_matching_strategy: Default::default(),
            camera_relation: None,
            confirmed_small_images: false,
            confirmed_image_order: false,
            frame_removal_fps: None,
        };
        let analysis = MaterialAnalysis {
            id: material.id.clone(),
            identity: "material".into(),
            legacy_identity: None,
            video: Some(crate::video::VideoInfo {
                duration: 10.0,
                width: 1920,
                height: 960,
                fps: 30.0,
                total_frames: 300,
                codec: "h264".into(),
                rotation: 0,
                pixel_format: "yuv420p".into(),
                has_alpha: false,
            }),
            image_sequence: None,
            dimensions: vec![],
            projection_detection: crate::video::panorama::ProjectionDetection {
                source_identity: "material".into(),
                unsupported: false,
                confirmed_erp: false,
                requires_choice: true,
                supported_erp: true,
                reason: String::new(),
                preview_jpeg: vec![],
            },
            plan: None,
            frame_sampling_range: None,
        };
        let p = Preflight {
            id: Uuid::new_v4(),
            identity: "preflight".into(),
            inputs: GenerationInputs::Materials {
                items: vec![material],
            },
            quality: Quality::Balanced,
            policy: GenerationPolicy::default(),
            source: Some(crate::pipeline::runner::ReshootSourceInfo {
                source_identity: "source".into(),
                projection: VideoProjection::Perspective,
                warm_available: true,
                project_id: Uuid::nil().to_string(),
                project_name: "source".into(),
                quality: Quality::Balanced,
                planner_enabled: true,
                camera_id: 1,
                camera_model: "SIMPLE_RADIAL".into(),
                width: 1920,
                height: 960,
                source_image_count: 10,
                eligible: true,
                reason: None,
                imported: false,
            }),
            materials: vec![analysis],
            colmap: None,
            estimate: None,
        };
        let initial = questions(&p, None);
        assert_eq!(
            initial
                .iter()
                .map(|q| q["key"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["model_condition", "input_projection"]
        );

        let mut perspective = Answers {
            model_condition: Some(ModelCondition::Good),
            ..Default::default()
        };
        perspective.inputs.insert(
            "ambiguous".into(),
            SourceAnswers {
                input_projection: Some(VideoProjection::Perspective),
                ..Default::default()
            },
        );
        assert_eq!(
            questions(&p, Some(&perspective))[0]["key"],
            "camera_relation"
        );

        perspective
            .inputs
            .get_mut("ambiguous")
            .unwrap()
            .input_projection = Some(VideoProjection::Equirectangular);
        assert!(questions(&p, Some(&perspective)).is_empty());
    }
    #[tokio::test]
    async fn direct_colmap_readers_remain_available_but_new_preflight_and_create_are_disabled() {
        let root = tempfile::tempdir().unwrap();
        let input = crate::reconstruction::direct_input::tests::fixture(root.path(), false);
        let existing = crate::reconstruction::direct_input::inspect(&input).unwrap();
        assert_eq!(existing.image_count, 2);
        assert!(!existing.reshoot_eligible);
        let tools = tools(root.path());
        let request = json!({"inputs":{"mode":"colmap","input":input},"quality":"fast"});
        let error = inspect(&tools, request.clone()).await.unwrap_err();
        assert_eq!(error.code, "UNSUPPORTED_INPUT");
        assert_eq!(
            error.reason,
            Some(crate::error::InputAdmissionReason::ColmapDisabled)
        );
        let mut invalid = request.clone();
        invalid["input_path"] = json!(root.path());
        assert_eq!(
            inspect(&tools, invalid).await.unwrap_err().code,
            "INVALID_ARGUMENT"
        );
        let mut create_request = request;
        create_request["client_request_id"] = json!("direct-1");
        let error = create(&tools, create_request).await.unwrap_err();
        assert_eq!(error.code, "UNSUPPORTED_INPUT");
        assert!(tools.tasks.all().await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn every_loose_image_and_nested_colmap_image_requires_authorization() {
        let root = tempfile::tempdir().unwrap();
        let allowed = root.path().join("allowed");
        let outside = root.path().join("outside");
        folder(&allowed, 2);
        folder(&outside, 2);
        let tools = tools(&allowed);
        let input = GenerationInputs::Materials {
            items: vec![MaterialInput {
                id: "loose".into(),
                source: InputSource::Images {
                    paths: vec![allowed.join("0.png"), outside.join("1.png")],
                },
                identity: None,
                projection: VideoProjection::Perspective,
                image_matching_strategy: Default::default(),
                camera_relation: None,
                confirmed_small_images: true,
                confirmed_image_order: false,
                frame_removal_fps: None,
            }],
        };
        assert_eq!(
            authorize(&tools, &input).unwrap_err().code,
            "PATH_NOT_AUTHORIZED"
        );
    }
}
