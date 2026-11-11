//! MCP admission only: reuse GUI analysis/planning and the shared Runner.
use super::*;
use crate::{
    commands::InputAnalysis,
    pipeline::{
        generation_policy::GenerationPolicy,
        inputs::{GenerationInputs, InputSource, MaterialInput},
        reshoot::{CameraRelation, ModelCondition, ReshootOptions},
        runner::ReshootSourceInfo,
    },
    project::ImageMatchingStrategy,
    video::panorama::{source_identity, VideoProjection},
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::OnceLock,
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Answers {
    pub input_projection: Option<VideoProjection>,
    pub small_images: Option<bool>,
    pub image_order: Option<ImageMatchingStrategy>,
    pub model_condition: Option<ModelCondition>,
    pub camera_relation: Option<CameraRelation>,
}

/// The legacy single-path transport is only an admission adapter. Once its
/// existing questions have been answered, execute the same material work order
/// as GUI/multi-input requests; never send a new task to a legacy Runner.
fn mapped_input(
    inspection: &Inspection,
    answers: &Answers,
    projection: VideoProjection,
    strategy: ImageMatchingStrategy,
    identity: String,
) -> GenerationInputs {
    let source = if inspection.analysis.input_type == crate::project::ProjectInputType::Video {
        InputSource::Video {
            path: inspection.path.clone(),
        }
    } else {
        InputSource::ImageFolder {
            path: inspection.path.clone(),
        }
    };
    GenerationInputs::Materials {
        items: vec![MaterialInput {
            id: "input".into(),
            source,
            identity: Some(identity),
            projection,
            image_matching_strategy: strategy,
            camera_relation: if projection == VideoProjection::Equirectangular {
                None
            } else {
                answers.camera_relation
            },
            // Reshoots never asked the small-sequence question in this transport;
            // retain that already-validated admission rule, not a guessed answer.
            confirmed_small_images: inspection.source.is_some()
                || answers.small_images == Some(true),
            confirmed_image_order: answers.image_order.is_some(),
            frame_removal_fps: None,
        }],
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InspectRequest {
    pub input_path: String,
    pub quality: Quality,
    pub generation_policy: Option<GenerationPolicy>,
    pub source_project_id: Option<Uuid>,
}
#[derive(Clone)]
pub(super) struct Inspection {
    pub id: Uuid,
    pub path: PathBuf,
    pub identity: String,
    pub quality: Quality,
    pub policy: GenerationPolicy,
    pub source: Option<ReshootSourceInfo>,
    pub analysis: InputAnalysis,
}
fn inspections() -> &'static Mutex<VecDeque<Inspection>> {
    static CACHE: OnceLock<Mutex<VecDeque<Inspection>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
/// Reuse approved UI text, embedded at build time; never read arbitrary files.
pub(super) fn text(key: &str, count: u64) -> Value {
    let source = include_str!("../../../src/i18n/index.tsx");
    let (zh, en) = source
        .split_once("const en:")
        .expect("UI locale dictionaries");
    let read = |source: &str| {
        let marker = format!("\"{key}\": ");
        let tail = source
            .split_once(&marker)
            .map(|(_, value)| value)
            .unwrap_or("\"\"");
        serde_json::Deserializer::from_str(tail)
            .into_iter::<String>()
            .next()
            .and_then(std::result::Result::ok)
            .unwrap_or_default()
            .replace("{count}", &count.to_string())
    };
    json!({"zh-CN":read(zh),"en":read(en)})
}
impl Inspection {
    pub fn questions(&self) -> Vec<Value> {
        materials::single_questions(self, None)
    }
    pub fn validate_answers(
        &self,
        answers: &Answers,
    ) -> Result<(
        VideoProjection,
        ImageMatchingStrategy,
        Option<ReshootOptions>,
    )> {
        if answers.input_projection == Some(VideoProjection::Auto) {
            return Err(ServiceError::new(
                "INVALID_ARGUMENT",
                "Projection answers must be explicit",
            ));
        }
        let missing: Vec<_> = materials::single_questions(self, Some(answers))
            .into_iter()
            .filter(|q| match q["key"].as_str().unwrap() {
                "input_projection" => !matches!(
                    answers.input_projection,
                    Some(VideoProjection::Perspective | VideoProjection::Equirectangular)
                ),
                "small_images" => answers.small_images != Some(true),
                "image_order" => answers.image_order.is_none(),
                "model_condition" => answers.model_condition.is_none(),
                "camera_relation" => answers.camera_relation.is_none(),
                _ => true,
            })
            .collect();
        if !missing.is_empty() {
            let mut error = ServiceError::new(
                "CONFIRMATION_REQUIRED",
                "Collect the missing answers from the user before creating a task.",
            );
            error.details = Some(
                json!({"preflight_id":self.id,"input_source_identity":self.identity,"questions":missing}),
            );
            return Err(error);
        }
        let detection = &self.analysis.projection_detection;
        let projection = if detection.confirmed_erp {
            VideoProjection::Equirectangular
        } else {
            answers
                .input_projection
                .unwrap_or(VideoProjection::Perspective)
        };
        if (projection == VideoProjection::Equirectangular && !detection.supported_erp)
            || (detection.confirmed_erp
                && answers.input_projection == Some(VideoProjection::Perspective))
        {
            return Err(ServiceError::new(
                "INVALID_PROJECTION",
                "The selected projection is incompatible with this input or source project.",
            ));
        }
        let strategy = if self
            .analysis
            .image_sequence
            .as_ref()
            .is_some_and(|i| i.image_count > 100)
        {
            answers.image_order.unwrap_or_default()
        } else {
            ImageMatchingStrategy::Exhaustive
        };
        let options = self.source.as_ref().map(|source| ReshootOptions {
            condition: answers.model_condition.expect("validated answer"),
            camera_relation: if projection == VideoProjection::Equirectangular {
                None
            } else {
                answers.camera_relation
            },
            allow_fallback: self.policy.auto_retry_enabled,
            source_identity: source.source_identity.clone(),
            input_identity: self.identity.clone(),
            projection,
            image_matching_strategy: strategy,
        });
        Ok((projection, strategy, options))
    }
}
pub(super) async fn inspect(tools: &McpTools, request: InspectRequest) -> Result<Inspection> {
    let path = authorize_input(Path::new(&request.input_path), &tools.settings.input_roots)?;
    crate::pipeline::input_admission::ensure_new_paths_supported(std::slice::from_ref(&path))?;
    if path.is_dir() {
        // A directory authorization does not authorize symlink targets outside it.
        for image in crate::video::list_images(&path)? {
            if !std::fs::canonicalize(image)?.starts_with(&path) {
                return Err(ServiceError::new(
                    "PATH_NOT_AUTHORIZED",
                    "Redirected image escapes the selected input directory",
                ));
            }
        }
    }
    let identity = source_identity(&path)?;
    let policy = request.generation_policy.unwrap_or_default().validate()?;
    let policy = GenerationPolicy {
        version: crate::pipeline::generation_policy::VERSION,
        ..policy
    };
    let (core_id, material, source) = materials::inspect_single(
        tools,
        path.clone(),
        request.quality,
        policy,
        request.source_project_id,
    )
    .await?;
    let mut cache = inspections().lock().await;
    if let Some(old) = cache.iter().find(|s| {
        s.id == core_id
            && s.identity == identity
            && s.quality == request.quality
            && s.policy == policy
            && s.source
                .as_ref()
                .map(|s| (&s.project_id, &s.source_identity))
                == source.as_ref().map(|s| (&s.project_id, &s.source_identity))
    }) {
        return Ok(old.clone());
    }
    let analysis = InputAnalysis {
        input_type: if material.video.is_some() {
            crate::project::ProjectInputType::Video
        } else {
            crate::project::ProjectInputType::Images
        },
        video: material.video,
        image_sequence: material.image_sequence,
        projection_detection: material.projection_detection,
    };
    if source_identity(&path)? != identity {
        return Err(ServiceError::new(
            "INPUT_CHANGED",
            "Input changed; inspect it again.",
        ));
    }
    let result = Inspection {
        id: core_id,
        path,
        identity,
        quality: request.quality,
        policy,
        source,
        analysis,
    };
    if cache.len() >= 64 {
        cache.pop_front();
    }
    cache.push_back(result.clone());
    Ok(result)
}
pub(super) async fn verify_id(id: Option<Uuid>, current: &Inspection) -> Result<()> {
    if id.is_some_and(|id| id != current.id) {
        return Err(ServiceError::new(
            "INPUT_CHANGED",
            "Preflight is stale or expired; inspect and confirm again.",
        ));
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum GpsSummary {
    Pending,
    Checked(bool),
    Unavailable,
}
fn gps_cache() -> &'static std::sync::Mutex<BTreeMap<String, GpsSummary>> {
    static CACHE: OnceLock<std::sync::Mutex<BTreeMap<String, GpsSummary>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn gps_summary(tools: &McpTools, inspection: &Inspection) -> Value {
    if !inspection.path.is_dir() {
        return json!({"detected":null,"status":"unavailable"});
    }
    let mut cache = gps_cache().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(value) = cache.get(&inspection.identity) {
        return match value {
            GpsSummary::Pending => json!({"detected":null,"status":"pending"}),
            GpsSummary::Checked(detected) => json!({"detected":detected,"status":"checked"}),
            GpsSummary::Unavailable => json!({"detected":null,"status":"unavailable"}),
        };
    }
    if cache.len() >= 64 {
        cache.clear();
    }
    cache.insert(inspection.identity.clone(), GpsSummary::Pending);
    let path = inspection.path.clone();
    let identity = inspection.identity.clone();
    let access =
        crate::pipeline::access::InvocationAccess::restricted(tools.settings.input_roots.clone());
    tokio::spawn(async move {
        let expected = identity.clone();
        let observed = tokio::task::spawn_blocking(move || {
            access.authorize_image_directory(&path).ok()?;
            let images = crate::video::list_images(&path).ok()?;
            let detected = crate::video::gps::detect(&images);
            (source_identity(&path).ok().as_deref() == Some(&expected)).then_some(detected)
        })
        .await;
        if let Some(value) = gps_cache()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&identity)
        {
            *value = match observed {
                Ok(Some(detected)) => GpsSummary::Checked(detected),
                _ => GpsSummary::Unavailable,
            };
        }
    });
    json!({"detected":null,"status":"pending"})
}

#[cfg(test)]
#[path = "generation_tests.rs"]
mod tests;
pub(super) async fn public_inspection(tools: &McpTools, inspection: Inspection) -> Result<Value> {
    let plan = crate::commands::plan_input(
        &tools.engines,
        inspection.path.clone(),
        inspection.quality,
        None,
        Some(inspection.analysis.clone()),
        Some(inspection.policy),
        None,
    )
    .await?;
    let mut plan = snake(serde_json::to_value(plan)?);
    if let Some(frame_plan) = plan.get_mut("plan").and_then(Value::as_object_mut) {
        for key in ["selected_frames", "candidate_frames"] {
            if let Some(Value::Array(frames)) = frame_plan.remove(key) {
                frame_plan.insert(
                    format!("{}_count", key.trim_end_matches('s')),
                    json!(frames.len()),
                );
            }
        }
    }
    if let Some(detection) = plan
        .get_mut("projection_detection")
        .and_then(Value::as_object_mut)
    {
        detection.remove("preview_jpeg");
    }
    let mut input = snake(serde_json::to_value(&inspection.analysis)?);
    if let Some(detection) = input
        .get_mut("projection_detection")
        .and_then(Value::as_object_mut)
    {
        detection.remove("preview_jpeg");
    }
    Ok(
        json!({"preflight_id":inspection.id,"input_source_identity":inspection.identity,"input":input,
        "source":inspection.source.as_ref().map(|s|snake(serde_json::to_value(s).unwrap())),"generation_policy":snake(serde_json::to_value(inspection.policy)?),
        "plan":plan,"plan_is_estimate":true,"max_splats":null,"questions":inspection.questions(),"gps":gps_summary(tools,&inspection)}),
    )
}

pub(super) async fn create(tools: &McpTools, args: Value) -> Result<Value> {
    let legacy_order = args
        .pointer("/reshoot_options/imageMatchingStrategy")
        .is_some();
    let request: CreateRequest = parse(args)?;
    if request.reshoot_options.is_some() && request.source_project_id.is_none() {
        return Err(ServiceError::new(
            "INVALID_ARGUMENT",
            "reshoot_options requires source_project_id",
        ));
    }
    if request.client_request_id.is_empty() || request.client_request_id.len() > 256 {
        return Err(ServiceError::new(
            "INVALID_ARGUMENT",
            "client_request_id must contain 1–256 bytes",
        ));
    }
    let policy = request
        .generation_policy
        .unwrap_or_else(|| GenerationPolicy {
            auto_retry_enabled: request
                .reshoot_options
                .as_ref()
                .is_none_or(|o| o.allow_fallback),
            ..Default::default()
        })
        .validate()?;
    let policy = GenerationPolicy {
        version: crate::pipeline::generation_policy::VERSION,
        ..policy
    };
    let mut intent = serde_json::to_value(&request)?;
    intent.as_object_mut().unwrap().remove("preflight_id");
    intent.as_object_mut().unwrap().remove("client_request_id");
    intent["generation_policy"] = serde_json::to_value(policy)?;
    // First honor a lost response, even if input/settings have since changed.
    if let Some(old) = tools
        .tasks
        .all()
        .await?
        .into_iter()
        .find(|t| t.client_request_id.as_ref() == Some(&request.client_request_id))
    {
        if old
            .request_parameters
            .as_ref()
            .and_then(|p| p.get("mcp_intent"))
            == Some(&intent)
        {
            return public_task(old);
        }
        // Compatibility with already-created v1 MCP tasks.
        let mut legacy = json!({"input_path":request.input_path,"quality":request.quality});
        if let Some(policy) = request.generation_policy {
            legacy["generationPolicy"] = serde_json::to_value(policy)?;
        }
        if let Some(options) = &request.reshoot_options {
            legacy["reshootOptions"] = serde_json::to_value(options)?;
            legacy["source_project_id"] = json!(request.source_project_id);
        }
        if old.request_parameters == Some(crate::tasks::generation_parameters(Some(legacy))?)
            && request.preflight_id.is_none()
            && request.input_source_identity.is_none()
            && serde_json::to_value(&request.answers)? == serde_json::to_value(Answers::default())?
        {
            return public_task(old);
        }
        return Err(ServiceError::new(
            "IDEMPOTENCY_CONFLICT",
            "client_request_id already identifies different parameters",
        ));
    }
    let inspection = inspect(
        tools,
        InspectRequest {
            input_path: request.input_path.clone(),
            quality: request.quality,
            generation_policy: Some(policy),
            source_project_id: request.source_project_id,
        },
    )
    .await?;
    verify_id(request.preflight_id, &inspection).await?;
    if request
        .input_source_identity
        .as_ref()
        .is_some_and(|id| id != &inspection.identity)
    {
        return Err(ServiceError::new(
            "INPUT_CHANGED",
            "Input changed; inspect and confirm again.",
        ));
    }
    let mut answers = request.answers;
    if let Some(options) = &request.reshoot_options {
        if options.source_identity != inspection.source.as_ref().unwrap().source_identity
            || options.input_identity != inspection.identity
        {
            return Err(ServiceError::new(
                "INPUT_CHANGED",
                "Reshoot source or input changed; inspect again.",
            ));
        }
        answers.model_condition = answers.model_condition.or(Some(options.condition));
        answers.camera_relation = answers.camera_relation.or(options.camera_relation);
        answers.input_projection = answers.input_projection.or(Some(options.projection));
        if legacy_order {
            answers.image_order = answers
                .image_order
                .or(Some(options.image_matching_strategy));
        }
    }
    let (projection, strategy, options) = inspection.validate_answers(&answers)?;
    if source_identity(&inspection.path)? != inspection.identity {
        return Err(ServiceError::new(
            "INPUT_CHANGED",
            "Input changed during admission",
        ));
    }
    let input_source = if inspection.analysis.input_type == crate::project::ProjectInputType::Video
    {
        InputSource::Video {
            path: inspection.path.clone(),
        }
    } else {
        InputSource::ImageFolder {
            path: inspection.path.clone(),
        }
    };
    let material_identity = input_source.identity()?;
    let inputs = mapped_input(
        &inspection,
        &answers,
        projection,
        strategy,
        material_identity,
    );
    // Shared per-material validation, not the obsolete scalar source/input
    // projection equality check. Mixed sources use their real camera models.
    crate::pipeline::inputs::validate(&tools.engines, &inputs, request.quality, policy).await?;
    if source_identity(&inspection.path)? != inspection.identity {
        return Err(ServiceError::new(
            "INPUT_CHANGED",
            "Input changed during admission",
        ));
    }
    let mut parameters = json!({"input_path":request.input_path,"quality":request.quality,"generationPolicy":policy,
        "videoProjection":projection,"videoSourceIdentity":inspection.identity,"imageMatchingStrategy":strategy,
        "inputs":inputs,"mcp_intent":intent});
    if let Some(options) = options {
        parameters["reshootOptions"] = serde_json::to_value(options)?;
        parameters["source_project_id"] = json!(request.source_project_id);
    }
    let settings = crate::project::catalog::load_settings().await?;
    let task = tools
        .tasks
        .create(
            None,
            inspection.path,
            request.quality,
            settings.projects_root,
            true,
            "mcp",
            Some(request.client_request_id),
            Some(parameters),
        )
        .await?;
    if let Some(id) = request.source_project_id {
        tools.tasks.mark_reshoot(task.task_id, id).await?;
    }
    public_task(tools.tasks.get(task.task_id).await?)
}
