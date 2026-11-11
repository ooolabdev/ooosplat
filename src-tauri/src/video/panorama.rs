//! Full, monoscopic ERP only. SfM stays spherical; pinhole images are derived
//! *after* registration. COLMAP axes are x right, y down, z forward throughout.
use crate::{
    error::{Result, SplatError},
    presets::Quality,
    process::{ProcessManager, ProcessSpec, ProcessUpdate},
    video::{FramePlan, PlannedFrame, VideoInfo},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const CONVERSION_VERSION: u32 = 1;
pub const FOV_DEGREES: f64 = 92.0;
pub const MAX_DECODE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "camelCase")]
pub enum VideoProjection {
    #[default]
    Auto,
    Perspective,
    Equirectangular,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionDetection {
    #[serde(default)]
    pub source_identity: String,
    #[serde(default)]
    pub unsupported: bool,
    pub confirmed_erp: bool,
    pub requires_choice: bool,
    pub supported_erp: bool,
    pub reason: String,
    pub preview_jpeg: Vec<u8>,
}

fn process_error(detail: impl Into<String>) -> SplatError {
    SplatError::Process(detail.into())
}

pub fn detect_metadata(
    value: &serde_json::Value,
    video: &VideoInfo,
    mp4: bool,
) -> ProjectionDetection {
    let stream = &value["streams"][0];
    let side = stream["side_data_list"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let tags = stream["tags"].to_string().to_lowercase().replace('_', "-");
    let spherical = side.iter().find(|v| {
        v["side_data_type"]
            .as_str()
            .is_some_and(|s| s.contains("Spherical"))
    });
    let projection = spherical
        .and_then(|v| v["projection"].as_str())
        .unwrap_or("");
    let stereo = side.iter().any(|v| {
        v["side_data_type"]
            .as_str()
            .is_some_and(|s| s.contains("Stereo"))
            && !matches!(v["type"].as_str(), Some("2D") | Some("mono"))
    }) || tags.contains("top-bottom")
        || tags.contains("left-right")
        || tags.contains("vr180")
        || tags.contains("dual-fisheye")
        || tags.contains("side by side");
    let cropped = spherical.is_some_and(|v| {
        ["bound_left", "bound_right", "bound_top", "bound_bottom"]
            .iter()
            .any(|k| v[*k].as_f64().is_some_and(|n| n != 0.0))
    });
    // Spherical yaw/pitch/roll describe a global viewing orientation, not an
    // incomplete projection. SfM has an arbitrary world gauge; local rays are
    // defined by the decoded pixels. Do not reject otherwise-standard brands.
    // FFmpeg's 180-degree display rotation remains a full ERP; quarter turns do not.
    let rotated = video.rotation.rem_euclid(180) != 0;
    let ratio = video.width > 0 && video.width == video.height.saturating_mul(2);
    // A phone's display matrix is also normal perspective-video metadata.
    // Quarter-turn restrictions apply only to spherical metadata or a 2:1
    // MP4 candidate that could otherwise be selected as full ERP.
    let rotated_panorama = rotated && (spherical.is_some() || (mp4 && ratio));
    let unsupported = stereo
        || cropped
        || rotated_panorama
        || (!projection.is_empty() && projection != "equirectangular");
    let supported_erp = mp4 && ratio && video.width >= 64 && !unsupported;
    let confirmed_erp = supported_erp && projection == "equirectangular";
    ProjectionDetection {
        source_identity: String::new(), unsupported,
        confirmed_erp, supported_erp,
        requires_choice: !confirmed_erp && ratio && mp4,
        reason: if unsupported { "Unsupported stereo, cropped, rotated or non-ERP projection; do not select full panorama." }
            else if confirmed_erp { "Full ERP metadata detected. Confirm the preview is stitched and monoscopic." }
            else if ratio { "2:1 is only a clue. Inspect the preview and choose ordinary video or stitched monoscopic full 360×180 ERP." }
            else { "Full panorama requires a 2:1 MP4. Ordinary video remains available." }.into(),
        preview_jpeg: Vec::new(),
    }
}

/// Local input identity; no full-video read and never sent to telemetry.
pub fn source_identity(input: &Path) -> Result<String> {
    if input.is_dir() {
        return super::image_projection::source_identity(input);
    }
    let path = fs::canonicalize(input)?;
    let metadata = fs::metadata(&path)?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| process_error(e.to_string()))?
        .as_nanos();
    let mut hash = Sha256::new();
    hash.update(path.to_string_lossy().as_bytes());
    hash.update(metadata.len().to_le_bytes());
    hash.update(modified.to_le_bytes());
    Ok(format!("{:x}", hash.finalize()))
}

pub async fn detect(
    ffprobe: &Path,
    ffmpeg: &Path,
    input: &Path,
    video: &VideoInfo,
    manager: &ProcessManager,
    preview: bool,
) -> Result<ProjectionDetection> {
    let identity = source_identity(input)?;
    let result = manager
        .clone()
        .with_output_capture_limit(64 * 1024)
        .run(ProcessSpec {
            executable: ffprobe.into(),
            args: vec![
                "-v".into(),
                "error".into(),
                "-select_streams".into(),
                "v:0".into(),
                "-show_streams".into(),
                "-of".into(),
                "json".into(),
                input.into(),
            ],
            working_directory: None,
            log_path: None,
            observer: None,
        })
        .await?;
    if !result.success {
        return Err(process_error(result.failure_detail()));
    }
    let value = serde_json::from_str(&result.stdout)?;
    let mut detection = detect_metadata(
        &value,
        video,
        input
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("mp4")),
    );
    detection.source_identity = identity;
    if preview && (detection.requires_choice || detection.confirmed_erp) {
        let output = std::env::temp_dir().join(format!(
            "ooosplat-panorama-preview-{}.jpg",
            uuid::Uuid::new_v4()
        ));
        let result = manager
            .run(ProcessSpec {
                executable: ffmpeg.into(),
                args: vec![
                    "-v".into(),
                    "error".into(),
                    "-i".into(),
                    input.into(),
                    "-frames:v".into(),
                    "1".into(),
                    "-vf".into(),
                    "scale=640:-2".into(),
                    "-q:v".into(),
                    "4".into(),
                    output.clone().into(),
                ],
                working_directory: None,
                log_path: None,
                observer: None,
            })
            .await;
        if matches!(result, Ok(ref r) if r.success)
            && fs::metadata(&output).is_ok_and(|m| m.len() <= 256 * 1024)
        {
            detection.preview_jpeg = fs::read(&output)?;
        }
        let _ = fs::remove_file(&output);
    }
    if source_identity(input)? != detection.source_identity {
        return Err(process_error("素材文件已发生变化，请重新分析后再生成"));
    }
    Ok(detection)
}

