use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, UNIX_EPOCH},
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    pipeline::input_admission::{
        ensure_colmap_import_enabled, is_colmap_input_file, unsupported_input,
        COLMAP_IMPORT_ENABLED,
    },
    presets::Quality,
    project::{
        catalog,
        manager::{atomic_write_json, sanitize_project_name, unique_project_path},
        ExternalFileFingerprint, ImportedColmapProvenance, ImportedGaussianFormat,
        ImportedProjectProvenance, ProjectInputType, ProjectMetadata, ProjectOutput, ProjectStatus,
        PROJECT_APP_ID,
    },
    reconstruction::{colmap_model, ply::inspect_gaussian_ply},
};

const MAX_SCAN_ENTRIES: usize = 20_000;
const MAX_SCAN_DEPTH: usize = 4;
const MAX_CHUNK_BYTES: usize = 1_024_000;
const PROBE_TOKEN_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianImportCandidate {
    id: Uuid,
    name: String,
    format: ImportedGaussianFormat,
    file_size: u64,
    direct_ply: bool,
    splat_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColmapImportCandidate {
    id: Uuid,
    label: String,
    registered_images: u64,
    points_3d: u64,
    camera_count: u64,
    reshoot_eligible: bool,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianImportProbe {
    probe_id: Uuid,
    suggested_name: String,
    models: Vec<GaussianImportCandidate>,
    colmap_models: Vec<ColmapImportCandidate>,
    default_model_id: Option<Uuid>,
    default_colmap_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GaussianImportSelectionKind {
    File,
    Folder,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianImportSelection {
    selection_id: Uuid,
    display_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginGaussianImportRequest {
    probe_id: Uuid,
    model_id: Uuid,
    colmap_id: Option<Uuid>,
    project_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianImportSessionInfo {
    import_id: Uuid,
    source_name: String,
    source_size: u64,
    source_total_size: u64,
    source_format: ImportedGaussianFormat,
    needs_conversion: bool,
    output_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteGaussianImportRequest {
    import_id: Uuid,
    splat_count: u64,
    sh_bands: u8,
    model: Option<String>,
}

#[derive(Debug, Clone)]
struct ModelCandidate {
    id: Uuid,
    path: PathBuf,
    format: ImportedGaussianFormat,
    file_size: u64,
    entry_size: u64,
    direct_ply: bool,
    splat_count: Option<u64>,
    allowed_files: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
struct ColmapCandidate {
    id: Uuid,
    value: ImportedColmapProvenance,
}

#[derive(Clone)]
struct ProbeSession {
    models: Vec<ModelCandidate>,
    colmap: Vec<ColmapCandidate>,
    suggested_name: String,
    created_at: Instant,
}

#[derive(Clone)]
struct SelectionSession {
    path: PathBuf,
    created_at: Instant,
}

#[derive(Clone)]
struct ImportSession {
    id: Uuid,
    model: ModelCandidate,
    colmap: Option<ImportedColmapProvenance>,
    project_name: String,
    project_path: PathBuf,
    staging_path: PathBuf,
    output_temporary: PathBuf,
    output_written: u64,
    cancellation: tokio_util::sync::CancellationToken,
    completing: bool,
}

#[derive(Default)]
pub struct ImportController {
    selections: Mutex<HashMap<Uuid, SelectionSession>>,
    probes: Mutex<HashMap<Uuid, ProbeSession>>,
    admission: AsyncMutex<()>,
    active: AsyncMutex<Option<ImportSession>>,
}

impl ImportController {
    pub async fn admission(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.admission.lock().await
    }

    pub async fn is_active(&self) -> bool {
        self.active.lock().await.is_some()
    }
}

fn import_format(path: &Path) -> Option<ImportedGaussianFormat> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name == "meta.json" {
        return Some(ImportedGaussianFormat::Sog);
    }
    Some(
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "ply" => ImportedGaussianFormat::Ply,
            "sog" => ImportedGaussianFormat::Sog,
            "spz" => ImportedGaussianFormat::Spz,
            "splat" => ImportedGaussianFormat::Splat,
            "ksplat" => ImportedGaussianFormat::Ksplat,
            _ => return None,
        },
    )
}

fn scan_files(path: &Path) -> Result<Vec<PathBuf>> {
    let root = std::fs::canonicalize(path)?;
    if root.is_file() {
        return Ok(vec![root]);
    }
    if !root.is_dir() {
        return Err(SplatError::InvalidPath(root));
    }
    let mut files = Vec::new();
    let mut pending = vec![(root, 0_usize)];
    let mut visited = 0_usize;
    while let Some((directory, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            visited += 1;
            if visited > MAX_SCAN_ENTRIES {
                return Err(SplatError::Process(
                    "导入目录超过 20,000 个条目的安全扫描上限".into(),
                ));
            }
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() && depth < MAX_SCAN_DEPTH {
                pending.push((entry.path(), depth + 1));
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn sog_allowed_files(meta: &Path) -> Result<Vec<PathBuf>> {
    if meta.extension().and_then(|value| value.to_str()) != Some("json") {
        return Ok(vec![meta.to_path_buf()]);
    }
    let root = std::fs::canonicalize(
        meta.parent()
            .ok_or_else(|| SplatError::InvalidPath(meta.into()))?,
    )?;
    let meta = std::fs::canonicalize(meta)?;
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta)?)?;
    let mut references = Vec::new();
    fn visit(value: &serde_json::Value, references: &mut Vec<String>) {
        match value {
            serde_json::Value::String(value) if value.to_ascii_lowercase().ends_with(".webp") => {
                references.push(value.clone());
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    visit(value, references);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values() {
                    visit(value, references);
                }
            }
            _ => {}
        }
    }
    visit(&value, &mut references);
    let mut files = vec![meta];
    for reference in references {
        let relative = Path::new(&reference);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(SplatError::Process(format!(
                "SOG 引用了导入目录外的文件：{reference}"
            )));
        }
        let unresolved = root.join(relative);
        if std::fs::symlink_metadata(&unresolved)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(SplatError::Process(format!(
                "SOG 不允许通过符号链接读取关联文件：{reference}"
            )));
        }
        let candidate = std::fs::canonicalize(&unresolved)
            .map_err(|_| SplatError::Process(format!("SOG 缺少关联文件：{reference}")))?;
        if !candidate.starts_with(&root) || !candidate.is_file() {
            return Err(SplatError::Process(format!(
                "SOG 关联文件路径无效：{reference}"
            )));
        }
        if !files.contains(&candidate) {
            files.push(candidate);
        }
    }
    Ok(files)
}

fn count_binary_records(path: &Path) -> Option<u64> {
    let mut bytes = [0_u8; 8];
    File::open(path).ok()?.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn validate_colmap_database(
    database: &Path,
    camera: &colmap_model::ColmapCamera,
    images: &[colmap_model::RegisteredImage],
) -> Result<()> {
    let result = (|| -> rusqlite::Result<_> {
        let connection = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let cameras = {
            let mut statement = connection
                .prepare("SELECT camera_id, model, width, height, params FROM cameras")?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, u64>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let database_images = {
            let mut statement = connection.prepare("SELECT name, camera_id FROM images")?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
                })?
                .collect::<rusqlite::Result<HashMap<_, _>>>()?;
            rows
        };
        Ok((cameras, database_images))
    })();
    let (cameras, database_images) = result.map_err(|error| {
        SplatError::Process(format!("无法校验导入项目的 COLMAP 数据库：{error}"))
    })?;
    let camera_matches = cameras.first().is_some_and(|stored| {
        let (chunks, remainder) = stored.4.as_chunks::<8>();
        let params = chunks
            .iter()
            .map(|bytes| f64::from_le_bytes(*bytes))
            .collect::<Vec<_>>();
        let params_match = params.len() == camera.params.len()
            && params.iter().zip(&camera.params).all(|(left, right)| {
                let tolerance = 1e-9_f64.max(right.abs() * 1e-9);
                (left - right).abs() <= tolerance
            });
        stored.0 == camera.id
            && stored.1 == camera.model_id
            && stored.2 == camera.width
            && stored.3 == camera.height
            && remainder.is_empty()
            && params_match
    });
    if cameras.len() != 1 || !camera_matches {
        return Err(SplatError::Process(
            "database.db 的相机记录与稀疏模型不一致".into(),
        ));
    }
    if images
        .iter()
        .any(|image| database_images.get(&image.name) != Some(&image.camera_id))
    {
        return Err(SplatError::Process(
            "database.db 的图像名称或相机记录与稀疏模型不一致".into(),
        ));
    }
    Ok(())
}

fn validate_registered_image_files(
    root: &Path,
    camera: &colmap_model::ColmapCamera,
    images: &[colmap_model::RegisteredImage],
) -> Result<()> {
    let canonical_root = std::fs::canonicalize(root)?;
    let expected_width = u32::try_from(camera.width)
        .map_err(|_| SplatError::Process("COLMAP 相机宽度超出图片格式范围".into()))?;
    let expected_height = u32::try_from(camera.height)
        .map_err(|_| SplatError::Process("COLMAP 相机高度超出图片格式范围".into()))?;
    for image in images {
        let path = canonical_root.join(&image.name);
        let path = std::fs::canonicalize(&path).map_err(|_| {
            SplatError::Process(format!("原始图片目录缺少已注册图像：{}", image.name))
        })?;
        if !path.starts_with(&canonical_root) || !path.is_file() {
            return Err(SplatError::Process(format!(
                "已注册图像路径无效：{}",
                image.name
            )));
        }
        let dimensions = image::ImageReader::open(&path)
            .and_then(|reader| reader.with_guessed_format())
            .map_err(|error| {
                SplatError::Process(format!("无法读取已注册图像 {}：{error}", image.name))
            })?
            .into_dimensions()
            .map_err(|error| {
                SplatError::Process(format!("无法读取已注册图像 {}：{error}", image.name))
            })?;
        if dimensions != (expected_width, expected_height) {
            return Err(SplatError::Process(format!(
                "已注册图像 {} 的尺寸与相机记录不一致",
                image.name
            )));
        }
    }
    Ok(())
}

pub(crate) fn revalidate_imported_colmap(
    colmap: &ImportedColmapProvenance,
) -> Result<(
    colmap_model::ColmapCamera,
    Vec<colmap_model::RegisteredImage>,
)> {
    let database = colmap
        .database_path
        .as_ref()
        .filter(|path| path.is_file())
        .ok_or_else(|| SplatError::Process("外部 COLMAP 项目缺少 database.db".into()))?;
    let images_root = colmap
        .images_path
        .as_ref()
        .filter(|path| path.is_dir())
        .ok_or_else(|| SplatError::Process("外部 COLMAP 项目缺少原始图片".into()))?;
    if !colmap.sparse_model_path.is_dir() {
        return Err(SplatError::Process("外部 COLMAP 项目缺少稀疏模型".into()));
    }
    let camera = colmap_model::read_single_camera(&colmap.sparse_model_path)?;
    if camera.model_id == 17 {
        return Err(SplatError::Process("全景项目暂不支持高清补拍。".into()));
    }
    let images = colmap_model::read_registered_images(&colmap.sparse_model_path)?;
    if images.is_empty() {
        return Err(SplatError::Process("稀疏模型没有已注册图像".into()));
    }
    if images.iter().any(|image| image.camera_id != camera.id) {
        return Err(SplatError::Process(
            "外部项目包含多个相机，不能进行同相机增量补拍".into(),
        ));
    }
    validate_colmap_database(database, &camera, &images)?;
    validate_registered_image_files(images_root, &camera, &images)?;
    Ok((camera, images))
}

fn inspect_colmap(model: &Path, all_files: &[PathBuf]) -> ImportedColmapProvenance {
    let root = model.parent().and_then(Path::parent).unwrap_or(model);
    let database = all_files.iter().find(|path| {
        path.file_name().and_then(|value| value.to_str()) == Some("database.db")
            && path.starts_with(root)
    });
    let images = [
        root.join("images"),
        model.parent().unwrap_or(root).join("images"),
    ]
    .into_iter()
    .find(|path| path.is_dir());
    let binary = model.join("cameras.bin").is_file() && model.join("images.bin").is_file();
    let parsed_images = colmap_model::read_registered_images(model).unwrap_or_default();
    let registered_images = parsed_images.len() as u64;
    let points_3d = if binary {
        count_binary_records(&model.join("points3D.bin")).unwrap_or(0)
    } else {
        std::fs::read_to_string(model.join("points3D.txt"))
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
                    .count() as u64
            })
            .unwrap_or(0)
    };
    let (camera_count, single_camera_ok) = if binary {
        match colmap_model::read_single_camera(model) {
            Ok(_) => (1, true),
            Err(_) => (
                count_binary_records(&model.join("cameras.bin")).unwrap_or(0),
                false,
            ),
        }
    } else {
        let count = std::fs::read_to_string(model.join("cameras.txt"))
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
                    .count() as u64
            })
            .unwrap_or(0);
        (count, count == 1)
    };
    let parsed_camera = colmap_model::read_single_camera(model).ok();
    let reason = if parsed_camera
        .as_ref()
        .is_some_and(|camera| camera.model_id == 17)
    {
        Some("全景项目暂不支持高清补拍。".into())
    } else if database.is_none() {
        Some("缺少 database.db".into())
    } else if images.is_none() {
        Some("缺少原始 images 目录".into())
    } else if !single_camera_ok {
        Some("高清补拍仅支持单一共享相机模型".into())
    } else if registered_images == 0 {
        Some("稀疏模型没有已注册图像".into())
    } else if let (Some(database), Some(camera), Some(images_root)) =
        (database, parsed_camera.as_ref(), images.as_ref())
    {
        validate_colmap_database(database, camera, &parsed_images)
            .and_then(|_| validate_registered_image_files(images_root, camera, &parsed_images))
            .err()
            .map(|error| error.to_string())
    } else {
        None
    };
    ImportedColmapProvenance {
        root_path: root.to_path_buf(),
        sparse_model_path: model.to_path_buf(),
        database_path: database.cloned(),
        images_path: images,
        registered_images,
        points_3d,
        camera_count,
        reshoot_eligible: reason.is_none(),
        reshoot_reason: reason,
    }
}

fn detect_colmap(files: &[PathBuf]) -> Vec<ColmapCandidate> {
    let mut result = Vec::new();
    for path in files {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name != "cameras.bin" && name != "cameras.txt" {
            continue;
        }
        let Some(model) = path.parent() else { continue };
        let valid = if name.ends_with(".bin") {
            model.join("images.bin").is_file() && model.join("points3D.bin").is_file()
        } else {
            model.join("images.txt").is_file() && model.join("points3D.txt").is_file()
        };
        if valid
            && !result
                .iter()
                .any(|item: &ColmapCandidate| item.value.sparse_model_path == model)
        {
            result.push(ColmapCandidate {
                id: Uuid::new_v4(),
                value: inspect_colmap(model, files),
            });
        }
    }
    result.sort_by_key(|value| {
        (
            std::cmp::Reverse(value.value.registered_images),
            std::cmp::Reverse(value.value.points_3d),
        )
    });
    result
}

fn new_import_colmap(source: &Path, files: &[PathBuf]) -> Result<Vec<ColmapCandidate>> {
    if !COLMAP_IMPORT_ENABLED {
        return Ok(Vec::new());
    }
    let context = if source.is_file() {
        source
            .parent()
            .map(scan_files)
            .transpose()?
            .unwrap_or_else(|| files.to_vec())
    } else {
        files.to_vec()
    };
    Ok(detect_colmap(&context))
}

fn unsupported_gaussian_source(files: &[PathBuf]) -> SplatError {
    if files.iter().any(|path| is_colmap_input_file(path)) {
        return ensure_colmap_import_enabled()
            .err()
            .unwrap_or_else(unsupported_input);
    }
    unsupported_input()
}

fn validate_new_import_colmap(colmap_id: Option<Uuid>) -> Result<()> {
    if colmap_id.is_some() {
        ensure_colmap_import_enabled()?;
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; MAX_CHUNK_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn fingerprint(path: &Path) -> Result<ExternalFileFingerprint> {
    let metadata = path.metadata()?;
    Ok(ExternalFileFingerprint {
        sha256: sha256_file(path)?,
        file_size: metadata.len(),
        modified_ms: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_millis() as u64),
    })
}

fn fingerprint_files(paths: &[PathBuf]) -> Result<ExternalFileFingerprint> {
    if paths.len() == 1 {
        return fingerprint(&paths[0]);
    }
    let mut ordered = paths.to_vec();
    ordered.sort();
    let mut hash = Sha256::new();
    let mut file_size = 0_u64;
    let mut modified_ms = None;
    let mut buffer = vec![0_u8; MAX_CHUNK_BYTES];
    for path in ordered {
        let metadata = path.metadata()?;
        file_size = file_size.saturating_add(metadata.len());
        modified_ms = modified_ms.max(
            metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis() as u64),
        );
        hash.update(
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .as_bytes(),
        );
        let mut file = File::open(path)?;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
    }
    Ok(ExternalFileFingerprint {
        sha256: format!("{:x}", hash.finalize()),
        file_size,
        modified_ms,
    })
}

#[tauri::command]
pub async fn select_gaussian_import_source(
    app: tauri::AppHandle,
    state: tauri::State<'_, ImportController>,
    kind: GaussianImportSelectionKind,
) -> Result<Option<GaussianImportSelection>> {
    let selected = tokio::task::spawn_blocking(move || match kind {
        GaussianImportSelectionKind::File => app
            .dialog()
            .file()
            .add_filter(
                "Gaussian Splat",
                &["ply", "sog", "spz", "splat", "ksplat", "json"],
            )
            .blocking_pick_file(),
        GaussianImportSelectionKind::Folder => app.dialog().file().blocking_pick_folder(),
    })
    .await
    .map_err(|error| SplatError::Process(format!("打开 Gaussian 导入选择器失败：{error}")))?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|error| SplatError::Process(format!("无法读取所选导入路径：{error}")))?;
    let display_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Gaussian")
        .to_string();
    let selection_id = Uuid::new_v4();
    let mut selections = state
        .selections
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    selections.retain(|_, selection| selection.created_at.elapsed() <= PROBE_TOKEN_TTL);
    selections.insert(
        selection_id,
        SelectionSession {
            path,
            created_at: Instant::now(),
        },
    );
    Ok(Some(GaussianImportSelection {
        selection_id,
        display_name,
    }))
}

#[tauri::command]
pub async fn probe_gaussian_import(
    state: tauri::State<'_, ImportController>,
    selection_id: Uuid,
) -> Result<GaussianImportProbe> {
    let source = {
        let mut selections = state
            .selections
            .lock()
            .unwrap_or_else(|value| value.into_inner());
        selections.retain(|_, selection| selection.created_at.elapsed() <= PROBE_TOKEN_TTL);
        selections
            .get(&selection_id)
            .map(|selection| selection.path.clone())
    }
    .ok_or_else(|| SplatError::Process("导入选择已失效，请重新选择模型".into()))?;
    let (files, models, colmap, suggested_name) =
        tokio::task::spawn_blocking(move || -> Result<_> {
            let canonical_source = std::fs::canonicalize(&source)?;
            let files = scan_files(&canonical_source)?;
            let mut models = Vec::new();
            for path in &files {
                let Some(mut format) = import_format(path) else {
                    continue;
                };
                let metadata = path.metadata()?;
                let ply = matches!(format, ImportedGaussianFormat::Ply);
                let ply_info = ply.then(|| inspect_gaussian_ply(path).ok()).flatten();
                let direct_ply = ply_info.is_some();
                if ply && !direct_ply {
                    format = ImportedGaussianFormat::CompressedPly;
                }
                let allowed_files = if path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("meta.json"))
                {
                    sog_allowed_files(path)?
                } else {
                    vec![path.clone()]
                };
                let file_size = allowed_files.iter().try_fold(0_u64, |total, file| {
                    Ok::<_, std::io::Error>(total.saturating_add(file.metadata()?.len()))
                })?;
                models.push(ModelCandidate {
                    id: Uuid::new_v4(),
                    path: path.clone(),
                    format,
                    file_size,
                    entry_size: metadata.len(),
                    direct_ply,
                    splat_count: ply_info.map(|info| info.splat_count),
                    allowed_files,
                });
            }
            if models.is_empty() {
                return Err(unsupported_gaussian_source(&files));
            }
            let colmap = new_import_colmap(&canonical_source, &files)?;
            let suggested_name = canonical_source
                .file_stem()
                .or_else(|| canonical_source.file_name())
                .and_then(|value| value.to_str())
                .unwrap_or("imported-gaussian")
                .to_string();
            Ok((files, models, colmap, suggested_name))
        })
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
    drop(files);
    let probe_id = Uuid::new_v4();
    let default_model_id = (models.len() == 1).then_some(models[0].id);
    let default_colmap_id = colmap.first().map(|value| value.id);
    let response = GaussianImportProbe {
        probe_id,
        suggested_name: suggested_name.clone(),
        models: models
            .iter()
            .map(|model| GaussianImportCandidate {
                id: model.id,
                name: model
                    .path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("model")
                    .into(),
                format: model.format,
                file_size: model.file_size,
                direct_ply: model.direct_ply,
                splat_count: model.splat_count,
            })
            .collect(),
        colmap_models: colmap
            .iter()
            .map(|item| ColmapImportCandidate {
                id: item.id,
                label: item.value.sparse_model_path.display().to_string(),
                registered_images: item.value.registered_images,
                points_3d: item.value.points_3d,
                camera_count: item.value.camera_count,
                reshoot_eligible: item.value.reshoot_eligible,
                reason: item.value.reshoot_reason.clone(),
            })
            .collect(),
        default_model_id,
        default_colmap_id,
    };
    let mut probes = state
        .probes
        .lock()
        .unwrap_or_else(|value| value.into_inner());
    probes.retain(|_, probe| probe.created_at.elapsed() <= PROBE_TOKEN_TTL);
    probes.insert(
        probe_id,
        ProbeSession {
            models,
            colmap,
            suggested_name,
            created_at: Instant::now(),
        },
    );
    Ok(response)
}

