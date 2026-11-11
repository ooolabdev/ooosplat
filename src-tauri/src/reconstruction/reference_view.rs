//! Read-only preview cameras from the dataset actually passed to the pinned Brush loader.
//! Do not infer a camera from an imported model's unrelated COLMAP directory.
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};

use crate::error::{Result, SplatError};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainingReferenceView {
    pub image_name: String,
    pub position: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
    pub fov: f64,
    pub projection_offset: [f64; 2],
    pub projection_scale_x: f64,
}

#[derive(Clone)]
struct Camera {
    width: u64,
    height: u64,
    model: i32,
    params: Vec<f64>,
}

fn invalid() -> SplatError {
    SplatError::Process("Invalid training reference camera data".into())
}

fn number<const N: usize>(reader: &mut impl Read) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn bounded_file(path: &Path, maximum: u64) -> Result<File> {
    let file = File::open(path)?;
    if file.metadata()?.len() > maximum {
        return Err(invalid());
    }
    Ok(file)
}

fn line(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                String::from_utf8(bytes).map(Some).map_err(|_| invalid())
            };
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        if bytes.len() + count > 64 * 1024 {
            return Err(invalid());
        }
        let end = available[count - 1] == b'\n';
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if end {
            return String::from_utf8(bytes).map(Some).map_err(|_| invalid());
        }
    }
}

fn parse<T: std::str::FromStr>(value: &str) -> Result<T> {
    value.parse().map_err(|_| invalid())
}

fn cameras(model: &Path) -> Result<HashMap<u32, Camera>> {
    let mut result = HashMap::new();
    let binary = model.join("cameras.bin");
    if binary.is_file() {
        let mut reader = BufReader::new(bounded_file(&binary, 8 * 1024 * 1024)?);
        let count = u64::from_le_bytes(number(&mut reader)?);
        if count > 100_000 {
            return Err(invalid());
        }
        for _ in 0..count {
            let id = u32::from_le_bytes(number(&mut reader)?);
            let model = i32::from_le_bytes(number(&mut reader)?);
            let width = u64::from_le_bytes(number(&mut reader)?);
            let height = u64::from_le_bytes(number(&mut reader)?);
            let n = super::colmap_model::camera_model(model)
                .ok_or_else(invalid)?
                .1;
            let params = (0..n)
                .map(|_| number(&mut reader).map(f64::from_le_bytes))
                .collect::<Result<Vec<_>>>()?;
            if result
                .insert(
                    id,
                    Camera {
                        width,
                        height,
                        model,
                        params,
                    },
                )
                .is_some()
            {
                return Err(invalid());
            }
        }
    } else {
        let mut reader = BufReader::new(bounded_file(&model.join("cameras.txt"), 8 * 1024 * 1024)?);
        while let Some(row) = line(&mut reader)? {
            if row.trim().is_empty() || row.trim_start().starts_with('#') {
                continue;
            }
            let fields = row.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 5 {
                return Err(invalid());
            }
            let model = (0..=17)
                .find(|id| super::colmap_model::camera_model(*id).is_some_and(|m| m.0 == fields[1]))
                .ok_or_else(invalid)?;
            let n = super::colmap_model::camera_model(model)
                .ok_or_else(invalid)?
                .1 as usize;
            if fields.len() != n + 4 {
                return Err(invalid());
            }
            let id = parse(fields[0])?;
            let camera = Camera {
                width: parse(fields[2])?,
                height: parse(fields[3])?,
                model,
                params: fields[4..]
                    .iter()
                    .map(|s| parse(s))
                    .collect::<Result<Vec<_>>>()?,
            };
            if result.insert(id, camera).is_some() {
                return Err(invalid());
            }
        }
    }
    Ok(result)
}