pub fn resolve(choice: VideoProjection, detection: &ProjectionDetection) -> Result<bool> {
    match choice {
        VideoProjection::Perspective => Ok(false),
        VideoProjection::Equirectangular if detection.supported_erp => Ok(true),
        VideoProjection::Equirectangular => Err(process_error("仅支持已拼接、单目、完整 360×180、2:1 ERP MP4；不支持双鱼眼、双目、VR180 或旋转/裁切的球面投影")),
        VideoProjection::Auto if detection.confirmed_erp => Ok(true),
        VideoProjection::Auto if detection.requires_choice => Err(process_error("无法仅凭 2:1 尺寸确认全景，请查看素材预览并选择普通视频或完整全景")),
        VideoProjection::Auto => Ok(false),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanoramaCheckpoint {
    pub version: u32,
    pub source_sha256: String,
    pub sfm_width: u32,
    pub sfm_height: u32,
    pub face_size: u32,
    pub field_of_view_degrees: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_sources: Option<Vec<super::image_projection::ImageSourceMapping>>,
}
impl PanoramaCheckpoint {
    pub fn new(source: &Path, quality: Quality, video: &VideoInfo) -> Result<Self> {
        Self::new_cancellable(
            source,
            quality,
            video,
            &tokio_util::sync::CancellationToken::new(),
        )
    }
    pub fn new_cancellable(
        source: &Path,
        quality: Quality,
        video: &VideoInfo,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Self> {
        Self::new_for_policy(source, quality, video, cancel, false)
    }
    pub fn new_for_policy(
        source: &Path,
        quality: Quality,
        video: &VideoInfo,
        cancel: &tokio_util::sync::CancellationToken,
        native: bool,
    ) -> Result<Self> {
        let (cap, face) = match quality {
            Quality::Fast => (2048, 512),
            Quality::Balanced => (3072, 768),
            Quality::High if native => (video.width, video.width / 4),
            Quality::High => (4096, 1024),
        };
        let width = if native && quality == Quality::High {
            video.width
        } else {
            video.width.min(cap) / 4 * 4
        };
        validate_training_cache_size(1, width as u64, u64::from(width / 2), face.min(width / 4))?;
        Ok(Self {
            version: CONVERSION_VERSION,
            source_sha256: fingerprint_cancellable(source, cancel)?,
            sfm_width: width,
            sfm_height: width / 2,
            face_size: face.min(width / 4),
            field_of_view_degrees: FOV_DEGREES,
            image_sources: None,
        })
    }
}
pub fn fingerprint(path: &Path) -> Result<String> {
    fingerprint_cancellable(path, &tokio_util::sync::CancellationToken::new())
}
pub(crate) fn fingerprint_cancellable(
    path: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String> {
    let mut source = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let size = source.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameTime {
    pub source_frame_index: u64,
    pub pts: i64,
    pub timestamp_seconds: f64,
}

/// Stream ffprobe records, not an unbounded JSON document. Keep actual decoder
/// frame indices/PTS (including VFR); never infer capture times from avg FPS.
pub async fn actual_frame_plan(
    ffprobe: &Path,
    input: &Path,
    plan: &mut FramePlan,
    manager: &ProcessManager,
) -> Result<Vec<FrameTime>> {
    let times = Arc::new(Mutex::new(Vec::<FrameTime>::new()));
    let target = times.clone();
    let cancellation = manager.clone();
    let observer = Arc::new(move |update| {
        if let ProcessUpdate::Line { line, .. } = update {
            let fields: std::collections::HashMap<_, _> =
                line.split('|').filter_map(|p| p.split_once('=')).collect();
            if fields.contains_key("best_effort_timestamp")
                || fields.contains_key("best_effort_timestamp_time")
            {
                let pts = fields
                    .get("best_effort_timestamp")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                let seconds = fields
                    .get("best_effort_timestamp_time")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(f64::NAN);
                let mut values = target.lock().unwrap();
                if values.len() >= 1_000_000 {
                    cancellation.cancel();
                    return;
                }
                let index = values.len() as u64;
                values.push(FrameTime {
                    source_frame_index: index,
                    pts,
                    timestamp_seconds: seconds,
                });
            }
        }
    });
    let result = manager
        .clone()
        .with_output_capture_limit(4096)
        .run(ProcessSpec {
            executable: ffprobe.into(),
            args: vec![
                "-v".into(),
                "error".into(),
                "-select_streams".into(),
                "v:0".into(),
                "-show_frames".into(),
                "-show_entries".into(),
                "frame=best_effort_timestamp,best_effort_timestamp_time".into(),
                "-of".into(),
                "compact=p=0:nk=0".into(),
                input.into(),
            ],
            working_directory: None,
            log_path: None,
            observer: Some(observer),
        })
        .await?;
    if !result.success {
        return Err(process_error(result.failure_detail()));
    }
    let times = times.lock().unwrap();
    apply_frame_times(plan, &times)
}

fn apply_frame_times(plan: &mut FramePlan, times: &[FrameTime]) -> Result<Vec<FrameTime>> {
    validate_frame_times(times)?;
    if plan.selected_frames.is_empty() {
        let count = plan.estimated_frames.min(times.len() as u64).max(1) as usize;
        plan.selected_frames = (0..count)
            .map(|i| PlannedFrame {
                source_frame_index: (i * times.len() / count) as u64,
                timestamp_seconds: 0.0,
            })
            .collect();
    }
    for frame in plan
        .selected_frames
        .iter_mut()
        .chain(plan.candidate_frames.iter_mut())
    {
        frame.timestamp_seconds = times
            .get(frame.source_frame_index as usize)
            .ok_or_else(|| process_error("视频帧数量与抽帧计划不一致"))?
            .timestamp_seconds;
    }
    plan.estimated_frames = plan.selected_frames.len() as u64;
    if plan.bridge_source_frames.is_some() {
        plan.use_manual_bridge_budget(times.len() as u64);
        plan.retention_ratio = plan.selected_frames.len() as f64 / times.len() as f64;
        // Already probed once for generation. Retain the bounded time index so
        // a later bridge can choose any source index without another ffprobe.
        return Ok(times.to_vec());
    }
    let wanted: std::collections::HashSet<_> = plan
        .selected_frames
        .iter()
        .chain(plan.candidate_frames.iter())
        .map(|f| f.source_frame_index)
        .collect();
    Ok(times
        .iter()
        .filter(|f| wanted.contains(&f.source_frame_index))
        .cloned()
        .collect())
}

fn validate_frame_times(times: &[FrameTime]) -> Result<()> {
    if times.is_empty()
        || times.len() >= 1_000_000
        || times
            .iter()
            .enumerate()
            .any(|(i, t)| t.source_frame_index != i as u64 || !t.timestamp_seconds.is_finite())
        || times
            .windows(2)
            .any(|p| p[1].timestamp_seconds < p[0].timestamp_seconds)
    {
        return Err(process_error(
            "视频帧时间戳不可用、非单调或超出 100 万帧限制",
        ));
    }
    Ok(())
}

pub(crate) fn bridge_source_frames(
    work: &Path,
    total: u64,
    indices: &[u64],
) -> Result<Vec<PlannedFrame>> {
    let times: Vec<FrameTime> = serde_json::from_reader(BufReader::new(File::open(
        work.join("panorama-frame-times.json"),
    )?))?;
    validate_frame_times(&times)?;
    if times.len() as u64 != total {
        return Err(process_error(
            "Panorama bridge source time index is inconsistent",
        ));
    }
    indices
        .iter()
        .map(|index| {
            let time = usize::try_from(*index)
                .ok()
                .and_then(|index| times.get(index))
                .ok_or_else(|| process_error("Panorama bridge frame index is out of range"))?;
            Ok(PlannedFrame {
                source_frame_index: *index,
                timestamp_seconds: time.timestamp_seconds,
            })
        })
        .collect()
}

type V3 = [f64; 3];
type M3 = [[f64; 3]; 3];
// Rows = face right/down/forward in ERP camera space. Rotation maps ERP -> face.
pub const FACES: [(&str, M3); 6] = [
    ("front", [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]),
    ("back", [[-1., 0., 0.], [0., 1., 0.], [0., 0., -1.]]),
    ("left", [[0., 0., 1.], [0., 1., 0.], [-1., 0., 0.]]),
    ("right", [[0., 0., -1.], [0., 1., 0.], [1., 0., 0.]]),
    ("up", [[1., 0., 0.], [0., 0., 1.], [0., -1., 0.]]),
    ("down", [[1., 0., 0.], [0., 0., -1.], [0., 1., 0.]]),
];
fn transpose(m: M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}
fn mul(m: M3, v: V3) -> V3 {
    m.map(|r| r[0] * v[0] + r[1] * v[1] + r[2] * v[2])
}
fn product(a: M3, b: M3) -> M3 {
    transpose(transpose(b).map(|col| mul(a, col)))
}
fn rotation([w, x, y, z]: [f64; 4]) -> M3 {
    [
        [
            1. - 2. * (y * y + z * z),
            2. * (x * y - z * w),
            2. * (x * z + y * w),
        ],
        [
            2. * (x * y + z * w),
            1. - 2. * (x * x + z * z),
            2. * (y * z - x * w),
        ],
        [
            2. * (x * z - y * w),
            2. * (y * z + x * w),
            1. - 2. * (x * x + y * y),
        ],
    ]
}
fn quaternion(m: M3) -> [f64; 4] {
    let trace = m[0][0] + m[1][1] + m[2][2];
    let q = if trace > 0. {
        let s = (trace + 1.).sqrt() * 2.;
        [
            s / 4.,
            (m[2][1] - m[1][2]) / s,
            (m[0][2] - m[2][0]) / s,
            (m[1][0] - m[0][1]) / s,
        ]
    } else {
        let i = if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
            0
        } else if m[1][1] > m[2][2] {
            1
        } else {
            2
        };
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        let s = (1. + m[i][i] - m[j][j] - m[k][k]).sqrt() * 2.;
        let mut q = [0.; 4];
        q[0] = (m[k][j] - m[j][k]) / s;
        q[i + 1] = s / 4.;
        q[j + 1] = (m[j][i] + m[i][j]) / s;
        q[k + 1] = (m[k][i] + m[i][k]) / s;
        q
    };
    let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    q.map(|v| v / norm)
}
pub fn focal(size: u32) -> f64 {
    size as f64 / (2. * (FOV_DEGREES.to_radians() / 2.).tan())
}
pub fn erp_pixel(q: M3, x: u32, y: u32, size: u32, width: u32, height: u32) -> (f64, f64) {
    let center = size as f64 / 2.;
    let f = focal(size);
    let ray = mul(
        transpose(q),
        [
            (x as f64 + 0.5 - center) / f,
            (y as f64 + 0.5 - center) / f,
            1.,
        ],
    );
    let u = (ray[0].atan2(ray[2]) / std::f64::consts::TAU + 0.5) * width as f64 - 0.5;
    let v = (ray[1].atan2(ray[0].hypot(ray[2])) / std::f64::consts::PI + 0.5) * height as f64 - 0.5;
    (u.rem_euclid(width as f64), v.clamp(0., height as f64 - 1.))
}
pub(crate) fn render_face(
    source: &image::RgbaImage,
    mask: Option<&image::GrayImage>,
    q: M3,
    size: u32,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<image::RgbaImage> {
    let (w, h) = source.dimensions();
    let mut out = image::RgbaImage::new(size, size);
    for y in 0..size {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        for x in 0..size {
            let (u, v) = erp_pixel(q, x, y, size, w, h);
            let x0 = u.floor() as u32;
            let y0 = v.floor() as u32;
            let dx = u - u.floor();
            let dy = v - v.floor();
            let mut result = [0.; 4];
            for (px, py, weight) in [
                (x0, y0, (1. - dx) * (1. - dy)),
                ((x0 + 1) % w, y0, dx * (1. - dy)),
                (x0, (y0 + 1).min(h - 1), (1. - dx) * dy),
                ((x0 + 1) % w, (y0 + 1).min(h - 1), dx * dy),
            ] {
                let p = source.get_pixel(px, py);
                for c in 0..4 {
                    result[c] += p[c] as f64 * weight;
                }
            }
            let mut pixel = result.map(|v| v.round().clamp(0., 255.) as u8);
            if let Some(mask) = mask {
                pixel[3] = if mask.get_pixel((u.round() as u32) % w, (v.round() as u32).min(h - 1))
                    [0]
                    >= 128
                {
                    pixel[3]
                } else {
                    0
                };
            }
            out.put_pixel(x, y, image::Rgba(pixel));
        }
    }
    Ok(out)
}

fn u32r(r: &mut impl Read) -> Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn u64r(r: &mut impl Read) -> Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn f64r(r: &mut impl Read) -> Result<f64> {
    Ok(f64::from_bits(u64r(r)?))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PosedImage {
    pub id: u32,
    pub camera_id: u32,
    pub name: String,
    pub qvec: [f64; 4],
    pub tvec: V3,
}
pub fn read_poses(model: &Path) -> Result<Vec<PosedImage>> {
    if !model.join("images.bin").is_file() {
        let text = fs::read_to_string(model.join("images.txt"))?;
        let mut lines = text.lines().filter(|l| !l.trim_start().starts_with('#'));
        let mut poses = Vec::new();
        while let Some(line) = lines.next() {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 10 {
                return Err(process_error("Invalid COLMAP image pose"));
            }
            let numbers = fields[1..8]
                .iter()
                .map(|s| {
                    s.parse::<f64>()
                        .map_err(|_| process_error("Invalid COLMAP pose"))
                })
                .collect::<Result<Vec<_>>>()?;
            if numbers.iter().any(|n| !n.is_finite()) {
                return Err(process_error("Non-finite COLMAP pose"));
            }
            let name = fields[9..].join(" ");
            if name.contains(['/', '\\']) || name == ".." {
                return Err(process_error("Invalid panorama image path"));
            }
            poses.push(PosedImage {
                id: fields[0]
                    .parse()
                    .map_err(|_| process_error("Invalid image id"))?,
                camera_id: fields[8]
                    .parse()
                    .map_err(|_| process_error("Invalid camera id"))?,
                name,
                qvec: numbers[0..4].try_into().unwrap(),
                tvec: numbers[4..7].try_into().unwrap(),
            });
            lines.next(); // Observations are deliberately not carried into training faces.
        }
        poses.sort_by(|a, b| a.name.cmp(&b.name));
        return Ok(poses);
    }
    let mut r = BufReader::new(File::open(model.join("images.bin"))?);
    let n = u64r(&mut r)?;
    if n > 1_000_000 {
        return Err(process_error("Too many panorama poses"));
    }
    let mut out = Vec::new();
    for _ in 0..n {
        let id = u32r(&mut r)?;
        let mut q = [0.; 4];
        for v in &mut q {
            *v = f64r(&mut r)?;
        }
        let mut t = [0.; 3];
        for v in &mut t {
            *v = f64r(&mut r)?;
        }
        let camera_id = u32r(&mut r)?;
        let mut name = Vec::new();
        loop {
            let mut b = [0];
            r.read_exact(&mut b)?;
            if b[0] == 0 {
                break;
            }
            name.push(b[0]);
            if name.len() > 4096 {
                return Err(process_error("Invalid image name"));
            }
        }
        let name = String::from_utf8(name).map_err(|_| process_error("Non UTF-8 image name"))?;
        // Internal frames have ASCII basenames. Reject path traversal in model files.
        if name.contains(['/', '\\']) || name == ".." {
            return Err(process_error("Invalid panorama image path"));
        }
        let observations = u64r(&mut r)?
            .checked_mul(24)
            .ok_or_else(|| process_error("Invalid observations"))?;
        if std::io::copy(&mut r.by_ref().take(observations), &mut std::io::sink())? != observations
        {
            return Err(process_error("Truncated images.bin"));
        }
        if q.iter().chain(t.iter()).any(|v| !v.is_finite())
            || (q.iter().map(|v| v * v).sum::<f64>() - 1.).abs() > 1e-5
        {
            return Err(process_error("Invalid panorama pose"));
        }
        out.push(PosedImage {
            id,
            camera_id,
            name,
            qvec: q,
            tvec: t,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Multi-batch ERP conversion. Pixels are independent of world pose, while the
/// camera model is rebuilt from the authoritative sphere on every invocation.
pub fn prepare_reshoot_training_cache(
    frames: &Path,
    model: &Path,
    root: &Path,
    records: &[crate::pipeline::reshoot::PanoramaFrameRecord],
    quality: Quality,
    cancel: &tokio_util::sync::CancellationToken,
    progress: impl Fn(TrainingCacheUpdate),
) -> Result<PathBuf> {
    prepare_reshoot_training_cache_for_policy(
        frames, model, root, records, quality, false, cancel, progress,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_reshoot_training_cache_for_policy(
    frames: &Path,
    model: &Path,
    root: &Path,
    records: &[crate::pipeline::reshoot::PanoramaFrameRecord],
    quality: Quality,
    native: bool,
    cancel: &tokio_util::sync::CancellationToken,
    progress: impl Fn(TrainingCacheUpdate),
) -> Result<PathBuf> {
    let cameras = crate::reconstruction::colmap_model::read_cameras(model)?;
    let poses = read_poses(model)?;
    let cap = match quality {
        Quality::Fast => 512,
        Quality::Balanced => 768,
        Quality::High if native => u32::MAX,
        Quality::High => 1024,
    };
    let images = root.join("images");
    let sparse = root.join("sparse/0");
    fs::create_dir_all(&images)?;
    fs::create_dir_all(&sparse)?;
    let ordered: Vec<_> = records
        .iter()
        .filter_map(|record| {
            poses
                .iter()
                .find(|p| p.name == record.prepared_name)
                .map(|pose| (record, pose))
        })
        .collect();
    if ordered.len() != poses.len() || ordered.is_empty() {
        return Err(process_error("Incomplete panorama batch/source mapping"));
    }
    let mut sizes: Vec<u32> = Vec::new();
    let mut face_rows = Vec::new();
    let mut manifest = Vec::new();
    for (index, (record, pose)) in ordered.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let camera = cameras
            .iter()
            .find(|c| c.id == pose.camera_id)
            .ok_or_else(|| process_error("Missing ERP camera"))?;
        if camera.model_id != 17
            || camera.width != camera.height.checked_mul(2).unwrap_or(0)
            || camera.params != vec![camera.width as f64, camera.height as f64]
        {
            return Err(process_error("Invalid ERP camera"));
        }
        let size = cap.min(
            u32::try_from(camera.width / 4)
                .map_err(|_| process_error("ERP dimensions overflow"))?,
        );
        validate_training_cache_size(1, camera.width, camera.height, size)?;
        if size == 0 {
            return Err(process_error("Invalid panorama face size"));
        }
        if !sizes.contains(&size) {
            sizes.push(size);
        }
        let camera_id = (sizes.iter().position(|s| *s == size).unwrap() + 1) as u32;
        let basename = format!("frame_{index:010}");
        let published = images.join(&basename);
        let source = frames.join(&pose.name);
        let key = format!(
            "reshoot-v2:{}:{}:{}",
            fingerprint_cancellable(&source, cancel)?,
            size,
            FOV_DEGREES
        );
        let reusable = fs::read_to_string(published.join("complete"))
            .ok()
            .as_deref()
            == Some(&key)
            && FACES.iter().all(|(name, _)| {
                image::image_dimensions(published.join(format!("{name}.png"))).ok()
                    == Some((size, size))
            });
        if !reusable {
            let staging = images.join(format!("{basename}.partial"));
            if staging.exists() {
                fs::remove_dir_all(&staging)?;
            }
            fs::create_dir_all(&staging)?;
            let prepared = (|| -> Result<()> {
                let mut reader = image::ImageReader::open(&source)?.with_guessed_format()?;
                let mut limits = image::Limits::default();
                limits.max_alloc = Some(MAX_DECODE_BYTES);
                reader.limits(limits);
                let rgba = reader
                    .decode()
                    .map_err(|e| process_error(e.to_string()))?
                    .to_rgba8();
                if rgba.width() as u64 != camera.width || rgba.height() as u64 != camera.height {
                    return Err(process_error("ERP image/camera dimensions differ"));
                }
                for (face_index, (name, q)) in FACES.iter().enumerate() {
                    render_face(&rgba, None, *q, size, cancel)?
                        .save(staging.join(format!("{name}.png")))
                        .map_err(|e| process_error(e.to_string()))?;
                    progress(TrainingCacheUpdate::Faces {
                        completed: (index * 6 + face_index + 1) as u64,
                        total: ordered.len() as u64 * 6,
                        reused: false,
                    });
                }
                fs::write(staging.join("complete"), &key)?;
                Ok(())
            })();
            if let Err(error) = prepared {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
            if published.exists() {
                fs::remove_dir_all(&published)?;
            }
            fs::rename(staging, published)?;
        } else {
            progress(TrainingCacheUpdate::Faces {
                completed: (index + 1) as u64 * 6,
                total: ordered.len() as u64 * 6,
                reused: true,
            });
        }
        for (face_index, (name, q)) in FACES.iter().enumerate() {
            let id = u32::try_from(
                index
                    .checked_mul(6)
                    .and_then(|n| n.checked_add(face_index + 1))
                    .ok_or_else(|| process_error("Face count overflow"))?,
            )
            .map_err(|_| process_error("Face count overflow"))?;
            let image_name = format!("{basename}/{name}.png");
            let qvec = quaternion(product(*q, rotation(pose.qvec)));
            let tvec = mul(*q, pose.tvec);
            face_rows.push((id, camera_id, image_name.clone(), qvec, tvec));
            manifest.push(serde_json::json!({"source_frame_index":record.source_index,"source_image_index":if record.timestamp_seconds.is_none() {Some(record.source_index)} else {None},"timestamp_seconds":record.timestamp_seconds,"pts":record.pts,"source_name":record.source_name,"source_sha256":record.source_sha256,"orientation":record.orientation,"batch_id":record.batch_id,"erp_image_id":pose.id,"erp_name":pose.name,"face":name,"image_id":id,"image_name":image_name,"qvec":qvec,"tvec":tvec}));
        }
    }
    progress(TrainingCacheUpdate::Validating);
    let mut out = BufWriter::new(File::create(sparse.join("cameras.bin"))?);
    out.write_all(&(sizes.len() as u64).to_le_bytes())?;
    for (index, size) in sizes.iter().enumerate() {
        out.write_all(&((index + 1) as u32).to_le_bytes())?;
        out.write_all(&1i32.to_le_bytes())?;
        out.write_all(&(*size as u64).to_le_bytes())?;
        out.write_all(&(*size as u64).to_le_bytes())?;
        for value in [
            focal(*size),
            focal(*size),
            *size as f64 / 2.,
            *size as f64 / 2.,
        ] {
            out.write_all(&value.to_le_bytes())?;
        }
    }
    out.flush()?;
    let mut out = BufWriter::new(File::create(sparse.join("images.bin"))?);
    out.write_all(&(face_rows.len() as u64).to_le_bytes())?;
    for (id, camera_id, name, q, t) in &face_rows {
        out.write_all(&id.to_le_bytes())?;
        for value in q.iter().chain(t) {
            out.write_all(&value.to_le_bytes())?;
        }
        out.write_all(&camera_id.to_le_bytes())?;
        out.write_all(name.as_bytes())?;
        out.write_all(&[0])?;
        out.write_all(&0u64.to_le_bytes())?;
    }
    out.flush()?;
    let mut source = BufReader::new(File::open(model.join("points3D.bin"))?);
    let count = u64r(&mut source)?;
    let mut out = BufWriter::new(File::create(sparse.join("points3D.bin"))?);
    out.write_all(&count.to_le_bytes())?;
    for _ in 0..count {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let mut point = [0u8; 43];
        source.read_exact(&mut point)?;
        out.write_all(&point)?;
        let n = u64r(&mut source)?
            .checked_mul(8)
            .ok_or_else(|| process_error("Invalid tracks"))?;
        if std::io::copy(&mut source.by_ref().take(n), &mut std::io::sink())? != n {
            return Err(process_error("Truncated points3D tracks"));
        }
        out.write_all(&0u64.to_le_bytes())?;
    }
    out.flush()?;
    if read_poses_for_training_count(&sparse)? != face_rows.len() as u64 {
        return Err(process_error("Incomplete face camera model"));
    }
    let reference = face_rows
        .first()
        .map(|r| r.2.clone())
        .ok_or_else(|| process_error("Missing front camera"))?;
    let pending = root.join("manifest.json.partial");
    fs::write(
        &pending,
        serde_json::to_vec_pretty(
            &serde_json::json!({"recipeVersion":2,"reference_image":reference,"faces":manifest}),
        )?,
    )?;
    let manifest_path = root.join("manifest.json");
    if manifest_path.exists() {
        fs::remove_file(&manifest_path)?;
    }
    fs::rename(pending, manifest_path)?;
    fs::write(
        root.join("signature.json"),
        serde_json::to_vec(&(CONVERSION_VERSION, quality, records.len()))?,
    )?;
    Ok(root.into())
}

fn read_poses_for_training_count(model: &Path) -> Result<u64> {
    Ok(crate::reconstruction::colmap_model::read_registered_images(model)?.len() as u64)
}

#[derive(Serialize, Deserialize)]
struct FaceMapping {
    source_frame_index: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timestamp_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pts: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_image_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_name: Option<String>,
    erp_image_id: u32,
    erp_name: String,
    face: String,
    image_id: u32,
    image_name: String,
    qvec: [f64; 4],
    tvec: V3,
}
#[derive(Serialize, Deserialize)]
struct CacheManifest {
    config: PanoramaCheckpoint,
    sphere_sha256: Vec<String>,
    faces: Vec<FaceMapping>,
}

/// One ERP + one face resident. Per-ERP atomic batches resume independently.
pub enum TrainingCacheUpdate {
    Faces {
        completed: u64,
        total: u64,
        reused: bool,
    },
    Validating,
}

// Validate representable sizes and the per-image decoder limit, not total disk usage.
fn validate_training_cache_size(count: u64, width: u64, height: u64, face: u32) -> Result<()> {
    let faces = count
        .checked_mul(6)
        .filter(|n| *n <= u32::MAX as u64)
        .ok_or_else(|| process_error("Panorama image count overflow"))?;
    faces
        .checked_mul(face as u64)
        .and_then(|n| n.checked_mul(face as u64))
        .and_then(|n| n.checked_mul(6))
        .ok_or_else(|| process_error("Panorama cache size overflow"))?;
    let decoded = width.checked_mul(height).and_then(|n| n.checked_mul(4));
    if width == 0 || height == 0 || decoded.is_none_or(|n| n > MAX_DECODE_BYTES) {
        return Err(process_error(
            "Panorama image exceeds the 512 MiB decode limit",
        ));
    }
    Ok(())
}

pub fn prepare_training_cache(
    frames: &Path,
    masks: &Path,
    model: &Path,
    root: &Path,
    config: &PanoramaCheckpoint,
    cancel: &tokio_util::sync::CancellationToken,
    progress: impl Fn(TrainingCacheUpdate),
) -> Result<PathBuf> {
    if config.version != CONVERSION_VERSION
        || config.face_size == 0
        || config.face_size > config.sfm_width / 4
        || config.field_of_view_degrees != FOV_DEGREES
    {
        return Err(process_error(
            "Unsupported panorama conversion configuration",
        ));
    }
    let camera = crate::reconstruction::colmap_model::read_single_camera(model)?;
    if camera.model_id != 17
        || camera.params != vec![camera.width as f64, camera.height as f64]
        || camera.width != config.sfm_width as u64
        || camera.height != config.sfm_height as u64
    {
        return Err(process_error(
            "Panorama SfM requires COLMAP camera 17 with width,height parameters",
        ));
    }
    let poses = read_poses(model)?;
    let times_path = frames
        .parent()
        .unwrap_or(frames)
        .join("panorama-frame-times.json");
    let times: Vec<FrameTime> = if config.image_sources.is_some() {
        Vec::new()
    } else {
        serde_json::from_slice(&fs::read(times_path)?)?
    };
    let by_index: std::collections::HashMap<_, _> =
        times.iter().map(|t| (t.source_frame_index, t)).collect();
    if poses.is_empty() {
        return Err(process_error("No registered panorama frames"));
    }
    validate_training_cache_size(
        poses.len() as u64,
        camera.width,
        camera.height,
        config.face_size,
    )?;
    let hashes = ["cameras.bin", "images.bin", "points3D.bin"]
        .into_iter()
        .map(|f| fingerprint_cancellable(&model.join(f), cancel))
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir_all(root)?;
    let signature = serde_json::to_string(&(config, &hashes))?;
    let signature_path = root.join("signature.json");
    if fs::read_to_string(&signature_path).ok().as_deref() != Some(&signature) {
        // This is exclusively a derived, project-owned cache directory.
        for entry in fs::read_dir(root)? {
            let p = entry?.path();
            if p.is_dir() {
                fs::remove_dir_all(p)?;
            } else {
                fs::remove_file(p)?;
            }
        }
        fs::write(&signature_path, signature)?;
    }
    let images = root.join("images");
    let sparse = root.join("sparse/0");
    fs::create_dir_all(&images)?;
    fs::create_dir_all(&sparse)?;
    let mut mappings = Vec::new();
    for (index, pose) in poses.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let basename = format!("erp_{:010}", pose.id);
        let published = images.join(&basename);
        let batch_valid = published.join("complete").is_file()
            && FACES.iter().all(|(f, _)| {
                image::image_dimensions(published.join(format!("{f}.png")))
                    .is_ok_and(|d| d == (config.face_size, config.face_size))
            });
        if !batch_valid {
            let staging = images.join(format!("{basename}.partial"));
            if staging.exists() {
                fs::remove_dir_all(&staging)?;
            }
            fs::create_dir_all(&staging)?;
            let result = (|| {
                let source = image::open(frames.join(&pose.name))
                    .map_err(|e| process_error(e.to_string()))?
                    .to_rgba8();
                if source.dimensions() != (camera.width as u32, camera.height as u32) {
                    return Err(process_error("Panorama image/camera dimensions differ"));
                }
                let mask_path = masks.join(format!("{}.png", pose.name));
                let mask = if mask_path.is_file() {
                    Some(
                        image::open(&mask_path)
                            .map_err(|e| process_error(e.to_string()))?
                            .to_luma8(),
                    )
                } else {
                    None
                };
                if mask
                    .as_ref()
                    .is_some_and(|m| m.dimensions() != source.dimensions())
                {
                    return Err(process_error("Panorama mask/image dimensions differ"));
                }
                for (face_index, (name, q)) in FACES.into_iter().enumerate() {
                    // Image masks are thresholded Alpha for SfM, not independent
                    // ignore regions. Do not threshold the continuous training Alpha again.
                    let face = render_face(
                        &source,
                        if config.image_sources.is_some() {
                            None
                        } else {
                            mask.as_ref()
                        },
                        q,
                        config.face_size,
                        cancel,
                    )?;
                    face.save(staging.join(format!("{name}.png")))
                        .map_err(|e| process_error(e.to_string()))?;
                    progress(TrainingCacheUpdate::Faces {
                        completed: (index * 6 + face_index + 1) as u64,
                        total: (poses.len() * 6) as u64,
                        reused: false,
                    });
                }
                fs::write(staging.join("complete"), b"1")?;
                Ok(())
            })();
            if let Err(e) = result {
                let _ = fs::remove_dir_all(&staging);
                return Err(e);
            }
            if published.exists() {
                fs::remove_dir_all(&published)?;
            }
            fs::rename(staging, &published)?;
        } else {
            progress(TrainingCacheUpdate::Faces {
                completed: ((index + 1) * 6) as u64,
                total: (poses.len() * 6) as u64,
                reused: true,
            });
        }
        for (face_index, (face, q)) in FACES.iter().enumerate() {
            let source_index = Path::new(&pose.name)
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("frame_"))
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| process_error("Panorama frame has no source index"))?;
            let source_image = config
                .image_sources
                .as_ref()
                .and_then(|v| v.iter().find(|s| s.source_image_index == source_index));
            let time = by_index.get(&source_index);
            if source_image.is_none() && time.is_none() {
                return Err(process_error("Missing panorama timestamp mapping"));
            }
            mappings.push(FaceMapping {
                source_frame_index: source_index,
                timestamp_seconds: time.map(|v| v.timestamp_seconds),
                pts: time.map(|v| v.pts),
                source_image_index: source_image.map(|v| v.source_image_index),
                source_name: source_image.map(|v| v.source_name.clone()),
                erp_image_id: pose.id,
                erp_name: pose.name.clone(),
                face: face.to_string(),
                image_id: (index * 6 + face_index + 1) as u32,
                image_name: format!("{basename}/{face}.png"),
                qvec: quaternion(product(*q, rotation(pose.qvec))),
                tvec: mul(*q, pose.tvec),
            });
        }
    }
    progress(TrainingCacheUpdate::Validating);
    let mut cameras = BufWriter::new(File::create(sparse.join("cameras.bin"))?);
    cameras.write_all(&1u64.to_le_bytes())?;
    cameras.write_all(&1u32.to_le_bytes())?;
    cameras.write_all(&1i32.to_le_bytes())?;
    let size = config.face_size as u64;
    for _ in 0..2 {
        cameras.write_all(&size.to_le_bytes())?;
    }
    for v in [
        focal(config.face_size),
        focal(config.face_size),
        size as f64 / 2.,
        size as f64 / 2.,
    ] {
        cameras.write_all(&v.to_le_bytes())?;
    }
    cameras.flush()?;
    let mut images_bin = BufWriter::new(File::create(sparse.join("images.bin"))?);
    images_bin.write_all(&(mappings.len() as u64).to_le_bytes())?;
    for face in &mappings {
        images_bin.write_all(&face.image_id.to_le_bytes())?;
        for v in face.qvec.iter().chain(face.tvec.iter()) {
            images_bin.write_all(&v.to_le_bytes())?;
        }
        images_bin.write_all(&1u32.to_le_bytes())?;
        images_bin.write_all(face.image_name.as_bytes())?;
        images_bin.write_all(&[0])?;
        images_bin.write_all(&0u64.to_le_bytes())?;
    }
    images_bin.flush()?;
    // Keep positions, colors and errors in the same world frame; discard ALL
    // old ERP tracks. They do not refer to the new pinhole image/observation IDs.
    let mut src = BufReader::new(File::open(model.join("points3D.bin"))?);
    let n = u64r(&mut src)?;
    let mut dst = BufWriter::new(File::create(sparse.join("points3D.bin"))?);
    dst.write_all(&n.to_le_bytes())?;
    for _ in 0..n {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let mut record = [0; 43];
        src.read_exact(&mut record)?;
        dst.write_all(&record)?;
        let bytes = u64r(&mut src)?
            .checked_mul(8)
            .ok_or_else(|| process_error("Invalid tracks"))?;
        if std::io::copy(&mut src.by_ref().take(bytes), &mut std::io::sink())? != bytes {
            return Err(process_error("Truncated points3D.bin"));
        }
        dst.write_all(&0u64.to_le_bytes())?;
    }
    dst.flush()?;
    let derived_camera = crate::reconstruction::colmap_model::read_single_camera(&sparse)?;
    let derived_images = crate::reconstruction::colmap_model::read_registered_images(&sparse)?;
    if derived_camera.model != "PINHOLE" || derived_images.len() != mappings.len() {
        return Err(process_error("训练图片和相机数据校验失败"));
    }
    let manifest = CacheManifest {
        config: config.clone(),
        sphere_sha256: hashes,
        faces: mappings,
    };
    let pending = root.join("manifest.json.partial");
    fs::write(&pending, serde_json::to_vec_pretty(&manifest)?)?;
    let manifest_path = root.join("manifest.json");
    if manifest_path.exists() {
        fs::remove_file(&manifest_path)?;
    }
    fs::rename(pending, manifest_path)?;
    Ok(root.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn training_cache_above_twenty_gib_keeps_decode_and_count_guards() {
        // Former face-cache estimate: 1000 * 6 * 1024^2 * 6 > 20 GiB.
        assert!(1000_u64 * 36 * 1024 * 1024 > 20 * 1024_u64.pow(3));
        assert!(validate_training_cache_size(1000, 4096, 2048, 1024).is_ok());
        assert!(validate_training_cache_size(1, 16_384, 8192, 1024).is_ok());
        assert!(validate_training_cache_size(1, 16_386, 8193, 1024).is_err());
        assert!(validate_training_cache_size(1, 0, 2048, 1024).is_err());
        assert!(validate_training_cache_size(u64::MAX, 4096, 2048, 1024).is_err());
        assert!(validate_training_cache_size(u32::MAX as u64 / 6 + 1, 64, 32, 16).is_err());
        assert!(validate_training_cache_size(1, u64::MAX, 2, 1024).is_err());
    }

    /// Density diagnostics only: retain registered poses/points, project the
    /// original source pixels at native resolution into an isolated cache.
    #[test]
    #[ignore = "Requires an existing real panorama image project; writes isolated cache only"]
    fn native_resolution_density_diagnostic_cache() {
        use image::ImageEncoder;
        let project = PathBuf::from(
            std::env::var_os("OOOSPLAT_PANORAMA_DENSITY_PROJECT")
                .expect("Set completed image project"),
        );
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let base = project.join("work/panorama-training");
        let mut manifest: CacheManifest =
            serde_json::from_slice(&fs::read(base.join("manifest.json")).unwrap()).unwrap();
        assert!(manifest.config.image_sources.is_some());
        assert!(manifest
            .config
            .image_sources
            .as_ref()
            .unwrap()
            .iter()
            .all(|source| source.orientation == 1));
        let cache_root = workspace.join(".cache/panorama-training-v1");
        fs::create_dir_all(&cache_root).unwrap();
        let root = if let Some(existing) = std::env::var_os("OOOSPLAT_PANORAMA_DENSITY_CACHE") {
            let existing = fs::canonicalize(existing).unwrap();
            assert_eq!(
                existing.parent().unwrap(),
                fs::canonicalize(&cache_root).unwrap()
            );
            assert!(existing
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("native-faces-"));
            let recorded: serde_json::Value =
                serde_json::from_slice(&fs::read(existing.join("manifest.json")).unwrap()).unwrap();
            let previous: CacheManifest = serde_json::from_value(recorded.clone()).unwrap();
            assert_eq!(previous.sphere_sha256, manifest.sphere_sha256);
            assert_eq!(
                serde_json::to_value(&previous.faces).unwrap(),
                serde_json::to_value(&manifest.faces).unwrap()
            );
            assert_eq!(
                recorded["diagnosticProvenance"]["sourceProject"],
                serde_json::to_value(&project).unwrap()
            );
            existing
        } else {
            tempfile::Builder::new()
                .prefix("native-faces-")
                .tempdir_in(&cache_root)
                .unwrap()
                .keep()
        };
        let sparse = root.join("sparse/0");
        fs::create_dir_all(&sparse).unwrap();
        let before: Vec<_> = ["cameras.bin", "images.bin", "points3D.bin"]
            .map(|name| fingerprint(&base.join("sparse/0").join(name)).unwrap())
            .into();
        let mut dimensions = None;
        for batch in manifest.faces.chunks(6) {
            assert_eq!(batch.len(), 6);
            let source_path = project
                .join("source/images")
                .join(batch[0].source_name.as_ref().unwrap());
            let (width, height) = image::image_dimensions(&source_path).unwrap();
            let size = (width / 4).min(1024);
            assert_eq!(width, height * 2);
            assert!(size >= manifest.config.face_size);
            if let Some(previous) = dimensions {
                assert_eq!(previous, (width, height, size));
            }
            dimensions = Some((width, height, size));
            let published = root
                .join("images")
                .join(Path::new(&batch[0].image_name).parent().unwrap());
            if published.join("complete").is_file()
                && batch.iter().all(|m| {
                    image::image_dimensions(root.join("images").join(&m.image_name))
                        .is_ok_and(|d| d == (size, size))
                })
            {
                continue;
            }
            let source = image::open(&source_path).unwrap().to_rgba8();
            for mapping in batch {
                let q = FACES
                    .iter()
                    .find(|(name, _)| *name == mapping.face)
                    .unwrap()
                    .1;
                let face = render_face(
                    &source,
                    None,
                    q,
                    size,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .unwrap();
                let destination = root.join("images").join(&mapping.image_name);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                image::codecs::png::PngEncoder::new_with_quality(
                    BufWriter::new(File::create(destination).unwrap()),
                    image::codecs::png::CompressionType::Fast,
                    image::codecs::png::FilterType::NoFilter,
                )
                .write_image(face.as_raw(), size, size, image::ExtendedColorType::Rgba8)
                .unwrap();
            }
            fs::write(
                root.join("images")
                    .join(Path::new(&batch[0].image_name).parent().unwrap())
                    .join("complete"),
                b"1",
            )
            .unwrap();
            if batch[0].source_image_index.unwrap_or(0).is_multiple_of(10) {
                eprintln!(
                    "Native face projection: {}",
                    batch[0].source_name.as_ref().unwrap()
                );
            }
        }
        let (width, height, size) = dimensions.unwrap();
        let mut camera = fs::read(base.join("sparse/0/cameras.bin")).unwrap();
        assert_eq!(camera.len(), 64); // One shared PINHOLE camera.
        assert_eq!(i32::from_le_bytes(camera[12..16].try_into().unwrap()), 1);
        camera[16..24].copy_from_slice(&(size as u64).to_le_bytes());
        camera[24..32].copy_from_slice(&(size as u64).to_le_bytes());
        for (i, value) in [focal(size), focal(size), size as f64 / 2., size as f64 / 2.]
            .iter()
            .enumerate()
        {
            camera[32 + i * 8..40 + i * 8].copy_from_slice(&value.to_le_bytes());
        }
        fs::write(sparse.join("cameras.bin"), camera).unwrap();
        for name in ["images.bin", "points3D.bin"] {
            fs::copy(base.join("sparse/0").join(name), sparse.join(name)).unwrap();
        }
        manifest.config.face_size = size;
        manifest.config.sfm_width = width;
        manifest.config.sfm_height = height;
        let mut recorded = serde_json::to_value(&manifest).unwrap();
        recorded["diagnosticProvenance"] = serde_json::json!({
            "sourceProject": project, "registeredPosesReused": true,
            "sfmNotRerun": true, "sourceRgb": "original images (not upscaled prepared frames)",
            "note": "Training cache only; no new authoritative spherical model was generated.",
        });
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&recorded).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("signature.json"),
            serde_json::to_vec(&recorded).unwrap(),
        )
        .unwrap();
        assert_eq!(
            crate::reconstruction::colmap_model::read_single_camera(&sparse)
                .unwrap()
                .width,
            size as u64
        );
        assert_eq!(
            crate::reconstruction::colmap_model::read_registered_images(&sparse)
                .unwrap()
                .len(),
            manifest.faces.len()
        );
        for (i, name) in ["cameras.bin", "images.bin", "points3D.bin"]
            .iter()
            .enumerate()
        {
            assert_eq!(
                fingerprint(&base.join("sparse/0").join(name)).unwrap(),
                before[i]
            );
        }
        fs::write(workspace.join(".cache/panorama-training-v1/native-cache.json"), serde_json::to_vec_pretty(&serde_json::json!({ "dataset": root, "width": width, "height": height, "faceSize": size })).unwrap()).unwrap();
        eprintln!("Native-resolution diagnostic dataset: {}", root.display());
    }
    #[test]
    fn manual_bridge_retains_full_vfr_index_for_late_source_candidates() {
        let work = tempfile::tempdir().unwrap();
        let times: Vec<_> = (0..20)
            .map(|index| FrameTime {
                source_frame_index: index,
                pts: 10000 + index as i64 * index as i64,
                timestamp_seconds: 10. + (index * index) as f64 / 1000.,
            })
            .collect();
        let mut plan = FramePlan {
            selected_frames: (0..20)
                .step_by(2)
                .map(|index| PlannedFrame {
                    source_frame_index: index,
                    timestamp_seconds: 0.,
                })
                .collect(),
            candidate_frames: Vec::new(),
            bridge_source_frames: Some(20),
            ..Default::default()
        };
        let mapped = apply_frame_times(&mut plan, &times).unwrap();
        assert_eq!(mapped.len(), 20);
        assert_eq!(plan.rescue_max_frames, 12);
        fs::write(
            work.path().join("panorama-frame-times.json"),
            serde_json::to_vec(&mapped).unwrap(),
        )
        .unwrap();
        let added = bridge_source_frames(work.path(), 20, &[9, 11]).unwrap();
        assert_eq!(added[0].timestamp_seconds, 10.081);
        assert_eq!(added[1].timestamp_seconds, 10.121);
        assert!(bridge_source_frames(work.path(), 19, &[9]).is_err());
        assert!(bridge_source_frames(work.path(), 20, &[20]).is_err());
        let mut corrupt = mapped;
        corrupt[9].source_frame_index = 8;
        fs::write(
            work.path().join("panorama-frame-times.json"),
            serde_json::to_vec(&corrupt).unwrap(),
        )
        .unwrap();
        assert!(bridge_source_frames(work.path(), 20, &[9]).is_err());
    }

    #[test]
    fn vfr_and_nonzero_start_keep_actual_pts_for_initial_and_bridge_frames() {
        let times: Vec<_> = [(1000, 10.), (1001, 10.01), (1040, 10.4), (1150, 11.5)]
            .into_iter()
            .enumerate()
            .map(|(i, (pts, timestamp_seconds))| FrameTime {
                source_frame_index: i as u64,
                pts,
                timestamp_seconds,
            })
            .collect();
        let mut plan = FramePlan {
            selected_frames: vec![
                PlannedFrame {
                    source_frame_index: 0,
                    timestamp_seconds: 0.,
                },
                PlannedFrame {
                    source_frame_index: 2,
                    timestamp_seconds: 0.,
                },
            ],
            candidate_frames: vec![PlannedFrame {
                source_frame_index: 3,
                timestamp_seconds: 0.,
            }],
            ..Default::default()
        };
        let mapped = apply_frame_times(&mut plan, &times).unwrap();
        assert_eq!(plan.selected_frames[1].timestamp_seconds, 10.4);
        assert_eq!(plan.candidate_frames[0].timestamp_seconds, 11.5);
        assert_eq!(mapped[0].pts, 1000);
        assert_eq!(mapped.len(), 3);
        let mut missing = times.clone();
        missing[1].timestamp_seconds = f64::NAN;
        assert!(apply_frame_times(&mut plan, &missing).is_err());
        let mut backwards = times;
        backwards[2].timestamp_seconds = 9.;
        assert!(apply_frame_times(&mut plan, &backwards).is_err());
    }

    fn fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf, PanoramaCheckpoint) {
        let frames = root.join("frames");
        let masks = root.join("masks");
        let sparse = root.join("sparse");
        for p in [&frames, &masks, &sparse] {
            fs::create_dir_all(p).unwrap();
        }
        image::RgbaImage::from_fn(64, 32, |x, y| {
            image::Rgba([(x * 4) as u8, (y * 8) as u8, 30, 255])
        })
        .save(frames.join("frame_0000000000.png"))
        .unwrap();
        image::GrayImage::from_fn(64, 32, |x, _| image::Luma([if x < 32 { 0 } else { 255 }]))
            .save(masks.join("frame_0000000000.png.png"))
            .unwrap();
        fs::write(
            root.join("panorama-frame-times.json"),
            serde_json::to_vec(&vec![FrameTime {
                source_frame_index: 0,
                pts: 100,
                timestamp_seconds: 0.1,
            }])
            .unwrap(),
        )
        .unwrap();
        let mut c = File::create(sparse.join("cameras.bin")).unwrap();
        for b in [
            1u64.to_le_bytes().to_vec(),
            1u32.to_le_bytes().to_vec(),
            17i32.to_le_bytes().to_vec(),
            64u64.to_le_bytes().to_vec(),
            32u64.to_le_bytes().to_vec(),
            64f64.to_le_bytes().to_vec(),
            32f64.to_le_bytes().to_vec(),
        ] {
            c.write_all(&b).unwrap();
        }
        let mut i = File::create(sparse.join("images.bin")).unwrap();
        i.write_all(&1u64.to_le_bytes()).unwrap();
        i.write_all(&99u32.to_le_bytes()).unwrap();
        for v in [1f64, 0., 0., 0., 2., -3., 5.] {
            i.write_all(&v.to_le_bytes()).unwrap();
        }
        i.write_all(&1u32.to_le_bytes()).unwrap();
        i.write_all(b"frame_0000000000.png\0").unwrap();
        i.write_all(&1u64.to_le_bytes()).unwrap();
        i.write_all(&[0; 24]).unwrap();
        let mut p = File::create(sparse.join("points3D.bin")).unwrap();
        p.write_all(&1u64.to_le_bytes()).unwrap();
        p.write_all(&777u64.to_le_bytes()).unwrap();
        for v in [1f64, 2., 3.] {
            p.write_all(&v.to_le_bytes()).unwrap();
        }
        p.write_all(&[20, 40, 60]).unwrap();
        p.write_all(&0.1f64.to_le_bytes()).unwrap();
        p.write_all(&1u64.to_le_bytes()).unwrap();
        p.write_all(&99u32.to_le_bytes()).unwrap();
        p.write_all(&0u32.to_le_bytes()).unwrap();
        let config = PanoramaCheckpoint {
            version: CONVERSION_VERSION,
            source_sha256: "fixture".into(),
            image_sources: None,
            sfm_width: 64,
            sfm_height: 32,
            face_size: 16,
            field_of_view_degrees: FOV_DEGREES,
        };
        (frames, masks, sparse, config)
    }
    #[test]
    fn image_cache_has_no_video_timestamps_and_preserves_source_mapping() {
        let temp = tempfile::tempdir().unwrap();
        let (frames, masks, sparse, mut config) = fixture(temp.path());
        fs::remove_file(temp.path().join("panorama-frame-times.json")).unwrap();
        image::RgbaImage::from_pixel(64, 32, image::Rgba([30, 60, 90, 64]))
            .save(frames.join("frame_0000000000.png"))
            .unwrap();
        config.image_sources = Some(vec![super::super::image_projection::ImageSourceMapping {
            source_image_index: 0,
            source_name: "图片2.jpg".into(),
            source_sha256: "original-sha".into(),
            orientation: 1,
            prepared_name: "frame_0000000000.png".into(),
            original: None,
        }]);
        let out = temp.path().join("training");
        prepare_training_cache(
            &frames,
            &masks,
            &sparse,
            &out,
            &config,
            &tokio_util::sync::CancellationToken::new(),
            |_| {},
        )
        .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
        for face in manifest["faces"].as_array().unwrap() {
            assert!(face.get("pts").is_none() && face.get("timestamp_seconds").is_none());
            assert_eq!(face["source_name"], "图片2.jpg");
            assert_eq!(face["source_image_index"], 0);
            let pixels = image::open(
                out.join("images")
                    .join(face["image_name"].as_str().unwrap()),
            )
            .unwrap()
            .into_rgba8();
            assert!(pixels.pixels().all(|pixel| pixel.0 == [30, 60, 90, 64]));
        }
        assert_eq!(manifest["faces"].as_array().unwrap().len(), 6);
        assert_eq!(
            manifest["config"]["imageSources"][0]["source_sha256"],
            "original-sha"
        );
    }

    #[test]
    fn reshoot_cache_handles_mixed_erp_sizes_and_reuses_faces_without_old_observations() {
        use crate::pipeline::reshoot::PanoramaFrameRecord;
        let temp = tempfile::tempdir().unwrap();
        let (frames, _, sparse, _) = fixture(temp.path());
        image::RgbaImage::from_pixel(128, 64, image::Rgba([90, 60, 30, 64]))
            .save(frames.join("new.png"))
            .unwrap();
        let mut cameras = fs::read(sparse.join("cameras.bin")).unwrap();
        cameras[..8].copy_from_slice(&2u64.to_le_bytes());
        cameras.extend(2u32.to_le_bytes());
        cameras.extend(17i32.to_le_bytes());
        for n in [128u64, 64] {
            cameras.extend(n.to_le_bytes());
        }
        for n in [128f64, 64.] {
            cameras.extend(n.to_le_bytes());
        }
        fs::write(sparse.join("cameras.bin"), cameras).unwrap();
        let mut poses = fs::read(sparse.join("images.bin")).unwrap();
        poses[..8].copy_from_slice(&2u64.to_le_bytes());
        poses.extend(100u32.to_le_bytes());
        for n in [1f64, 0., 0., 0., 2., -3., 5.] {
            poses.extend(n.to_le_bytes());
        }
        poses.extend(2u32.to_le_bytes());
        poses.extend(b"new.png\0");
        poses.extend(0u64.to_le_bytes());
        fs::write(sparse.join("images.bin"), poses).unwrap();
        let records: Vec<_> = ["frame_0000000000.png", "new.png"]
            .iter()
            .enumerate()
            .map(|(i, name)| PanoramaFrameRecord {
                prepared_name: (*name).into(),
                batch_id: format!("batch-{i}"),
                source_name: (*name).into(),
                source_sha256: format!("source-{i}"),
                orientation: Some(1),
                source_index: i as u64,
                timestamp_seconds: None,
                pts: None,
            })
            .collect();
        let token = tokio_util::sync::CancellationToken::new();
        let out = temp.path().join("reshoot-cache");
        let run = || {
            prepare_reshoot_training_cache(
                &frames,
                &sparse,
                &out,
                &records,
                Quality::Fast,
                &token,
                |_| {},
            )
            .unwrap()
        };
        run();
        let derived = out.join("sparse/0");
        assert_eq!(
            crate::reconstruction::colmap_model::read_cameras(&derived)
                .unwrap()
                .len(),
            2
        );
        let poses = crate::reconstruction::colmap_model::read_registered_images(&derived).unwrap();
        assert_eq!(poses.len(), 12);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
        for pose in manifest["faces"].as_array().unwrap() {
            let qvec: [f64; 4] = serde_json::from_value(pose["qvec"].clone()).unwrap();
            let tvec: [f64; 3] = serde_json::from_value(pose["tvec"].clone()).unwrap();
            let center = mul(transpose(rotation(qvec)), tvec).map(|v| -v);
            for (a, b) in center.iter().zip([-2., 3., -5.]) {
                assert!((a - b).abs() < 1e-9);
            }
        }
        let front = out.join("images/frame_0000000001/front.png");
        assert_eq!(image::image_dimensions(&front).unwrap(), (32, 32));
        assert_eq!(
            image::open(&front).unwrap().to_rgba8().get_pixel(16, 16)[3],
            64
        );
        let mtime = front.metadata().unwrap().modified().unwrap();
        run();
        assert_eq!(front.metadata().unwrap().modified().unwrap(), mtime);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["reference_image"], "frame_0000000000/front.png");
        assert!(manifest["faces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["pts"].is_null()));
        assert_eq!(
            fs::metadata(derived.join("points3D.bin")).unwrap().len(),
            59
        );
        token.cancel();
        assert!(matches!(
            prepare_reshoot_training_cache(
                &frames,
                &sparse,
                &out,
                &records,
                Quality::Fast,
                &token,
                |_| {}
            ),
            Err(SplatError::Cancelled)
        ));
    }
    #[test]
    fn training_cache_preserves_world_points_but_replaces_erp_observations_and_tracks() {
        let temp = tempfile::tempdir().unwrap();
        let (frames, masks, sparse, config) = fixture(temp.path());
        let original = fingerprint(&sparse.join("points3D.bin")).unwrap();
        let out = temp.path().join("training");
        prepare_training_cache(
            &frames,
            &masks,
            &sparse,
            &out,
            &config,
            &tokio_util::sync::CancellationToken::new(),
            |_| {},
        )
        .unwrap();
        let derived = out.join("sparse/0");
        let camera = crate::reconstruction::colmap_model::read_single_camera(&derived).unwrap();
        assert_eq!(camera.model, "PINHOLE");
        assert_eq!(camera.params, vec![focal(16), focal(16), 8., 8.]);
        let images = crate::reconstruction::colmap_model::read_registered_images(&derived).unwrap();
        assert_eq!(images.len(), 6);
        assert_eq!(
            images.iter().map(|i| i.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
        let source_points = fs::read(sparse.join("points3D.bin")).unwrap();
        let points = fs::read(derived.join("points3D.bin")).unwrap();
        assert_eq!(points.len(), 59);
        assert_eq!(&points[..51], &source_points[..51]);
        assert_eq!(&points[51..], &0u64.to_le_bytes());
        assert_eq!(fingerprint(&sparse.join("points3D.bin")).unwrap(), original);
        let manifest: CacheManifest =
            serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
        for face in &manifest.faces {
            assert_eq!(face.erp_image_id, 99);
            assert_eq!(face.source_frame_index, 0);
            assert_eq!(face.timestamp_seconds, Some(0.1));
            assert_eq!(face.pts, Some(100));
            let center = mul(transpose(rotation(face.qvec)), face.tvec).map(|v| -v);
            for (a, b) in center.iter().zip([-2., 3., -5.]) {
                assert!((a - b).abs() < 1e-9);
            }
        }
    }
    #[test]
    fn cache_resumes_complete_batches_rebuilds_damaged_faces_and_honors_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let (frames, masks, sparse, config) = fixture(temp.path());
        let out = temp.path().join("training");
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();
        assert!(matches!(
            prepare_training_cache(&frames, &masks, &sparse, &out, &config, &token, |_| {}),
            Err(SplatError::Cancelled)
        ));
        let run = |cfg: &PanoramaCheckpoint| {
            prepare_training_cache(
                &frames,
                &masks,
                &sparse,
                &out,
                cfg,
                &tokio_util::sync::CancellationToken::new(),
                |_| {},
            )
            .unwrap()
        };
        run(&config);
        let front = out.join("images/erp_0000000099/front.png");
        let first = fs::metadata(&front).unwrap().modified().unwrap();
        run(&config);
        assert_eq!(fs::metadata(&front).unwrap().modified().unwrap(), first);
        fs::write(&front, b"broken").unwrap();
        run(&config);
        assert_eq!(image::image_dimensions(&front).unwrap(), (16, 16));
        let mut changed = config.clone();
        changed.face_size = 12;
        run(&changed);
        assert_eq!(image::image_dimensions(front).unwrap(), (12, 12));
    }
    #[test]
    fn incomplete_face_batches_are_cleaned_after_write_failure_or_mid_batch_cancel() {
        for cancel_instead_of_write_failure in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (frames, masks, sparse, config) = fixture(temp.path());
            let source_hash = fingerprint(&frames.join("frame_0000000000.png")).unwrap();
            let model_hash = fingerprint(&sparse.join("points3D.bin")).unwrap();
            let published_model = temp.path().join("final.ply");
            fs::write(&published_model, b"existing result").unwrap();
            let out = temp.path().join("training");
            let token = tokio_util::sync::CancellationToken::new();
            let result =
                prepare_training_cache(&frames, &masks, &sparse, &out, &config, &token, |update| {
                    if matches!(update, TrainingCacheUpdate::Faces { completed: 1, .. }) {
                        if cancel_instead_of_write_failure {
                            token.cancel();
                        } else {
                            // The next PNG write fails deterministically, without
                            // filling the disk or relying on permission fixtures.
                            fs::create_dir(out.join("images/erp_0000000099.partial/back.png"))
                                .unwrap();
                        }
                    }
                });
            assert!(result.is_err());
            if cancel_instead_of_write_failure {
                assert!(matches!(result, Err(SplatError::Cancelled)));
            }
            assert!(!out.join("manifest.json").exists());
            assert!(!out.join("images/erp_0000000099").exists());
            assert!(!out.join("images/erp_0000000099.partial").exists());
            assert_eq!(
                fingerprint(&frames.join("frame_0000000000.png")).unwrap(),
                source_hash
            );
            assert_eq!(
                fingerprint(&sparse.join("points3D.bin")).unwrap(),
                model_hash
            );
            assert_eq!(fs::read(published_model).unwrap(), b"existing result");
            prepare_training_cache(
                &frames,
                &masks,
                &sparse,
                &out,
                &config,
                &tokio_util::sync::CancellationToken::new(),
                |_| {},
            )
            .unwrap();
            assert!(out.join("manifest.json").is_file());
        }
    }

    #[test]
    fn source_fingerprint_and_three_quality_budgets_are_stable() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.mp4");
        fs::write(&source, b"original").unwrap();
        let info = VideoInfo {
            duration: 1.,
            width: 7680,
            height: 3840,
            fps: 30.,
            total_frames: 30,
            codec: "hevc".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        };
        for (q, w, face) in [
            (Quality::Fast, 2048, 512),
            (Quality::Balanced, 3072, 768),
            (Quality::High, 4096, 1024),
        ] {
            let config = PanoramaCheckpoint::new(&source, q, &info).unwrap();
            let native = PanoramaCheckpoint::new_for_policy(
                &source,
                q,
                &info,
                &tokio_util::sync::CancellationToken::new(),
                true,
            )
            .unwrap();
            if q == Quality::High {
                assert_eq!(
                    (native.sfm_width, native.sfm_height, native.face_size),
                    (7680, 3840, 1920)
                );
                assert_eq!(native.field_of_view_degrees, 92.);
            } else {
                assert_eq!(native, config);
            }
            assert_eq!(config.sfm_width, w);
            assert_eq!(config.face_size, face);
            assert_eq!(config.sfm_height, w / 2);
        }
        let old = fingerprint(&source).unwrap();
        fs::write(&source, b"changed").unwrap();
        assert_ne!(fingerprint(&source).unwrap(), old);
    }
    #[test]
    fn cropped_stereo_vr180_and_rotated_sources_are_never_auto_erp() {
        let info = VideoInfo {
            duration: 1.,
            width: 2000,
            height: 1000,
            fps: 30.,
            total_frames: 30,
            codec: "hevc".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        };
        let full = serde_json::json!({"streams":[{"side_data_list":[{"side_data_type":"Spherical Mapping","projection":"equirectangular","yaw":0,"pitch":0,"roll":0}]}]});
        assert!(detect_metadata(&full, &info, true).confirmed_erp);
        for field in ["bound_top", "bound_left"] {
            let mut value = full.clone();
            value["streams"][0]["side_data_list"][0][field] = serde_json::json!(1);
            let result = detect_metadata(&value, &info, true);
            assert!(!result.supported_erp);
            assert!(resolve(VideoProjection::Equirectangular, &result).is_err());
        }
        for field in ["yaw", "pitch", "roll"] {
            let mut oriented = full.clone();
            oriented["streams"][0]["side_data_list"][0][field] = serde_json::json!(90);
            assert!(detect_metadata(&oriented, &info, true).confirmed_erp);
        }
        let mut quarter_turn = info.clone();
        quarter_turn.rotation = 90;
        assert!(!detect_metadata(&full, &quarter_turn, true).supported_erp);
        let mut vr = full.clone();
        vr["streams"][0]["tags"] = serde_json::json!({"projection":"VR180"});
        assert!(!detect_metadata(&vr, &info, true).supported_erp);
        assert!(!detect_metadata(&full, &info, false).confirmed_erp);
    }

    #[test]
    fn ordinary_display_rotation_is_not_an_unsupported_panorama() {
        for mp4 in [false, true] {
            for rotation in [-270, -90, 0, 90, 180, 270] {
                let video = VideoInfo {
                    width: 1920,
                    height: 1080,
                    duration: 1.,
                    fps: 30.,
                    total_frames: 30,
                    codec: "h264".into(),
                    rotation,
                    pixel_format: "yuv420p".into(),
                    has_alpha: false,
                };
                let json = serde_json::json!({"streams":[{"width":1920,"height":1080,"side_data_list":[{"side_data_type":"Display Matrix","rotation":rotation}]}]});
                let detection = detect_metadata(&json, &video, mp4);
                assert!(!detection.unsupported, "rotation={rotation}, mp4={mp4}");
                assert!(
                    !detection.supported_erp
                        && !detection.confirmed_erp
                        && !detection.requires_choice
                );
                assert!(!resolve(VideoProjection::Auto, &detection).unwrap());
                assert!(resolve(VideoProjection::Equirectangular, &detection).is_err());
                let size = crate::video::prepared_video_dimensions(&video);
                assert_eq!(
                    size,
                    if rotation.rem_euclid(180) == 90 {
                        (1080, 1920)
                    } else {
                        (1920, 1080)
                    }
                );
            }
        }
    }

    #[test]
    fn rotated_ambiguous_or_explicit_panorama_stays_blocked_without_rejecting_perspective() {
        let video = VideoInfo {
            width: 2000,
            height: 1000,
            duration: 1.,
            fps: 30.,
            total_frames: 30,
            codec: "h264".into(),
            rotation: -90,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        };
        let unknown = serde_json::json!({"streams":[{"side_data_list":[{"side_data_type":"Display Matrix","rotation":-90}]}]});
        let detection = detect_metadata(&unknown, &video, true);
        assert!(detection.unsupported && detection.requires_choice);
        assert!(!detection.supported_erp && !detection.confirmed_erp);
        assert!(resolve(VideoProjection::Equirectangular, &detection).is_err());
        assert!(!resolve(VideoProjection::Perspective, &detection).unwrap());
        let spherical = serde_json::json!({"streams":[{"side_data_list":[{"side_data_type":"Spherical Mapping","projection":"equirectangular"}]}]});
        for mp4 in [false, true] {
            let detection = detect_metadata(&spherical, &video, mp4);
            assert!(detection.unsupported && !detection.supported_erp && !detection.confirmed_erp);
        }
        let stereo = serde_json::json!({"streams":[{"side_data_list":[{"side_data_type":"Stereo 3D","type":"side by side"}]}]});
        assert!(detect_metadata(&stereo, &video, false).unsupported);
    }

    #[tokio::test]
    #[ignore = "Read-only IMG_8023.MOV regression; requires the local phone video and bundled FFprobe"]
    async fn native_ordinary_rotated_mov_is_not_a_panorama_error() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let input = Path::new("E:/Download/LocalSend/IMG_8023.MOV");
        let before = input.metadata().unwrap();
        let (video, json) = crate::engines::ffprobe::probe_video_metadata(
            &repo.join("engines/ffmpeg/ffprobe.exe"),
            input,
            None,
            &ProcessManager::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            (video.width, video.height, video.rotation),
            (1920, 1080, -90)
        );
        let detection = detect_metadata(&json, &video, false);
        assert!(
            !detection.unsupported
                && !detection.confirmed_erp
                && !detection.requires_choice
                && !detection.supported_erp
        );
        assert!(!resolve(VideoProjection::Auto, &detection).unwrap());
        assert_eq!(
            crate::video::prepared_video_dimensions(&video),
            (1080, 1920)
        );
        let after = input.metadata().unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }
    #[test]
    fn face_centers_cover_both_hemispheres_and_poles() {
        let uv = FACES.map(|(_, q)| erp_pixel(q, 50, 50, 101, 2000, 1000));
        assert!((uv[0].0 - 999.5).abs() < 1e-8);
        assert!((uv[1].0 - 1999.5).abs() < 1e-8);
        assert!((uv[2].0 - 499.5).abs() < 1e-8);
        assert!((uv[3].0 - 1499.5).abs() < 1e-8);
        assert_eq!(uv[4].1, 0.);
        assert_eq!(uv[5].1, 999.);
    }
    #[test]
    fn all_face_poses_keep_center_and_round_trip_rotation() {
        let r = rotation([0.5, 0.5, 0.5, 0.5]);
        let t = [2., -3., 5.];
        let center = mul(transpose(r), t).map(|v| -v);
        for (_, q) in FACES {
            let rf = product(q, r);
            let tf = mul(q, t);
            let recovered = mul(transpose(rf), tf).map(|v| -v);
            for i in 0..3 {
                assert!((recovered[i] - center[i]).abs() < 1e-10);
            }
            let qr = rotation(quaternion(rf));
            for i in 0..3 {
                for j in 0..3 {
                    assert!((rf[i][j] - qr[i][j]).abs() < 1e-10);
                }
            }
        }
    }
    #[test]
    fn seam_wrap_and_mask_share_projection() {
        let source = image::RgbaImage::from_pixel(8, 4, image::Rgba([20, 40, 60, 255]));
        let mask =
            image::GrayImage::from_fn(8, 4, |x, _| image::Luma([if x < 4 { 0 } else { 255 }]));
        let out = render_face(
            &source,
            Some(&mask),
            FACES[1].1,
            9,
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(out.get_pixel(0, 4)[3], 255);
        assert_eq!(out.get_pixel(8, 4)[3], 0);
        assert_eq!(out.get_pixel(0, 4)[0], 20);
    }
    #[test]
    fn ambiguous_ratio_never_silently_becomes_panorama() {
        let video = VideoInfo {
            width: 2000,
            height: 1000,
            duration: 1.,
            fps: 30.,
            total_frames: 30,
            codec: "h264".into(),
            rotation: 0,
            pixel_format: "yuv420p".into(),
            has_alpha: false,
        };
        let d = detect_metadata(&serde_json::json!({"streams":[{}]}), &video, true);
        assert!(resolve(VideoProjection::Auto, &d).is_err());
        assert!(resolve(VideoProjection::Equirectangular, &d).unwrap());
        assert!(!resolve(VideoProjection::Perspective, &d).unwrap());
        let stereo = detect_metadata(
            &serde_json::json!({"streams":[{"side_data_list":[{"side_data_type":"Stereo 3D","type":"side by side"}]}]}),
            &video,
            true,
        );
        assert!(!stereo.supported_erp);
        assert!(resolve(VideoProjection::Equirectangular, &stereo).is_err());
    }
    #[test]
    fn source_identity_is_stable_and_changes_with_size_mtime_or_path() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("视频.mp4");
        fs::write(&source, b"first").unwrap();
        let first = source_identity(&source).unwrap();
        assert_eq!(source_identity(&source).unwrap(), first);
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
            ))
            .unwrap();
        assert_ne!(source_identity(&source).unwrap(), first);
        let first = source_identity(&source).unwrap();
        fs::write(&source, b"changed-input").unwrap();
        assert_ne!(source_identity(&source).unwrap(), first);
        let moved = root.path().join("moved.mp4");
        fs::rename(&source, &moved).unwrap();
        assert_ne!(source_identity(&moved).unwrap(), first);
        assert!(source_identity(&source).is_err());
    }
}