#[tauri::command]
pub async fn begin_gaussian_import(
    imports: tauri::State<'_, ImportController>,
    tasks: tauri::State<'_, crate::commands::PipelineController>,
    request: BeginGaussianImportRequest,
) -> Result<GaussianImportSessionInfo> {
    validate_new_import_colmap(request.colmap_id)?;
    let _admission = imports.admission().await;
    if tasks
        .running()
        .await
        .map_err(|error| SplatError::Process(error.to_string()))?
        .is_some()
    {
        return Err(SplatError::Process(
            "已有生成任务正在运行，请等待完成后再导入".into(),
        ));
    }
    let mut active = imports.active.lock().await;
    if active.is_some() {
        return Err(SplatError::Process("已有 Gaussian 导入正在进行".into()));
    }
    let probe = {
        let mut probes = imports
            .probes
            .lock()
            .unwrap_or_else(|value| value.into_inner());
        probes.retain(|_, probe| probe.created_at.elapsed() <= PROBE_TOKEN_TTL);
        probes.get(&request.probe_id).cloned()
    }
    .ok_or_else(|| SplatError::Process("导入分析已失效，请重新选择模型".into()))?;
    let model = probe
        .models
        .into_iter()
        .find(|value| value.id == request.model_id)
        .ok_or_else(|| SplatError::Process("导入模型令牌无效".into()))?;
    let colmap = request
        .colmap_id
        .and_then(|id| probe.colmap.into_iter().find(|value| value.id == id))
        .map(|value| value.value);
    let projects_root = catalog::load_settings().await?.projects_root;
    crate::project::ProjectManager::validate_root(&projects_root).await?;
    let project_name = sanitize_project_name(if request.project_name.trim().is_empty() {
        &probe.suggested_name
    } else {
        &request.project_name
    });
    let final_path = unique_project_path(&projects_root, &project_name);
    let staging_path = projects_root.join(format!(".import-{}.tmp", Uuid::new_v4()));
    tokio::fs::create_dir_all(staging_path.join("cache")).await?;
    let output_temporary = staging_path.join("cache").join("model.ply.tmp");
    let import_id = Uuid::new_v4();
    let response = GaussianImportSessionInfo {
        import_id,
        source_name: model
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("model")
            .into(),
        source_size: model.entry_size,
        source_total_size: model.file_size,
        source_format: model.format,
        needs_conversion: !model.direct_ply,
        output_name: "model.ply".into(),
    };
    *active = Some(ImportSession {
        id: import_id,
        model,
        colmap,
        project_name,
        project_path: final_path,
        staging_path,
        output_temporary,
        output_written: 0,
        cancellation: tokio_util::sync::CancellationToken::new(),
        completing: false,
    });
    Ok(response)
}