/// Matches focal_to_fov/with_pinhole in bundled Brush 866a7d65. This is a
/// pinhole preview of that field of view, not lens undistortion of an image.
fn reference_fov(focal: f64, pixels: u64, radial: [f64; 6]) -> Option<f64> {
    let target = pixels as f64 / (2.0 * focal);
    let mut radius = target;
    for _ in 0..30 {
        let r2 = radius * radius;
        let r4 = r2 * r2;
        let r6 = r4 * r2;
        let factor = (1.0 + radial[0] * r2 + radial[1] * r4 + radial[2] * r6)
            / (1.0 + radial[3] * r2 + radial[4] * r4 + radial[5] * r6);
        if !factor.is_finite() || factor.abs() < 1e-12 {
            return None;
        }
        let next = target / factor;
        if !next.is_finite() || next <= 0.0 {
            return None;
        }
        let converged = (next - radius).abs() < 1e-12;
        radius = next;
        if converged {
            break;
        }
    }
    // Brush's fixed-point loop can fail to converge on non-physical fits.
    // Do not publish an unverifiable locator camera for those inputs.
    let r2 = radius * radius;
    let factor = (1.0 + radial[0] * r2 + radial[1] * r2 * r2 + radial[2] * r2 * r2 * r2)
        / (1.0 + radial[3] * r2 + radial[4] * r2 * r2 + radial[5] * r2 * r2 * r2);
    if !(radius * factor).is_finite() || (radius * factor - target).abs() > 1e-6 * target.max(1.0) {
        return None;
    }
    Some(2.0 * radius.atan())
}

fn view(name: String, q: [f64; 4], t: [f64; 3], camera: &Camera) -> Option<TrainingReferenceView> {
    if !matches!(camera.model, 0 | 1 | 2 | 3 | 4 | 6)
        || camera.params.len() != super::colmap_model::camera_model(camera.model)?.1 as usize
        || camera.width == 0
        || camera.height == 0
        || q.iter()
            .chain(t.iter())
            .chain(camera.params.iter())
            .any(|v| !v.is_finite())
    {
        return None;
    }
    let (fx, fy, cx, cy) = if [0, 2, 3, 8, 9].contains(&camera.model) {
        (
            camera.params[0],
            camera.params[0],
            camera.params[1],
            camera.params[2],
        )
    } else {
        (
            camera.params[0],
            camera.params[1],
            camera.params[2],
            camera.params[3],
        )
    };
    let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !norm.is_finite() || norm < 1e-8 || fx <= 0.0 || fy <= 0.0 {
        return None;
    }
    let p = &camera.params;
    let radial = match camera.model {
        2 => [p[3], 0., 0., 0., 0., 0.],
        3 => [p[3], p[4], 0., 0., 0., 0.],
        4 => [p[4], p[5], 0., 0., 0., 0.],
        6 => [p[4], p[5], p[8], p[9], p[10], p[11]],
        _ => [0.; 6],
    }
    .map(|value| (value as f32) as f64);
    let fov_x = reference_fov(fx, camera.width, radial)?;
    let fov_y = reference_fov(fy, camera.height, radial)?;
    let virtual_fx = camera.width as f64 / (2.0 * (fov_x / 2.0).tan());
    let virtual_fy = camera.height as f64 / (2.0 * (fov_y / 2.0).tan());
    let [w, x, y, z] = q.map(|v| v / norm);
    let r = [
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
    ];
    let position = std::array::from_fn(|i| -(0..3).map(|j| r[j][i] * t[j]).sum::<f64>());
    let h = camera.height as f64;
    let view = TrainingReferenceView {
        image_name: name,
        position,
        forward: r[2],
        up: r[1].map(|v| -v),
        fov: fov_y.to_degrees(),
        projection_offset: [(cx - camera.width as f64 / 2.) / h, (cy - h / 2.) / h],
        projection_scale_x: virtual_fx / virtual_fy,
    };
    if !view
        .position
        .iter()
        .chain(view.projection_offset.iter())
        .all(|v| v.is_finite() && v.abs() < 1e15)
        || !view.projection_scale_x.is_finite()
        || view.projection_scale_x <= 0.0
        || !(0.0 < view.fov && view.fov < 180.0)
    {
        return None;
    }
    Some(view)
}

fn safe_image(dataset: &Path, name: &str) -> bool {
    let path = Path::new(name);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
        || name.contains('\\')
        || name.is_empty()
    {
        return false;
    }
    let images = dataset.join("images");
    match (
        std::fs::canonicalize(&images),
        std::fs::canonicalize(images.join(path)),
    ) {
        (Ok(root), Ok(file)) => file.starts_with(root) && file.is_file(),
        _ => false,
    }
}

fn read_view(dataset: &Path, wanted: Option<&str>) -> Result<TrainingReferenceView> {
    let model = dataset.join("sparse/0");
    let cameras = cameras(&model)?;
    let mut best: Option<TrainingReferenceView> = None;
    let mut consider = |name: String, camera_id, q, t| {
        if wanted.is_some_and(|wanted| wanted != name)
            || best.as_ref().is_some_and(|v| v.image_name <= name)
            || !safe_image(dataset, &name)
        {
            return;
        }
        if let Some(candidate) = cameras
            .get(&camera_id)
            .and_then(|camera| view(name, q, t, camera))
        {
            best = Some(candidate);
        }
    };
    let binary = model.join("images.bin");
    if binary.is_file() {
        // Large reconstructions can have gigabytes of observations. Only pose headers
        // are decoded; seek over observations without buffering the complete file.
        let file = bounded_file(&binary, 64 * 1024 * 1024 * 1024)?;
        let len = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        let count = u64::from_le_bytes(number(&mut reader)?);
        if count > 1_000_000 {
            return Err(invalid());
        }
        for _ in 0..count {
            let _id = u32::from_le_bytes(number(&mut reader)?);
            let mut q = [0.; 4];
            let mut t = [0.; 3];
            for v in q.iter_mut().chain(t.iter_mut()) {
                *v = f64::from_le_bytes(number(&mut reader)?);
            }
            let camera = u32::from_le_bytes(number(&mut reader)?);
            let mut name = Vec::new();
            loop {
                let [byte] = number(&mut reader)?;
                if byte == 0 {
                    break;
                }
                name.push(byte);
                if name.len() > 32 * 1024 {
                    return Err(invalid());
                }
            }
            let points = u64::from_le_bytes(number(&mut reader)?);
            let bytes = points.checked_mul(24).ok_or_else(invalid)?;
            let end = reader
                .stream_position()?
                .checked_add(bytes)
                .filter(|end| *end <= len)
                .ok_or_else(invalid)?;
            reader.seek(SeekFrom::Start(end))?;
            consider(
                String::from_utf8(name).map_err(|_| invalid())?,
                camera,
                q,
                t,
            );
        }
    } else {
        let mut reader = BufReader::new(bounded_file(
            &model.join("images.txt"),
            8 * 1024 * 1024 * 1024,
        )?);
        while let Some(row) = line(&mut reader)? {
            if row.trim().is_empty() || row.trim_start().starts_with('#') {
                continue;
            }
            let fields = row.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 10 {
                return Err(invalid());
            }
            let mut q = [0.; 4];
            let mut t = [0.; 3];
            for (v, text) in q.iter_mut().chain(t.iter_mut()).zip(&fields[1..8]) {
                *v = parse(text)?;
            }
            consider(fields[9..].join(" "), parse(fields[8])?, q, t);
            // Observation rows can be huge: discard them without allocating a String.
            reader.skip_until(b'\n')?;
        }
    }
    best.ok_or_else(invalid)
}