fn header_uuid(request: &Request<'_>, name: &str) -> Result<Uuid> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| SplatError::Process("Gaussian 导入令牌无效".into()))
}

#[tauri::command]
pub async fn read_gaussian_import_chunk(
    state: tauri::State<'_, ImportController>,
    request: Request<'_>,
) -> Result<Response> {
    let id = header_uuid(&request, "x-ooosplat-import-id")?;
    let filename = request
        .headers()
        .get("x-ooosplat-import-file")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let offset = request
        .headers()
        .get("x-ooosplat-offset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let length = request
        .headers()
        .get("x-ooosplat-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(MAX_CHUNK_BYTES)
        .min(MAX_CHUNK_BYTES);
    let active = state.active.lock().await;
    let session = active
        .as_ref()
        .filter(|value| value.id == id)
        .ok_or_else(|| SplatError::Process("Gaussian 导入会话不存在或已结束".into()))?;
    if session.cancellation.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    let path = if filename.is_empty() {
        session.model.path.clone()
    } else {
        let normalized = filename.replace('\\', "/");
        let source_root = session.model.path.parent();
        session
            .model
            .allowed_files
            .iter()
            .find(|path| {
                source_root
                    .and_then(|root| path.strip_prefix(root).ok())
                    .map(|relative| relative.to_string_lossy().replace('\\', "/") == normalized)
                    .unwrap_or(false)
                    || (!normalized.contains('/')
                        && path.file_name().and_then(|value| value.to_str()) == Some(filename))
            })
            .cloned()
            .ok_or_else(|| SplatError::Process("导入文件不在已授权清单中".into()))?
    };
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; length];
        let read = file.read(&mut bytes)?;
        bytes.truncate(read);
        Ok(bytes)
    })
    .await
    .map_err(|error| SplatError::Process(error.to_string()))??;
    Ok(Response::new(bytes))
}

#[tauri::command]
pub async fn write_gaussian_import_chunk(
    state: tauri::State<'_, ImportController>,
    request: Request<'_>,
) -> Result<u64> {
    let id = header_uuid(&request, "x-ooosplat-import-id")?;
    let bytes = match request.body() {
        InvokeBody::Raw(bytes) if bytes.len() <= MAX_CHUNK_BYTES => bytes.as_slice(),
        _ => {
            return Err(SplatError::Process(
                "导入写入必须使用不超过 1 MiB 的原始二进制分块".into(),
            ))
        }
    };
    let mut active = state.active.lock().await;
    let session = active
        .as_mut()
        .filter(|value| value.id == id)
        .ok_or_else(|| SplatError::Process("Gaussian 导入会话不存在或已结束".into()))?;
    if session.cancellation.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    let offset = request
        .headers()
        .get("x-ooosplat-offset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| SplatError::Process("导入写入偏移无效".into()))?;
    if offset != session.output_written {
        return Err(SplatError::Process("导入转换分块顺序无效".into()));
    }
    let path = session.output_temporary.clone();
    let data = bytes.to_vec();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        let mut file = options.open(path)?;
        file.write_all(&data)?;
        Ok(())
    })
    .await
    .map_err(|error| SplatError::Process(error.to_string()))??;
    session.output_written += bytes.len() as u64;
    Ok(session.output_written)
}