#[derive(Deserialize)]
struct PanoramaManifest {
    #[serde(default)]
    reference_image: Option<String>,
    #[serde(default)]
    config: PanoramaSource,
    faces: Vec<PanoramaFace>,
}
#[derive(Default, Deserialize)]
struct PanoramaSource {
    #[serde(default, rename = "imageSources")]
    image_sources: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct PanoramaFace {
    #[serde(default)]
    timestamp_seconds: Option<f64>,
    source_frame_index: u64,
    #[serde(default)]
    source_image_index: Option<u64>,
    face: String,
    image_name: String,
}

fn load(root: &Path) -> Result<TrainingReferenceView> {
    if root.join("state.json").is_file() {
        let state: serde_json::Value =
            serde_json::from_reader(bounded_file(&root.join("state.json"), 16 * 1024 * 1024)?)?;
        if let Some(reference) = state
            .pointer("/inputs/referenceImage")
            .and_then(|v| v.as_str())
        {
            return read_view(&root.join("work/brush/dataset"), Some(reference));
        }
    }
    // Projection is persisted in state.json, never re-detected from mutable source media.
    let panorama = if root.join("state.json").is_file() {
        let state: serde_json::Value =
            serde_json::from_reader(bounded_file(&root.join("state.json"), 16 * 1024 * 1024)?)?;
        state.get("videoProjection").and_then(|v| v.as_str()) == Some("equirectangular")
    } else {
        false
    };
    if panorama {
        let dataset = root.join("work/panorama-training");
        let manifest: PanoramaManifest = serde_json::from_reader(bounded_file(
            &dataset.join("manifest.json"),
            32 * 1024 * 1024,
        )?)?;
        if let Some(reference) = &manifest.reference_image {
            if !manifest
                .faces
                .iter()
                .any(|f| f.face == "front" && &f.image_name == reference)
            {
                return Err(invalid());
            }
            return read_view(&dataset, Some(reference));
        }
        let images = manifest.config.image_sources.is_some();
        let first = manifest
            .faces
            .iter()
            .filter(|f| {
                f.face == "front"
                    && if images {
                        f.source_image_index.is_some()
                    } else {
                        f.timestamp_seconds.is_some_and(|v| v.is_finite())
                    }
            })
            .min_by(|a, b| {
                if images {
                    a.source_image_index.cmp(&b.source_image_index)
                } else {
                    a.timestamp_seconds
                        .unwrap_or(0.)
                        .total_cmp(&b.timestamp_seconds.unwrap_or(0.))
                        .then(a.source_frame_index.cmp(&b.source_frame_index))
                }
            })
            .ok_or_else(invalid)?;
        read_view(&dataset, Some(&first.image_name))
    } else {
        // OOOSplat passes no frame subsampling/eval split overrides to Brush. The pinned
        // loader sorts registered image names lexicographically and skips missing images.
        read_view(&root.join("work/brush/dataset"), None)
    }
}

pub fn training_reference_view(
    root: &Path,
    imported: bool,
) -> (Option<TrainingReferenceView>, Option<&'static str>) {
    if imported {
        return (None, Some("imported"));
    }
    match load(root) {
        Ok(view) => (Some(view), None),
        Err(_) => (None, Some("unavailable")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn radial_reference_matches_pinned_brush_pinhole_projection() {
        // Fixed output from Brush 866a7d65 focal_to_fov (RT8, k1 as f32).
        let camera = Camera {
            width: 800,
            height: 600,
            model: 2,
            params: vec![400., 400., 300., 0.1],
        };
        let v = view("first.png".into(), [1., 0., 0., 0.], [0.; 3], &camera).unwrap();
        assert!((v.fov - 71.0274).abs() < 0.0001);
        assert!((v.projection_scale_x - 1.03237).abs() < 0.00001);
        for model in [3, 4, 6] {
            let params = match model {
                3 => vec![400., 400., 300., 0.1, 0.],
                4 => vec![400., 400., 400., 300., 0.1, 0., 0.01, -0.02],
                _ => vec![400., 400., 400., 300., 0.1, 0., 0.01, -0.02, 0., 0., 0., 0.],
            };
            let other = view(
                "first.png".into(),
                [1., 0., 0., 0.],
                [0.; 3],
                &Camera {
                    model,
                    params,
                    ..camera.clone()
                },
            )
            .unwrap();
            assert_eq!(other.fov, v.fov);
            assert_eq!(other.projection_scale_x, v.projection_scale_x);
        }
    }

    #[test]
    fn zero_radial_is_unchanged_and_unsafe_solutions_are_unavailable() {
        let zero = Camera {
            width: 800,
            height: 600,
            model: 4,
            params: vec![400., 300., 450., 250., 0., 0., 0., 0.],
        };
        let v = view("a.png".into(), [1., 0., 0., 0.], [0.; 3], &zero).unwrap();
        assert!((v.fov - 90.).abs() < 1e-10);
        assert!((v.projection_scale_x - 4. / 3.).abs() < 1e-10);
        assert_eq!(v.projection_offset, [50. / 600., -50. / 600.]);
        let mut bad = zero;
        bad.params[4] = f64::INFINITY;
        assert!(view("a.png".into(), [1., 0., 0., 0.], [0.; 3], &bad).is_none());
        assert!(reference_fov(400., 800, [-1., 0., 0., 0., 0., 0.]).is_none());
        assert!(reference_fov(400., 800, [-0.01, 0., 0., 0., 0., 0.]).is_some());
    }
    fn dataset(root: &Path, subdir: &str) -> std::path::PathBuf {
        let d = root.join(subdir);
        std::fs::create_dir_all(d.join("sparse/0")).unwrap();
        std::fs::create_dir_all(d.join("images")).unwrap();
        std::fs::write(
            d.join("sparse/0/cameras.txt"),
            "1 PINHOLE 800 600 400 300 450 250\n",
        )
        .unwrap();
        d
    }
    #[test]
    fn text_loader_order_pose_and_intrinsics() {
        let root = tempfile::tempdir().unwrap();
        let d = dataset(root.path(), "work/brush/dataset");
        std::fs::write(d.join("sparse/0/images.txt"), "# images\n9 1 0 0 0 0 0 0 1 z.png\n\n2 1 0 0 0 -1 -2 -3 1 a.png\n\n1 1 0 0 0 0 0 0 1 0-missing.png\n\n").unwrap();
        for name in ["a.png", "z.png"] {
            std::fs::write(d.join("images").join(name), b"image").unwrap();
        }
        let (v, reason) = training_reference_view(root.path(), false);
        assert!(reason.is_none());
        let v = v.unwrap();
        assert_eq!(v.image_name, "a.png");
        assert_eq!(v.position, [1., 2., 3.]);
        assert_eq!(v.forward, [0., 0., 1.]);
        assert_eq!(v.up, [-0., -1., -0.]);
        assert_eq!(v.fov, 90.);
        assert_eq!(v.projection_offset, [50. / 600., -50. / 600.]);
        assert_eq!(v.projection_scale_x, 4. / 3.);
        assert!(training_reference_view(root.path(), true).0.is_none());
    }
    #[test]
    fn panorama_selects_earliest_front_from_mapping_not_alphabetical_back() {
        let root = tempfile::tempdir().unwrap();
        let d = dataset(root.path(), "work/panorama-training");
        std::fs::write(
            root.path().join("state.json"),
            r#"{"videoProjection":"equirectangular"}"#,
        )
        .unwrap();
        std::fs::write(d.join("manifest.json"), r#"{"faces":[{"timestamp_seconds":2,"source_frame_index":1,"face":"front","image_name":"a.png"},{"timestamp_seconds":1,"source_frame_index":0,"face":"front","image_name":"z.png"},{"timestamp_seconds":0,"source_frame_index":0,"face":"back","image_name":"back.png"}]}"#).unwrap();
        std::fs::write(
            d.join("sparse/0/images.txt"),
            "1 1 0 0 0 0 0 0 1 back.png\n\n2 1 0 0 0 0 0 0 1 a.png\n\n3 1 0 0 0 0 0 0 1 z.png\n\n",
        )
        .unwrap();
        for name in ["a.png", "z.png", "back.png"] {
            std::fs::write(d.join("images").join(name), b"image").unwrap();
        }
        assert_eq!(
            training_reference_view(root.path(), false)
                .0
                .unwrap()
                .image_name,
            "z.png"
        );
        // Still images have no PTS or invented capture time; natural source order is authoritative.
        std::fs::write(d.join("manifest.json"),r#"{"config":{"imageSources":[]},"faces":[{"source_frame_index":10,"source_image_index":10,"face":"front","image_name":"a.png"},{"source_frame_index":2,"source_image_index":2,"face":"front","image_name":"z.png"},{"source_frame_index":0,"source_image_index":0,"face":"back","image_name":"back.png"}]}"#).unwrap();
        assert_eq!(
            training_reference_view(root.path(), false)
                .0
                .unwrap()
                .image_name,
            "z.png"
        );
    }
    #[test]
    fn binary_pose_rotation_and_truncation_are_checked() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let d = dataset(root.path(), "work/brush/dataset");
        let mut cameras = File::create(d.join("sparse/0/cameras.bin")).unwrap();
        cameras.write_all(&1u64.to_le_bytes()).unwrap();
        cameras.write_all(&1u32.to_le_bytes()).unwrap();
        cameras.write_all(&1i32.to_le_bytes()).unwrap();
        for dimension in [800u64, 600] {
            cameras.write_all(&dimension.to_le_bytes()).unwrap();
        }
        for parameter in [400f64, 300., 450., 250.] {
            cameras.write_all(&parameter.to_le_bytes()).unwrap();
        }
        cameras.flush().unwrap();
        std::fs::write(d.join("images/a.png"), b"image").unwrap();
        let mut images = File::create(d.join("sparse/0/images.bin")).unwrap();
        images.write_all(&1u64.to_le_bytes()).unwrap();
        images.write_all(&1u32.to_le_bytes()).unwrap();
        let s = 0.5f64.sqrt();
        for v in [s, 0., s, 0., 1., 2., 3.] {
            images.write_all(&v.to_le_bytes()).unwrap();
        }
        images.write_all(&1u32.to_le_bytes()).unwrap();
        images.write_all(b"a.png\0").unwrap();
        images.write_all(&0u64.to_le_bytes()).unwrap();
        images.flush().unwrap();
        let v = training_reference_view(root.path(), false).0.unwrap();
        assert!((v.position[0] - 3.).abs() < 1e-8);
        assert!((v.position[1] + 2.).abs() < 1e-8);
        assert!((v.position[2] + 1.).abs() < 1e-8);
        assert!((v.forward[0] + 1.).abs() < 1e-8);
        images.set_len(20).unwrap();
        assert!(training_reference_view(root.path(), false).0.is_none());
    }
    #[test]
    fn missing_corrupt_and_escaped_data_do_not_block_preview() {
        let root = tempfile::tempdir().unwrap();
        assert!(training_reference_view(root.path(), false).0.is_none());
        let d = dataset(root.path(), "work/brush/dataset");
        std::fs::write(
            d.join("sparse/0/images.txt"),
            "1 1 0 0 0 0 0 0 1 ../../escape.png\n\n",
        )
        .unwrap();
        assert!(training_reference_view(root.path(), false).0.is_none());
        std::fs::write(d.join("sparse/0/cameras.txt"), "broken").unwrap();
        assert!(training_reference_view(root.path(), false).0.is_none());
    }

    #[test]
    fn invalid_camera_and_pose_are_not_treated_as_a_reference() {
        let camera = Camera {
            width: 800,
            height: 600,
            model: 1,
            params: vec![400., 300., 400., 300.],
        };
        assert!(view("bad".into(), [0.; 4], [0.; 3], &camera).is_none());
        assert!(view("bad".into(), [f64::NAN, 0., 0., 0.], [0.; 3], &camera).is_none());
        let mut bad = camera.clone();
        bad.params[0] = 0.;
        assert!(view("bad".into(), [1., 0., 0., 0.], [0.; 3], &bad).is_none());
        bad.model = 17;
        bad.params = vec![0., 0.];
        assert!(view("erp".into(), [1., 0., 0., 0.], [0.; 3], &bad).is_none());
    }

    #[test]
    #[ignore = "Requires OOOSPLAT_REFERENCE_PROJECT containing a real Brush training dataset"]
    fn actual_training_reference_view() {
        let root =
            std::env::var_os("OOOSPLAT_REFERENCE_PROJECT").expect("Set a real completed project");
        let (view, reason) = training_reference_view(Path::new(&root), false);
        let view = view.unwrap_or_else(|| panic!("No reference: {reason:?}"));
        let json = serde_json::to_string(&view).unwrap();
        println!("TRAINING_REFERENCE_JSON={json}");
        if let Some(artifact) = std::env::var_os("OOOSPLAT_REFERENCE_ARTIFACT") {
            std::fs::write(artifact, json).unwrap();
        }
    }
}