#[tauri::command]
pub async fn complete_gaussian_import(
    state: tauri::State<'_, ImportController>,
    request: CompleteGaussianImportRequest,
) -> Result<catalog::ProjectSummary> {
    let session = {
        let mut active = state.active.lock().await;
        let session = active
            .as_mut()
            .filter(|value| value.id == request.import_id && !value.completing)
            .ok_or_else(|| SplatError::Process("Gaussian 导入会话不存在或已结束".into()))?;
        session.completing = true;
        session.clone()
    };
    let result = async {
        if session.cancellation.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        if request
            .model
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("2dgs"))
        {
            return Err(SplatError::Process(
                "暂不支持 2D Gaussian Splatting 模型".into(),
            ));
        }
        if request.sh_bands > 3 {
            return Err(SplatError::Process(
                "Gaussian 模型包含暂不支持的 SH 阶数".into(),
            ));
        }
        let canonical_relative = if session.model.direct_ply {
            None
        } else {
            let info = inspect_gaussian_ply(&session.output_temporary)?;
            if request.splat_count > 0 && info.splat_count != request.splat_count {
                return Err(SplatError::Process(
                    "转换后的 Gaussian 数量与源模型不一致".into(),
                ));
            }
            let published = session.staging_path.join("cache").join("model.ply");
            tokio::fs::rename(&session.output_temporary, &published).await?;
            Some(PathBuf::from("cache").join("model.ply"))
        };
        if session.cancellation.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let source_files = session.model.allowed_files.clone();
        let source_format = session.model.format;
        let fingerprint = tokio::task::spawn_blocking(move || fingerprint_files(&source_files))
            .await
            .map_err(|error| SplatError::Process(error.to_string()))??;
        if session.cancellation.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let validation_model_path = canonical_relative
            .as_ref()
            .map(|value| session.staging_path.join(value))
            .unwrap_or_else(|| session.model.path.clone());
        let info = inspect_gaussian_ply(&validation_model_path)?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let published_model_path = canonical_relative
            .as_ref()
            .map(|value| session.project_path.join(value))
            .unwrap_or_else(|| session.model.path.clone());
        let metadata = ProjectMetadata {
            schema_version: crate::project::metadata::schema_version(),
            app_id: PROJECT_APP_ID.into(),
            id,
            workspace_task_id: None,
            name: session.project_name.clone(),
            created_at: now,
            started_at: Some(now),
            completed_at: Some(now),
            duration_ms: Some(0),
            status: ProjectStatus::Completed,
            source_path: session.model.path.clone(),
            input_type: ProjectInputType::Images,
            quality: Quality::Balanced,
            project_path: session.project_path.clone(),
            output_path: None,
            output: Some(ProjectOutput {
                final_ply: published_model_path,
                file_size: info.file_size,
                splat_count: info.splat_count,
                input_images: 0,
                registered_images: session
                    .colmap
                    .as_ref()
                    .map_or(0, |value| value.registered_images),
                registered_ratio: 0.0,
                points_3d: session.colmap.as_ref().map_or(0, |value| value.points_3d),
            }),
            failure_message: None,
            model: "final.ply".into(),
            transform: Default::default(),
            editing: Default::default(),
            reshoot: None,
            imported: Some(ImportedProjectProvenance {
                source_model_path: session.model.path.clone(),
                source_format,
                source_fingerprint: fingerprint,
                canonical_model_path: canonical_relative,
                colmap: session.colmap.clone(),
            }),
        };
        atomic_write_json(&session.staging_path.join("project.json"), &metadata).await?;
        if session.cancellation.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        tokio::fs::rename(&session.staging_path, &session.project_path).await?;
        if session.cancellation.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let summary = catalog::summarize_project_metadata(&session.project_path, metadata).await?;
        if session.cancellation.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        catalog::register_project(id, &session.project_path).await?;
        Ok(summary)
    }
    .await;
    let mut active = state.active.lock().await;
    if active
        .as_ref()
        .is_some_and(|value| value.id == request.import_id)
    {
        active.take();
    }
    drop(active);
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&session.staging_path).await;
        let _ = tokio::fs::remove_dir_all(&session.project_path).await;
    }
    result
}

#[tauri::command]
pub async fn cancel_gaussian_import(
    state: tauri::State<'_, ImportController>,
    import_id: Uuid,
) -> Result<()> {
    let mut active = state.active.lock().await;
    if active.as_ref().is_some_and(|value| value.id == import_id) {
        let session = active.as_ref().expect("import session checked above");
        session.cancellation.cancel();
        if !session.completing {
            let session = active.take().expect("import session checked above");
            let _ = tokio::fs::remove_dir_all(session.staging_path).await;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn relink_imported_project(
    state: tauri::State<'_, ImportController>,
    project_id: Uuid,
    selection_id: Uuid,
) -> Result<catalog::ProjectSummary> {
    let (root, mut metadata) = catalog::load_registered_project(project_id).await?;
    let imported = metadata
        .imported
        .as_mut()
        .ok_or_else(|| SplatError::Process("该项目不是外部导入项目".into()))?;
    let candidate = {
        let mut selections = state
            .selections
            .lock()
            .unwrap_or_else(|value| value.into_inner());
        selections.retain(|_, selection| selection.created_at.elapsed() <= PROBE_TOKEN_TTL);
        selections
            .get(&selection_id)
            .map(|selection| selection.path.clone())
    }
    .ok_or_else(|| SplatError::Process("重新定位选择已失效，请重新选择文件".into()))?;
    let candidate = std::fs::canonicalize(candidate)?;
    let candidates = if candidate
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("meta.json"))
    {
        sog_allowed_files(&candidate)?
    } else {
        vec![candidate.clone()]
    };
    let fingerprint = tokio::task::spawn_blocking(move || fingerprint_files(&candidates))
        .await
        .map_err(|error| SplatError::Process(error.to_string()))??;
    if fingerprint.sha256 != imported.source_fingerprint.sha256
        || fingerprint.file_size != imported.source_fingerprint.file_size
    {
        return Err(SplatError::Process(
            "所选文件与原导入模型的指纹不一致".into(),
        ));
    }
    imported.source_model_path = candidate.clone();
    imported.source_fingerprint.modified_ms = fingerprint.modified_ms;
    metadata.source_path = candidate;
    metadata.schema_version = crate::project::metadata::schema_version();
    atomic_write_json(&root.join("project.json"), &metadata).await?;
    let (_, _, summary) = catalog::load_registered_project_summary(project_id).await?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::{
        fingerprint_files, import_format, new_import_colmap, scan_files, sog_allowed_files,
        unsupported_gaussian_source, validate_colmap_database, validate_new_import_colmap,
        ImportedGaussianFormat,
    };
    use crate::error::{InputAdmissionReason, SplatError};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn new_gaussian_import_ignores_colmap_sidecars() {
        let root = tempdir().unwrap();
        let model = root.path().join("model.ply");
        fs::write(&model, b"Gaussian import fixture").unwrap();
        for name in ["cameras.bin", "images.bin", "points3D.bin", "database.db"] {
            fs::write(root.path().join(name), b"read-only sidecar").unwrap();
        }
        let files = scan_files(root.path()).unwrap();
        assert_eq!(
            files
                .iter()
                .filter(|path| import_format(path).is_some())
                .count(),
            1
        );
        assert!(new_import_colmap(&model, std::slice::from_ref(&model))
            .unwrap()
            .is_empty());
        assert!(new_import_colmap(root.path(), &files).unwrap().is_empty());
        for name in ["cameras.bin", "images.bin", "points3D.bin", "database.db"] {
            assert_eq!(
                fs::read(root.path().join(name)).unwrap(),
                b"read-only sidecar"
            );
        }
    }

    #[test]
    fn disabled_colmap_import_does_not_read_a_gaussian_files_parent() {
        let root = tempdir().unwrap();
        let source = root.path().join("unread-parent").join("model.ply");
        assert!(new_import_colmap(&source, std::slice::from_ref(&source))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn pure_colmap_and_unsupported_gaussian_sources_use_typed_rejections() {
        assert!(matches!(
            unsupported_gaussian_source(&["cameras.bin".into(), "images.bin".into()]),
            SplatError::InputAdmission(InputAdmissionReason::ColmapDisabled)
        ));
        assert!(matches!(
            unsupported_gaussian_source(&["unsupported.lcc".into()]),
            SplatError::InputAdmission(InputAdmissionReason::UnsupportedInput)
        ));
        assert!(matches!(
            unsupported_gaussian_source(&[]),
            SplatError::InputAdmission(InputAdmissionReason::UnsupportedInput)
        ));
    }

    #[test]
    fn explicit_new_colmap_association_is_rejected_before_import_setup() {
        assert!(validate_new_import_colmap(None).is_ok());
        assert!(matches!(
            validate_new_import_colmap(Some(uuid::Uuid::new_v4())),
            Err(SplatError::InputAdmission(
                InputAdmissionReason::ColmapDisabled
            ))
        ));
    }

    #[test]
    fn imported_database_camera_rejects_malformed_parameter_blobs() {
        let root = tempdir().unwrap();
        let database = root.path().join("database.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE cameras (camera_id INTEGER PRIMARY KEY, model INTEGER, width INTEGER, height INTEGER, params BLOB);
                 CREATE TABLE images (name TEXT, camera_id INTEGER);",
            )
            .unwrap();
        let camera = crate::reconstruction::colmap_model::ColmapCamera {
            id: 1,
            model_id: 2,
            model: "SIMPLE_RADIAL".into(),
            width: 640,
            height: 480,
            params: vec![600.5, 320.25, 240.75, 0.125],
        };
        let valid: Vec<u8> = camera
            .params
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        connection
            .execute("INSERT INTO cameras VALUES (1, 2, 640, 480, ?1)", [&valid])
            .unwrap();
        validate_colmap_database(&database, &camera, &[]).unwrap();

        let mut trailing_byte = valid.clone();
        trailing_byte.push(0);
        for malformed in [
            trailing_byte,
            valid[..valid.len() - 1].to_vec(),
            valid[..valid.len() - 8].to_vec(),
            vec![0_u8; valid.len()],
            Vec::new(),
        ] {
            connection
                .execute("UPDATE cameras SET params = ?1", [&malformed])
                .unwrap();
            assert!(validate_colmap_database(&database, &camera, &[]).is_err());
        }
    }

    #[test]
    fn recognizes_supported_gaussian_formats_only() {
        let cases = [
            ("model.ply", ImportedGaussianFormat::Ply),
            ("model.SOG", ImportedGaussianFormat::Sog),
            ("model.spz", ImportedGaussianFormat::Spz),
            ("model.splat", ImportedGaussianFormat::Splat),
            ("model.ksplat", ImportedGaussianFormat::Ksplat),
            ("meta.json", ImportedGaussianFormat::Sog),
        ];
        for (name, expected) in cases {
            assert_eq!(import_format(name.as_ref()), Some(expected));
        }
        for name in ["model.lcc", "model.lcc2", "points.xyz", "scene.json"] {
            assert_eq!(import_format(name.as_ref()), None);
        }
    }

    #[test]
    fn folder_scan_stops_after_four_levels() {
        let root = tempdir().unwrap();
        let visible = root.path().join("a").join("b").join("c").join("d");
        let hidden = visible.join("e");
        fs::create_dir_all(&hidden).unwrap();
        fs::write(root.path().join("root.splat"), b"root").unwrap();
        fs::write(visible.join("visible.spz"), b"visible").unwrap();
        fs::write(hidden.join("hidden.ksplat"), b"hidden").unwrap();

        let files = scan_files(root.path()).unwrap();
        assert!(files.iter().any(|path| path.ends_with("root.splat")));
        assert!(files.iter().any(|path| path.ends_with("visible.spz")));
        assert!(!files.iter().any(|path| path.ends_with("hidden.ksplat")));
    }

    #[test]
    fn multi_file_fingerprint_is_stable_and_content_sensitive() {
        let root = tempdir().unwrap();
        let first = root.path().join("meta.json");
        let second = root.path().join("means.webp");
        fs::write(&first, b"metadata").unwrap();
        fs::write(&second, b"pixels").unwrap();
        let original = fingerprint_files(&[first.clone(), second.clone()]).unwrap();
        let reversed = fingerprint_files(&[second.clone(), first]).unwrap();
        assert_eq!(original.sha256, reversed.sha256);
        assert_eq!(original.file_size, reversed.file_size);

        fs::write(&second, b"changed").unwrap();
        let changed = fingerprint_files(&[second]).unwrap();
        assert_ne!(original.sha256, changed.sha256);
    }

    #[test]
    fn unpacked_sog_allows_nested_files_but_rejects_parent_traversal() {
        let root = tempdir().unwrap();
        let textures = root.path().join("textures");
        fs::create_dir(&textures).unwrap();
        fs::write(textures.join("means.webp"), b"webp").unwrap();
        let meta = root.path().join("meta.json");
        fs::write(&meta, r#"{"means":"textures/means.webp"}"#).unwrap();

        let allowed = sog_allowed_files(&meta).unwrap();
        assert_eq!(allowed.len(), 2);
        assert!(allowed.iter().any(|path| path.ends_with("means.webp")));

        fs::write(&meta, r#"{"means":"../outside.webp"}"#).unwrap();
        let error = sog_allowed_files(&meta).unwrap_err().to_string();
        assert!(error.contains("导入目录外"));
    }
}
