//! Streaming, non-destructive transform export for Brush Gaussian PLY files.
//!
//! SH rotation follows the MIT-licensed PlayCanvas splat-transform implementation.

use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use uuid::Uuid;

use crate::{
    error::{Result, SplatError},
    project::{GaussianCrop, GaussianTransform},
    reconstruction::ply::{inspect_gaussian_ply, PlyInfo},
};

const MAX_HEADER_BYTES: usize = 1024 * 1024;
const ROWS_PER_CHUNK: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvalidGaussianKind {
    ZeroPoints,
    NonFiniteAttributes,
    InvalidRotation,
}

/// Only a fully read, structurally complete output may establish an algorithm
/// failure. Missing/truncated files, unsupported formats and I/O are not one.
#[derive(Debug)]
pub(crate) enum TrainingPlyValidationError {
    Operational(SplatError),
    InvalidGaussian(InvalidGaussianKind),
}

impl TrainingPlyValidationError {
    pub(crate) fn is_algorithm_failure(&self) -> bool {
        matches!(self, Self::InvalidGaussian(_))
    }

    pub(crate) fn into_error(self) -> SplatError {
        match self {
            Self::Operational(error) => error,
            Self::InvalidGaussian(kind) => unsupported(match kind {
                InvalidGaussianKind::ZeroPoints => "Gaussian output contains zero points",
                InvalidGaussianKind::NonFiniteAttributes => "Non-finite Gaussian attributes",
                InvalidGaussianKind::InvalidRotation => "Invalid Gaussian rotation",
            }),
        }
    }
}

impl From<SplatError> for TrainingPlyValidationError {
    fn from(error: SplatError) -> Self {
        Self::Operational(error)
    }
}

impl From<std::io::Error> for TrainingPlyValidationError {
    fn from(error: std::io::Error) -> Self {
        Self::Operational(SplatError::Io(error))
    }
}

/// Full bounded read, used for reshoot initialization and publication validation.
/// Unlike header inspection this rejects truncated/non-finite Gaussian payloads.
pub fn validate_training_ply(
    path: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(PlyInfo, u32, String)> {
    validate_training_ply_classified(path, cancel).map_err(TrainingPlyValidationError::into_error)
}

pub(crate) fn validate_training_ply_classified(
    path: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> std::result::Result<(PlyInfo, u32, String), TrainingPlyValidationError> {
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled.into());
    }
    let file = File::open(path)?;
    let info = file.metadata()?;
    let mut reader = BufReader::new(file);
    let layout = parse_layout_with_empty(&mut reader, true)?;
    let expected = (layout.header.len() as u64)
        .checked_add(
            layout
                .count
                .checked_mul(layout.stride as u64)
                .ok_or_else(|| unsupported("PLY size overflow"))?,
        )
        .ok_or_else(|| unsupported("PLY size overflow"))?;
    if info.len() != expected {
        return Err(unsupported("PLY data length mismatch").into());
    }
    validate_training_payload(&mut reader, layout, info.len(), cancel)
}

fn validate_training_payload(
    reader: &mut impl Read,
    layout: PlyLayout,
    file_size: u64,
    cancel: &tokio_util::sync::CancellationToken,
) -> std::result::Result<(PlyInfo, u32, String), TrainingPlyValidationError> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(&layout.header);
    let mut row = vec![0; layout.stride];
    let mut invalid = (layout.count == 0).then_some(InvalidGaussianKind::ZeroPoints);
    let rotation_offsets: Vec<_> = (0..4)
        .map(|i| layout.offsets[&format!("rot_{i}")])
        .collect();
    for _ in 0..layout.count {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled.into());
        }
        reader.read_exact(&mut row)?;
        if row
            .as_chunks::<4>()
            .0
            .iter()
            .any(|b| !f32::from_le_bytes(*b).is_finite())
        {
            invalid = Some(InvalidGaussianKind::NonFiniteAttributes);
        }
        let norm: f64 = rotation_offsets
            .iter()
            .map(|offset| read_float(&row, *offset).powi(2))
            .sum();
        if norm < 1e-20 && invalid.is_none() {
            invalid = Some(InvalidGaussianKind::InvalidRotation);
        }
        hash.update(&row);
    }
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled.into());
    }
    // A read failure/truncation after an earlier NaN must still stop without
    // consuming an algorithm fallback. Also reject files that grew mid-read.
    if reader.read(&mut [0_u8; 1])? != 0 {
        return Err(unsupported("PLY data length mismatch").into());
    }
    if let Some(kind) = invalid {
        return Err(TrainingPlyValidationError::InvalidGaussian(kind));
    }
    let sh = match layout.rest_count {
        0 => 0,
        9 => 1,
        24 => 2,
        _ => 3,
    };
    Ok((
        PlyInfo {
            file_size,
            splat_count: layout.count,
        },
        sh,
        format!("{:x}", hash.finalize()),
    ))
}
pub mod export_view;

#[cfg(windows)]
fn publish_export(source: &Path, destination: &Path) -> Result<()> {
    use std::{iter, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: Both paths are owned, NUL-terminated UTF-16 buffers that remain live for the call.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn publish_export(source: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(source, destination)?;
    Ok(())
}

#[derive(Debug)]
struct PlyLayout {
    header: Vec<u8>,
    count: u64,
    stride: usize,
    offsets: HashMap<String, usize>,
    rest_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct Quaternion {
    x: f64,
    y: f64,
    z: f64,
    w: f64,
}

impl Quaternion {
    fn from_euler_degrees([ex, ey, ez]: [f64; 3]) -> Self {
        let half_to_rad = 0.5 * std::f64::consts::PI / 180.0;
        let (sx, cx) = (ex * half_to_rad).sin_cos();
        let (sy, cy) = (ey * half_to_rad).sin_cos();
        let (sz, cz) = (ez * half_to_rad).sin_cos();
        Self {
            x: sx * cy * cz - cx * sy * sz,
            y: cx * sy * cz + sx * cy * sz,
            z: cx * cy * sz - sx * sy * cz,
            w: cx * cy * cz + sx * sy * sz,
        }
        .normalized()
    }

    fn normalized(self) -> Self {
        let length = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if length <= f64::EPSILON || !length.is_finite() {
            Self {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            }
        } else {
            Self {
                x: self.x / length,
                y: self.y / length,
                z: self.z / length,
                w: self.w / length,
            }
        }
    }

    fn mul(self, rhs: Self) -> Self {
        Self {
            x: self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            y: self.w * rhs.y + self.y * rhs.w + self.z * rhs.x - self.x * rhs.z,
            z: self.w * rhs.z + self.z * rhs.w + self.x * rhs.y - self.y * rhs.x,
            w: self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
        }
        .normalized()
    }

    fn conjugate(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
        .normalized()
    }

    fn transform_point(self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let ix = self.w * x + self.y * z - self.z * y;
        let iy = self.w * y + self.z * x - self.x * z;
        let iz = self.w * z + self.x * y - self.y * x;
        let iw = -self.x * x - self.y * y - self.z * z;
        [
            ix * self.w + iw * -self.x + iy * -self.z - iz * -self.y,
            iy * self.w + iw * -self.y + iz * -self.x - ix * -self.z,
            iz * self.w + iw * -self.z + ix * -self.y - iy * -self.x,
        ]
    }

    fn matrix(self) -> [f64; 9] {
        let x2 = self.x + self.x;
        let y2 = self.y + self.y;
        let z2 = self.z + self.z;
        let xx = self.x * x2;
        let xy = self.x * y2;
        let xz = self.x * z2;
        let yy = self.y * y2;
        let yz = self.y * z2;
        let zz = self.z * z2;
        let wx = self.w * x2;
        let wy = self.w * y2;
        let wz = self.w * z2;
        [
            1.0 - (yy + zz),
            xy + wz,
            xz - wy,
            xy - wz,
            1.0 - (xx + zz),
            yz + wx,
            xz + wy,
            yz - wx,
            1.0 - (xx + yy),
        ]
    }
}

/// Project transforms are stored in PlayCanvas world coordinates while Gaussian
/// PLY rows use the PLY convention (180 degrees around Z). Convert the user
/// transform back into PLY space before baking so reopening the exported file
/// reproduces the same world-space result.
fn engine_transform_to_ply(transform: GaussianTransform) -> (GaussianTransform, Quaternion) {
    let ply_to_engine = Quaternion::from_euler_degrees([0.0, 0.0, 180.0]);
    let engine_to_ply = ply_to_engine.conjugate();
    let engine_rotation = Quaternion::from_euler_degrees(transform.rotation);
    let ply_rotation = engine_to_ply.mul(engine_rotation).mul(ply_to_engine);
    let mut mapped = transform;
    mapped.position = engine_to_ply.transform_point(transform.position);
    // transform_row consumes the conjugated quaternion separately.
    (mapped, ply_rotation)
}

fn unsupported(detail: impl Into<String>) -> SplatError {
    SplatError::Process(format!(
        "第一阶段仅支持 OOOSplat/Brush binary_little_endian Gaussian PLY：{}",
        detail.into()
    ))
}

fn parse_layout(reader: &mut impl BufRead) -> Result<PlyLayout> {
    parse_layout_with_empty(reader, false)
}

fn parse_layout_with_empty(reader: &mut impl BufRead, allow_empty: bool) -> Result<PlyLayout> {
    let mut header = Vec::new();
    let mut line = Vec::new();
    let mut format_ok = false;
    let mut vertex_seen = false;
    let mut in_vertex = false;
    let mut count = 0_u64;
    let mut stride = 0_usize;
    let mut offsets = HashMap::new();

    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Err(unsupported("PLY header 未完整结束"));
        }
        header.extend_from_slice(&line);
        if header.len() > MAX_HEADER_BYTES {
            return Err(unsupported("PLY header 超过 1 MB"));
        }
        let text = String::from_utf8_lossy(&line);
        let trimmed = text.trim();
        if header.len() == line.len() && trimmed != "ply" {
            return Err(unsupported("缺少 ply 文件标识"));
        }
        if trimmed == "format binary_little_endian 1.0" {
            format_ok = true;
        } else if let Some(rest) = trimmed.strip_prefix("element ") {
            let mut parts = rest.split_whitespace();
            let name = parts.next().unwrap_or_default();
            let element_count = parts
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| unsupported("element 数量无效"))?;
            in_vertex = name == "vertex";
            if in_vertex {
                if vertex_seen {
                    return Err(unsupported("vertex 包含重复声明"));
                }
                vertex_seen = true;
                count = element_count;
            } else if element_count > 0 {
                // Gaussian validation only reads vertex payloads. In
                // particular, a zero-vertex file with an absent face payload
                // is structurally incomplete, not a complete zero-point
                // training result that may authorize an algorithm fallback.
                return Err(unsupported("仅支持 vertex 数据元素"));
            }
        } else if trimmed.starts_with("property ") && in_vertex {
            let parts = trimmed.split_whitespace().collect::<Vec<_>>();
            if parts.len() != 3 || !matches!(parts[1], "float" | "float32") {
                return Err(unsupported("vertex 只支持标量 float32 属性"));
            }
            if offsets.insert(parts[2].to_string(), stride).is_some() {
                return Err(unsupported("vertex 包含重复属性"));
            }
            stride += 4;
        }
        if trimmed == "end_header" {
            break;
        }
    }

    if !format_ok || !vertex_seen || (!allow_empty && count == 0) || stride == 0 {
        return Err(unsupported("格式、vertex 数量或属性布局无效"));
    }
    for name in [
        "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2",
        "rot_0", "rot_1", "rot_2", "rot_3",
    ] {
        if !offsets.contains_key(name) {
            return Err(unsupported(format!("缺少 {name} 属性")));
        }
    }
    let rest_count = (0..)
        .take_while(|index| offsets.contains_key(&format!("f_rest_{index}")))
        .count();
    if !matches!(rest_count, 0 | 9 | 24 | 45) {
        return Err(unsupported("f_rest 属性必须完整对应 SH degree 0–3"));
    }
    if offsets.keys().any(|name| {
        name.strip_prefix("f_rest_")
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|index| index >= rest_count)
    }) {
        return Err(unsupported("f_rest 属性编号不连续"));
    }

    Ok(PlyLayout {
        header,
        count,
        stride,
        offsets,
        rest_count,
    })
}

fn read_float(row: &[u8], offset: usize) -> f64 {
    f32::from_le_bytes(row[offset..offset + 4].try_into().expect("float slice")) as f64
}

fn write_float(row: &mut [u8], offset: usize, value: f64) {
    row[offset..offset + 4].copy_from_slice(&(value as f32).to_le_bytes());
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GaussianExportEdits<'a> {
    pub crop: Option<GaussianCrop>,
    pub deleted_mask: Option<&'a [u8]>,
}

fn is_deleted(mask: Option<&[u8]>, index: u64) -> bool {
    mask.and_then(|bytes| bytes.get((index / 8) as usize))
        .is_some_and(|byte| byte & (1 << (index % 8)) != 0)
}

fn transformed_engine_point(
    row: &[u8],
    layout: &PlyLayout,
    transform: GaussianTransform,
    rotation: Quaternion,
) -> [f64; 3] {
    let offset = |name: &str| *layout.offsets.get(name).expect("validated property");
    let scaled = [
        read_float(row, offset("x")) * transform.scale,
        read_float(row, offset("y")) * transform.scale,
        read_float(row, offset("z")) * transform.scale,
    ];
    let point = rotation.transform_point(scaled);
    let ply = [
        point[0] + transform.position[0],
        point[1] + transform.position[1],
        point[2] + transform.position[2],
    ];
    // The preview applies a 180-degree Z rotation to PLY coordinates.
    [-ply[0], -ply[1], ply[2]]
}

fn keep_row(
    row: &[u8],
    index: u64,
    layout: &PlyLayout,
    transform: GaussianTransform,
    rotation: Quaternion,
    edits: GaussianExportEdits<'_>,
) -> bool {
    if is_deleted(edits.deleted_mask, index) {
        return false;
    }
    edits.crop.is_none_or(|crop| {
        crop.contains(transformed_engine_point(row, layout, transform, rotation))
    })
}

fn header_with_vertex_count(header: &[u8], count: u64) -> Vec<u8> {
    let text = String::from_utf8_lossy(header);
    let mut replaced = false;
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if !replaced && line.trim_start().starts_with("element vertex ") {
            let ending = if line.ends_with("\r\n") { "\r\n" } else { "\n" };
            output.push_str(&format!("element vertex {count}{ending}"));
            replaced = true;
        } else {
            output.push_str(line);
        }
    }
    output.into_bytes()
}

/// Cached offsets and fixed scratch buffers: no allocation/name lookup per row.
struct RowTransform {
    transform: GaussianTransform,
    rotation: Quaternion,
    rotated: bool,
    identity: bool,
    xyz: [usize; 3],
    quaternion: [usize; 4],
    scales: [usize; 3],
    log_scale: f64,
    sh: Vec<usize>,
    sh_rotation: Option<ShRotation>,
}
impl RowTransform {
    fn new(layout: &PlyLayout, transform: GaussianTransform, rotation: Quaternion) -> Self {
        let rotated =
            rotation.x.abs() > 1e-12 || rotation.y.abs() > 1e-12 || rotation.z.abs() > 1e-12;
        let offset = |name: &str| layout.offsets[name];
        Self {
            identity: !rotated && transform.scale == 1.0 && transform.position == [0.0; 3],
            transform,
            rotation,
            rotated,
            xyz: ["x", "y", "z"].map(offset),
            quaternion: ["rot_0", "rot_1", "rot_2", "rot_3"].map(offset),
            scales: ["scale_0", "scale_1", "scale_2"].map(offset),
            log_scale: transform.scale.ln(),
            sh: (0..layout.rest_count)
                .map(|i| offset(&format!("f_rest_{i}")))
                .collect(),
            sh_rotation: (rotated && layout.rest_count > 0)
                .then(|| ShRotation::new(rotation.matrix())),
        }
    }
    fn apply(&self, row: &mut [u8]) -> Result<()> {
        if self.identity {
            return Ok(());
        }
        let point = self
            .xyz
            .map(|offset| read_float(row, offset) * self.transform.scale);
        let point = if self.rotated {
            self.rotation.transform_point(point)
        } else {
            point
        };
        for (i, value) in point.into_iter().enumerate() {
            let value = value + self.transform.position[i];
            if !value.is_finite() {
                return Err(unsupported("Transform 导致 position 溢出"));
            }
            write_float(row, self.xyz[i], value);
        }
        if self.rotated {
            let [w, x, y, z] = self.quaternion.map(|offset| read_float(row, offset));
            let q = self.rotation.mul(Quaternion { w, x, y, z });
            for (offset, value) in self.quaternion.into_iter().zip([q.w, q.x, q.y, q.z]) {
                write_float(row, offset, value);
            }
        }
        if self.transform.scale != 1.0 {
            for offset in self.scales {
                write_float(row, offset, read_float(row, offset) + self.log_scale);
            }
        }
        if let Some(rotation) = &self.sh_rotation {
            let n = self.sh.len() / 3;
            let mut source = [0.0; 15];
            let mut result = [0.0; 15];
            for channel in 0..3 {
                for (i, value) in source[..n].iter_mut().enumerate() {
                    *value = read_float(row, self.sh[channel * n + i]);
                }
                rotation.apply_into(&source[..n], &mut result[..n]);
                for (i, value) in result[..n].iter().enumerate() {
                    write_float(row, self.sh[channel * n + i], *value);
                }
            }
        }
        Ok(())
    }
}

pub fn export_transformed_ply(
    source: &Path,
    project_root: &Path,
    transform: GaussianTransform,
    progress: impl FnMut(u64, u64),
) -> Result<(PathBuf, PlyInfo)> {
    export_transformed_ply_with_edits(
        source,
        project_root,
        transform,
        GaussianExportEdits::default(),
        progress,
    )
}

pub fn export_transformed_ply_with_edits(
    source: &Path,
    project_root: &Path,
    transform: GaussianTransform,
    edits: GaussianExportEdits<'_>,
    progress: impl FnMut(u64, u64),
) -> Result<(PathBuf, PlyInfo)> {
    let output = project_root.join("edit.ply");
    let info = export_transformed_ply_to(source, &output, transform, edits, progress, || Ok(()))?;
    Ok((output, info))
}

/// Export to a caller-owned destination without publishing or changing edit.ply.
pub fn export_transformed_ply_to(
    source: &Path,
    output: &Path,
    transform: GaussianTransform,
    edits: GaussianExportEdits<'_>,
    mut progress: impl FnMut(u64, u64),
    check_cancelled: impl Fn() -> Result<()>,
) -> Result<PlyInfo> {
    check_cancelled()?;
    let transform = transform.validate()?;
    let (transform, rotation) = engine_transform_to_ply(transform);
    let source = std::fs::canonicalize(source)?;
    let mut reader = BufReader::new(File::open(&source)?);
    let layout = parse_layout(&mut reader)?;
    if let Some(mask) = edits.deleted_mask {
        let expected = usize::try_from(layout.count.div_ceil(8))
            .map_err(|_| SplatError::Process("Gaussian 删除位图过大".into()))?;
        if mask.len() != expected {
            return Err(SplatError::Process(
                "Gaussian 删除位图与源 PLY 数量不一致".into(),
            ));
        }
    }
    let row_transform = RowTransform::new(&layout, transform, rotation);

    let mut retained = 0_u64;
    let mut row = vec![0_u8; layout.stride];
    for index in 0..layout.count {
        if index % ROWS_PER_CHUNK as u64 == 0 {
            check_cancelled()?;
            progress(index, layout.count.saturating_mul(2));
        }
        reader.read_exact(&mut row)?;
        if keep_row(&row, index, &layout, transform, rotation, edits) {
            retained += 1;
        }
    }
    if retained == 0 {
        return Err(SplatError::Process(
            "当前裁切和删除状态没有保留任何 Gaussian，无法导出".into(),
        ));
    }
    reader = BufReader::new(File::open(&source)?);
    let layout = parse_layout(&mut reader)?;

    let temporary = output.with_file_name(format!(".edit-{}.ply.tmp", Uuid::new_v4()));

    let result = (|| -> Result<()> {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(&header_with_vertex_count(&layout.header, retained))?;
        let mut rows_done = 0_u64;
        let mut chunk = vec![0_u8; layout.stride * ROWS_PER_CHUNK];
        let mut output_chunk = Vec::with_capacity(layout.stride * ROWS_PER_CHUNK);
        while rows_done < layout.count {
            check_cancelled()?;
            let rows = ((layout.count - rows_done) as usize).min(ROWS_PER_CHUNK);
            let bytes = rows * layout.stride;
            reader.read_exact(&mut chunk[..bytes])?;
            output_chunk.clear();
            for (chunk_index, row) in chunk[..bytes].chunks_exact_mut(layout.stride).enumerate() {
                let index = rows_done + chunk_index as u64;
                if !keep_row(row, index, &layout, transform, rotation, edits) {
                    continue;
                }
                row_transform.apply(row)?;
                output_chunk.extend_from_slice(row);
            }
            writer.write_all(&output_chunk)?;
            rows_done += rows as u64;
            progress(layout.count + rows_done, layout.count.saturating_mul(2));
        }
        loop {
            check_cancelled()?;
            let read = reader.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            writer.write_all(&chunk[..read])?;
        }
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }

    let info = match inspect_gaussian_ply(&temporary) {
        Ok(info) => info,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    };
    if info.splat_count != retained {
        let _ = std::fs::remove_file(&temporary);
        return Err(SplatError::Process("导出 PLY 的 Splat 数量校验失败".into()));
    }
    if let Err(error) = check_cancelled().and_then(|()| publish_export(&temporary, output)) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(info)
}

// PlayCanvas uses real SH coefficients grouped by channel and by bands of 3, 5 and 7.
struct ShRotation {
    band1: [[f64; 3]; 3],
    band2: [[f64; 5]; 5],
    band3: [[f64; 7]; 7],
}

impl ShRotation {
    fn new(rot: [f64; 9]) -> Self {
        let band1 = [
            [rot[4], -rot[7], rot[1]],
            [-rot[5], rot[8], -rot[2]],
            [rot[3], -rot[6], rot[0]],
        ];
        let band2 = build_band2(&band1);
        let band3 = build_band3(&band1, &band2);
        Self {
            band1,
            band2,
            band3,
        }
    }

    #[cfg(test)]
    fn apply(&self, source: &[f64]) -> Vec<f64> {
        let mut result = source.to_vec();
        self.apply_into(source, &mut result);
        result
    }
    fn apply_into(&self, source: &[f64], result: &mut [f64]) {
        if source.len() >= 3 {
            multiply_band(&mut result[0..3], &source[0..3], &self.band1);
        }
        if source.len() >= 8 {
            multiply_band(&mut result[3..8], &source[3..8], &self.band2);
        }
        if source.len() >= 15 {
            multiply_band(&mut result[8..15], &source[8..15], &self.band3);
        }
    }
}

fn multiply_band<const N: usize>(result: &mut [f64], source: &[f64], matrix: &[[f64; N]; N]) {
    for row in 0..N {
        result[row] = (0..N)
            .map(|column| source[column] * matrix[row][column])
            .sum();
    }
}

fn build_band2(s: &[[f64; 3]; 3]) -> [[f64; 5]; 5] {
    let q = |a: f64, b: f64, c: f64, d: f64| 0.5 * (a * b + c * d + b * a + d * c);
    let r3_4 = (3.0_f64 / 4.0).sqrt();
    let r1_3 = (1.0_f64 / 3.0).sqrt();
    let r4_3 = (4.0_f64 / 3.0).sqrt();
    let r1_4 = 0.5_f64;
    [
        [
            q(s[2][2], s[0][0], s[2][0], s[0][2]),
            s[2][1] * s[0][0] + s[0][1] * s[2][0],
            r3_4 * 2.0 * s[2][1] * s[0][1],
            s[2][1] * s[0][2] + s[0][1] * s[2][2],
            r1_4 * 2.0 * (s[2][2] * s[0][2] - s[2][0] * s[0][0]),
        ],
        [
            q(s[1][2], s[0][0], s[1][0], s[0][2]),
            s[1][1] * s[0][0] + s[0][1] * s[1][0],
            r3_4 * 2.0 * s[1][1] * s[0][1],
            s[1][1] * s[0][2] + s[0][1] * s[1][2],
            r1_4 * 2.0 * (s[1][2] * s[0][2] - s[1][0] * s[0][0]),
        ],
        [
            r1_3 * 2.0 * s[1][2] * s[1][0]
                - (1.0_f64 / 12.0).sqrt() * 2.0 * (s[2][2] * s[2][0] + s[0][2] * s[0][0]),
            r4_3 * s[1][1] * s[1][0] - r1_3 * (s[2][1] * s[2][0] + s[0][1] * s[0][0]),
            s[1][1] * s[1][1] - r1_4 * (s[2][1] * s[2][1] + s[0][1] * s[0][1]),
            r4_3 * s[1][1] * s[1][2] - r1_3 * (s[2][1] * s[2][2] + s[0][1] * s[0][2]),
            r1_3 * (s[1][2] * s[1][2] - s[1][0] * s[1][0])
                - (1.0_f64 / 12.0).sqrt()
                    * (s[2][2] * s[2][2] - s[2][0] * s[2][0] + s[0][2] * s[0][2]
                        - s[0][0] * s[0][0]),
        ],
        [
            q(s[1][2], s[2][0], s[1][0], s[2][2]),
            s[1][1] * s[2][0] + s[2][1] * s[1][0],
            r3_4 * 2.0 * s[1][1] * s[2][1],
            s[1][1] * s[2][2] + s[2][1] * s[1][2],
            r1_4 * 2.0 * (s[1][2] * s[2][2] - s[1][0] * s[2][0]),
        ],
        [
            r1_4 * 2.0 * (s[2][2] * s[2][0] - s[0][2] * s[0][0]),
            s[2][1] * s[2][0] - s[0][1] * s[0][0],
            r3_4 * (s[2][1] * s[2][1] - s[0][1] * s[0][1]),
            s[2][1] * s[2][2] - s[0][1] * s[0][2],
            r1_4 * (s[2][2] * s[2][2] - s[2][0] * s[2][0] - s[0][2] * s[0][2] + s[0][0] * s[0][0]),
        ],
    ]
}

fn build_band3(s: &[[f64; 3]; 3], b: &[[f64; 5]; 5]) -> [[f64; 7]; 7] {
    // The expressions below are the real-SH recurrence used by PlayCanvas splat-transform.
    let a = |n: f64, d: f64| (n / d).sqrt();
    [
        [
            0.5 * ((s[2][2] * b[0][0] + s[2][0] * b[0][4])
                + (s[0][2] * b[4][0] + s[0][0] * b[4][4])),
            a(3.0, 2.0) * (s[2][1] * b[0][0] + s[0][1] * b[4][0]),
            a(15.0, 16.0) * (s[2][1] * b[0][1] + s[0][1] * b[4][1]),
            a(5.0, 6.0) * (s[2][1] * b[0][2] + s[0][1] * b[4][2]),
            a(15.0, 16.0) * (s[2][1] * b[0][3] + s[0][1] * b[4][3]),
            a(3.0, 2.0) * (s[2][1] * b[0][4] + s[0][1] * b[4][4]),
            0.5 * ((s[2][2] * b[0][4] - s[2][0] * b[0][0])
                + (s[0][2] * b[4][4] - s[0][0] * b[4][0])),
        ],
        [
            a(1.0, 6.0)
                * (s[1][2] * b[0][0]
                    + s[1][0] * b[0][4]
                    + s[2][2] * b[1][0]
                    + s[2][0] * b[1][4]
                    + s[0][2] * b[3][0]
                    + s[0][0] * b[3][4]),
            s[1][1] * b[0][0] + s[2][1] * b[1][0] + s[0][1] * b[3][0],
            a(5.0, 8.0) * (s[1][1] * b[0][1] + s[2][1] * b[1][1] + s[0][1] * b[3][1]),
            a(5.0, 9.0) * (s[1][1] * b[0][2] + s[2][1] * b[1][2] + s[0][1] * b[3][2]),
            a(5.0, 8.0) * (s[1][1] * b[0][3] + s[2][1] * b[1][3] + s[0][1] * b[3][3]),
            s[1][1] * b[0][4] + s[2][1] * b[1][4] + s[0][1] * b[3][4],
            a(1.0, 6.0)
                * (s[1][2] * b[0][4] - s[1][0] * b[0][0] + s[2][2] * b[1][4] - s[2][0] * b[1][0]
                    + s[0][2] * b[3][4]
                    - s[0][0] * b[3][0]),
        ],
        [
            a(4.0, 15.0) * (s[1][2] * b[1][0] + s[1][0] * b[1][4])
                + a(1.0, 5.0) * (s[0][2] * b[2][0] + s[0][0] * b[2][4])
                - a(1.0, 60.0)
                    * (s[2][2] * b[0][0] + s[2][0] * b[0][4]
                        - s[0][2] * b[4][0]
                        - s[0][0] * b[4][4]),
            a(8.0, 5.0) * s[1][1] * b[1][0] + a(6.0, 5.0) * s[0][1] * b[2][0]
                - a(1.0, 10.0) * (s[2][1] * b[0][0] - s[0][1] * b[4][0]),
            s[1][1] * b[1][1] + a(3.0, 4.0) * s[0][1] * b[2][1]
                - 0.25 * (s[2][1] * b[0][1] - s[0][1] * b[4][1]),
            a(8.0, 9.0) * s[1][1] * b[1][2] + a(2.0, 3.0) * s[0][1] * b[2][2]
                - a(1.0, 18.0) * (s[2][1] * b[0][2] - s[0][1] * b[4][2]),
            s[1][1] * b[1][3] + a(3.0, 4.0) * s[0][1] * b[2][3]
                - 0.25 * (s[2][1] * b[0][3] - s[0][1] * b[4][3]),
            a(8.0, 5.0) * s[1][1] * b[1][4] + a(6.0, 5.0) * s[0][1] * b[2][4]
                - a(1.0, 10.0) * (s[2][1] * b[0][4] - s[0][1] * b[4][4]),
            a(4.0, 15.0) * (s[1][2] * b[1][4] - s[1][0] * b[1][0])
                + a(1.0, 5.0) * (s[0][2] * b[2][4] - s[0][0] * b[2][0])
                - a(1.0, 60.0)
                    * (s[2][2] * b[0][4] - s[2][0] * b[0][0] - s[0][2] * b[4][4]
                        + s[0][0] * b[4][0]),
        ],
        [
            a(3.0, 10.0) * (s[1][2] * b[2][0] + s[1][0] * b[2][4])
                - a(1.0, 10.0)
                    * (s[2][2] * b[3][0]
                        + s[2][0] * b[3][4]
                        + s[0][2] * b[1][0]
                        + s[0][0] * b[1][4]),
            a(9.0, 5.0) * s[1][1] * b[2][0] - a(3.0, 5.0) * (s[2][1] * b[3][0] + s[0][1] * b[1][0]),
            a(9.0, 8.0) * s[1][1] * b[2][1] - a(3.0, 8.0) * (s[2][1] * b[3][1] + s[0][1] * b[1][1]),
            s[1][1] * b[2][2] - a(1.0, 3.0) * (s[2][1] * b[3][2] + s[0][1] * b[1][2]),
            a(9.0, 8.0) * s[1][1] * b[2][3] - a(3.0, 8.0) * (s[2][1] * b[3][3] + s[0][1] * b[1][3]),
            a(9.0, 5.0) * s[1][1] * b[2][4] - a(3.0, 5.0) * (s[2][1] * b[3][4] + s[0][1] * b[1][4]),
            a(3.0, 10.0) * (s[1][2] * b[2][4] - s[1][0] * b[2][0])
                - a(1.0, 10.0)
                    * (s[2][2] * b[3][4] - s[2][0] * b[3][0] + s[0][2] * b[1][4]
                        - s[0][0] * b[1][0]),
        ],
        [
            a(4.0, 15.0) * (s[1][2] * b[3][0] + s[1][0] * b[3][4])
                + a(1.0, 5.0) * (s[2][2] * b[2][0] + s[2][0] * b[2][4])
                - a(1.0, 60.0)
                    * (s[2][2] * b[4][0]
                        + s[2][0] * b[4][4]
                        + s[0][2] * b[0][0]
                        + s[0][0] * b[0][4]),
            a(8.0, 5.0) * s[1][1] * b[3][0] + a(6.0, 5.0) * s[2][1] * b[2][0]
                - a(1.0, 10.0) * (s[2][1] * b[4][0] + s[0][1] * b[0][0]),
            s[1][1] * b[3][1] + a(3.0, 4.0) * s[2][1] * b[2][1]
                - 0.25 * (s[2][1] * b[4][1] + s[0][1] * b[0][1]),
            a(8.0, 9.0) * s[1][1] * b[3][2] + a(2.0, 3.0) * s[2][1] * b[2][2]
                - a(1.0, 18.0) * (s[2][1] * b[4][2] + s[0][1] * b[0][2]),
            s[1][1] * b[3][3] + a(3.0, 4.0) * s[2][1] * b[2][3]
                - 0.25 * (s[2][1] * b[4][3] + s[0][1] * b[0][3]),
            a(8.0, 5.0) * s[1][1] * b[3][4] + a(6.0, 5.0) * s[2][1] * b[2][4]
                - a(1.0, 10.0) * (s[2][1] * b[4][4] + s[0][1] * b[0][4]),
            a(4.0, 15.0) * (s[1][2] * b[3][4] - s[1][0] * b[3][0])
                + a(1.0, 5.0) * (s[2][2] * b[2][4] - s[2][0] * b[2][0])
                - a(1.0, 60.0)
                    * (s[2][2] * b[4][4] - s[2][0] * b[4][0] + s[0][2] * b[0][4]
                        - s[0][0] * b[0][0]),
        ],
        [
            a(1.0, 6.0)
                * (s[1][2] * b[4][0] + s[1][0] * b[4][4] + s[2][2] * b[3][0] + s[2][0] * b[3][4]
                    - s[0][2] * b[1][0]
                    - s[0][0] * b[1][4]),
            s[1][1] * b[4][0] + s[2][1] * b[3][0] - s[0][1] * b[1][0],
            a(5.0, 8.0) * (s[1][1] * b[4][1] + s[2][1] * b[3][1] - s[0][1] * b[1][1]),
            a(5.0, 9.0) * (s[1][1] * b[4][2] + s[2][1] * b[3][2] - s[0][1] * b[1][2]),
            a(5.0, 8.0) * (s[1][1] * b[4][3] + s[2][1] * b[3][3] - s[0][1] * b[1][3]),
            s[1][1] * b[4][4] + s[2][1] * b[3][4] - s[0][1] * b[1][4],
            a(1.0, 6.0)
                * (s[1][2] * b[4][4] - s[1][0] * b[4][0] + s[2][2] * b[3][4]
                    - s[2][0] * b[3][0]
                    - s[0][2] * b[1][4]
                    + s[0][0] * b[1][0]),
        ],
        [
            0.5 * (s[2][2] * b[4][0] + s[2][0] * b[4][4] - s[0][2] * b[0][0] - s[0][0] * b[0][4]),
            a(3.0, 2.0) * (s[2][1] * b[4][0] - s[0][1] * b[0][0]),
            a(15.0, 16.0) * (s[2][1] * b[4][1] - s[0][1] * b[0][1]),
            a(5.0, 6.0) * (s[2][1] * b[4][2] - s[0][1] * b[0][2]),
            a(15.0, 16.0) * (s[2][1] * b[4][3] - s[0][1] * b[0][3]),
            a(3.0, 2.0) * (s[2][1] * b[4][4] - s[0][1] * b[0][4]),
            0.5 * (s[2][2] * b[4][4] - s[2][0] * b[4][0] - s[0][2] * b[0][4] + s[0][0] * b[0][0]),
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture_rows(path: &Path, positions: &[[f32; 3]]) {
        let properties = [
            "f_dc_0", "x", "rot_0", "scale_0", "y", "f_dc_1", "rot_1", "scale_1", "z", "f_dc_2",
            "rot_2", "scale_2", "opacity", "rot_3",
        ];
        let mut bytes = format!(
            "ply\nformat binary_little_endian 1.0\nelement vertex {}\n{}end_header\n",
            positions.len(),
            properties
                .iter()
                .map(|name| format!("property float {name}\n"))
                .collect::<String>()
        )
        .into_bytes();
        for [x, y, z] in positions {
            let values = [
                0.1_f32, *x, 1.0, 0.0, *y, 0.2, 0.0, 0.0, *z, 0.3, 0.0, 0.0, 1.0, 0.0,
            ];
            for value in values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn write_fixture(path: &Path) {
        write_fixture_rows(path, &[[1.0, 0.0, 0.0]]);
    }

    #[test]
    fn reshoot_full_ply_validation_rejects_nonfinite_truncated_and_cancelled_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("init.ply");
        let token = tokio_util::sync::CancellationToken::new();
        write_fixture(&path);
        let original = std::fs::read(&path).unwrap();
        let (info, sh, hash) = validate_training_ply(&path, &token).unwrap();
        assert_eq!((info.splat_count, sh), (1, 0));
        assert_eq!(hash, crate::video::panorama::fingerprint(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::write(&path, &original[..original.len() - 1]).unwrap();
        assert!(validate_training_ply(&path, &token).is_err());
        write_fixture_rows(&path, &[[f32::NAN, 0., 0.]]);
        assert!(validate_training_ply(&path, &token).is_err());
        write_fixture(&path);
        token.cancel();
        assert!(matches!(
            validate_training_ply(&path, &token),
            Err(SplatError::Cancelled)
        ));
    }

    #[test]
    fn classified_validation_only_allows_complete_invalid_gaussians_to_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("candidate.ply");
        let cancel = tokio_util::sync::CancellationToken::new();
        let missing = validate_training_ply_classified(&path, &cancel).unwrap_err();
        assert!(!missing.is_algorithm_failure());
        assert_eq!(
            missing.into_error().io_error().unwrap().kind(),
            std::io::ErrorKind::NotFound
        );
        for bytes in [
            b"".as_slice(),
            b"ply\n",
            b"not a PLY",
            b"ply\nformat binary_little_endian 1.0\nelement vertex 0\nend_header\n",
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(!validate_training_ply_classified(&path, &cancel)
                .unwrap_err()
                .is_algorithm_failure());
        }
        write_fixture_rows(&path, &[]);
        assert!(matches!(
            validate_training_ply_classified(&path, &cancel),
            Err(TrainingPlyValidationError::InvalidGaussian(
                InvalidGaussianKind::ZeroPoints
            ))
        ));
        write_fixture_rows(&path, &[[f32::NAN, 0., 0.], [0., 1., 0.]]);
        assert!(matches!(
            validate_training_ply_classified(&path, &cancel),
            Err(TrainingPlyValidationError::InvalidGaussian(
                InvalidGaussianKind::NonFiniteAttributes
            ))
        ));
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        assert!(!validate_training_ply_classified(&path, &cancel)
            .unwrap_err()
            .is_algorithm_failure());
        write_fixture(&path);
        let mut bytes = std::fs::read(&path).unwrap();
        let layout = parse_layout(&mut std::io::Cursor::new(&bytes)).unwrap();
        for index in 0..4 {
            let offset = layout.header.len() + layout.offsets[&format!("rot_{index}")];
            bytes[offset..offset + 4].copy_from_slice(&0_f32.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(
            validate_training_ply_classified(&path, &cancel),
            Err(TrainingPlyValidationError::InvalidGaussian(
                InvalidGaussianKind::InvalidRotation
            ))
        ));
        cancel.cancel();
        assert!(matches!(
            validate_training_ply_classified(&path, &cancel),
            Err(TrainingPlyValidationError::Operational(
                SplatError::Cancelled
            ))
        ));
    }

    #[test]
    fn malformed_zero_point_headers_are_not_algorithm_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("candidate.ply");
        let cancel = tokio_util::sync::CancellationToken::new();
        write_fixture_rows(&path, &[]);
        let header = std::fs::read_to_string(&path).unwrap();
        for declaration in [
            "element vertex 0\n",
            "element vertex 1\n",
            "element face 1\nproperty list uchar int vertex_indices\n",
        ] {
            std::fs::write(
                &path,
                header.replace("end_header\n", &format!("{declaration}end_header\n")),
            )
            .unwrap();
            assert!(matches!(
                validate_training_ply_classified(&path, &cancel),
                Err(TrainingPlyValidationError::Operational(_))
            ));
        }
        // An empty auxiliary element carries no payload and remains valid.
        std::fs::write(
            &path,
            header.replace("end_header\n", "element face 0\nend_header\n"),
        )
        .unwrap();
        assert!(matches!(
            validate_training_ply_classified(&path, &cancel),
            Err(TrainingPlyValidationError::InvalidGaussian(
                InvalidGaussianKind::ZeroPoints
            ))
        ));
    }

    #[test]
    fn reading_after_nonfinite_attributes_preserves_later_io_failure() {
        struct FailsAfterRow(std::io::Cursor<Vec<u8>>);
        impl Read for FailsAfterRow {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                if self.0.position() as usize == self.0.get_ref().len() {
                    return Err(std::io::Error::from_raw_os_error(5));
                }
                self.0.read(bytes)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("candidate.ply");
        write_fixture_rows(&path, &[[f32::NAN, 0., 0.], [0., 1., 0.]]);
        let bytes = std::fs::read(&path).unwrap();
        let layout = parse_layout(&mut std::io::Cursor::new(&bytes)).unwrap();
        let first_row = bytes[layout.header.len()..layout.header.len() + layout.stride].to_vec();
        let mut reader = FailsAfterRow(std::io::Cursor::new(first_row));
        let error = validate_training_payload(
            &mut reader,
            layout,
            bytes.len() as u64,
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap_err();
        assert!(!error.is_algorithm_failure());
        assert_eq!(
            error.into_error().io_error().unwrap().raw_os_error(),
            Some(5)
        );
    }

    #[test]
    fn identity_sh_rotation_preserves_coefficients() {
        let rotation = ShRotation::new(Quaternion::from_euler_degrees([0.0, 0.0, 0.0]).matrix());
        let source = (1..=15).map(f64::from).collect::<Vec<_>>();
        let result = rotation.apply(&source);
        for (actual, expected) in result.iter().zip(source) {
            assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
        }
    }

    #[test]
    fn playcanvas_euler_rotation_matches_expected_axis() {
        let q = Quaternion::from_euler_degrees([0.0, 0.0, 90.0]);
        let point = q.transform_point([1.0, 0.0, 0.0]);
        assert!(point[0].abs() < 1e-10);
        assert!((point[1] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn maps_playcanvas_world_transform_back_to_ply_coordinates() {
        let transform = GaussianTransform {
            position: [1.0, 2.0, 3.0],
            rotation: [90.0, 0.0, 0.0],
            scale: 1.0,
        };
        let (mapped, ply_rotation) = engine_transform_to_ply(transform);
        assert!((mapped.position[0] + 1.0).abs() < 1e-10);
        assert!((mapped.position[1] + 2.0).abs() < 1e-10);
        assert!((mapped.position[2] - 3.0).abs() < 1e-10);

        let ply_to_engine = Quaternion::from_euler_degrees([0.0, 0.0, 180.0]);
        let source = [2.0, 3.0, 4.0];
        let actual = ply_to_engine.transform_point(ply_rotation.transform_point(source));
        let expected = Quaternion::from_euler_degrees(transform.rotation)
            .transform_point(ply_to_engine.transform_point(source));
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
        }
    }

    #[test]
    fn z_rotation_matches_official_first_sh_band_mapping() {
        let rotation = ShRotation::new(Quaternion::from_euler_degrees([0.0, 0.0, 90.0]).matrix());
        let result = rotation.apply(&[1.0, 2.0, 3.0]);
        assert!((result[0] - 3.0).abs() < 1e-10);
        assert!((result[1] - 2.0).abs() < 1e-10);
        assert!((result[2] + 1.0).abs() < 1e-10);
    }

    #[test]
    fn streams_reordered_properties_and_replaces_the_single_edit_export() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("final.ply");
        write_fixture(&source);
        let original = std::fs::read(&source).unwrap();
        let transform = GaussianTransform {
            position: [10.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 90.0],
            scale: 2.0,
        };
        let (first, info) =
            export_transformed_ply(&source, directory.path(), transform, |_, _| {}).unwrap();
        let (second, _) =
            export_transformed_ply(&source, directory.path(), transform, |_, _| {}).unwrap();
        assert_eq!(first.file_name().unwrap(), "edit.ply");
        assert_eq!(second.file_name().unwrap(), "edit.ply");
        assert_eq!(info.splat_count, 1);
        assert_eq!(std::fs::read(&source).unwrap(), original);

        let mut reader = BufReader::new(File::open(first).unwrap());
        let layout = parse_layout(&mut reader).unwrap();
        let mut row = vec![0_u8; layout.stride];
        reader.read_exact(&mut row).unwrap();
        let get = |name: &str| read_float(&row, layout.offsets[name]);
        assert!((get("x") + 10.0).abs() < 1e-5);
        assert!((get("y") - 2.0).abs() < 1e-5);
        assert!(get("z").abs() < 1e-5);
        assert!((get("scale_0") - 2.0_f64.ln()).abs() < 1e-5);
        assert!((get("rot_0") - 0.5_f64.sqrt()).abs() < 1e-5);
        assert!((get("rot_3") - 0.5_f64.sqrt()).abs() < 1e-5);
    }

    #[test]
    fn filters_deleted_and_cropped_rows_without_changing_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("final.ply");
        write_fixture_rows(
            &source,
            &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [4.0, 0.0, 0.0]],
        );
        let original = std::fs::read(&source).unwrap();
        let deleted = [0b0000_0001];
        let edits = GaussianExportEdits {
            crop: Some(GaussianCrop::Box {
                center: [-1.0, 0.0, 0.0],
                size: [3.0, 1.0, 1.0],
                inverted: false,
            }),
            deleted_mask: Some(&deleted),
        };
        let (output, info) = export_transformed_ply_with_edits(
            &source,
            directory.path(),
            GaussianTransform::default(),
            edits,
            |_, _| {},
        )
        .unwrap();

        assert_eq!(info.splat_count, 1);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let mut reader = BufReader::new(File::open(output).unwrap());
        let layout = parse_layout(&mut reader).unwrap();
        assert_eq!(layout.count, 1);
        let mut row = vec![0_u8; layout.stride];
        reader.read_exact(&mut row).unwrap();
        assert!((read_float(&row, layout.offsets["x"]) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn dedicated_html_model_preserves_existing_edit_and_source_files() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("final.ply");
        write_fixture_rows(
            &source,
            &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [4.0, 0.0, 0.0]],
        );
        let original = std::fs::read(&source).unwrap();
        let edit = directory.path().join("edit.ply");
        std::fs::write(&edit, b"existing edit").unwrap();
        let output = directory.path().join("html-model.ply");
        let info = export_transformed_ply_to(
            &source,
            &output,
            GaussianTransform {
                position: [10.0, 0.0, 0.0],
                ..GaussianTransform::default()
            },
            GaussianExportEdits {
                crop: Some(GaussianCrop::Box {
                    center: [8.0, 0.0, 0.0],
                    size: [10.0, 2.0, 2.0],
                    inverted: false,
                }),
                deleted_mask: Some(&[1]),
            },
            |_, _| {},
            || Ok(()),
        )
        .unwrap();
        assert_eq!(info.splat_count, 2);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert_eq!(std::fs::read(&edit).unwrap(), b"existing edit");
        let mut reader = BufReader::new(File::open(output).unwrap());
        let layout = parse_layout(&mut reader).unwrap();
        let mut row = vec![0; layout.stride];
        reader.read_exact(&mut row).unwrap();
        assert!((read_float(&row, layout.offsets["x"]) + 8.0).abs() < 1e-5);
    }

    #[test]
    fn exports_inverted_sphere_and_box_using_the_same_world_space_filter() {
        for crop in [
            GaussianCrop::Sphere {
                center: [-1.0, 0.0, 0.0],
                radius: 2.0,
                inverted: true,
            },
            GaussianCrop::Box {
                center: [-1.0, 0.0, 0.0],
                size: [4.0, 2.0, 2.0],
                inverted: true,
            },
        ] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("final.ply");
            write_fixture_rows(
                &source,
                &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [4.0, 0.0, 0.0]],
            );
            let original = std::fs::read(&source).unwrap();
            let (output, info) = export_transformed_ply_with_edits(
                &source,
                directory.path(),
                GaussianTransform::default(),
                GaussianExportEdits {
                    crop: Some(crop),
                    deleted_mask: Some(&[1]),
                },
                |_, _| {},
            )
            .unwrap();
            assert_eq!(info.splat_count, 1);
            assert_eq!(std::fs::read(&source).unwrap(), original);
            let mut reader = BufReader::new(File::open(output).unwrap());
            let layout = parse_layout(&mut reader).unwrap();
            let mut row = vec![0; layout.stride];
            reader.read_exact(&mut row).unwrap();
            assert_eq!(read_float(&row, layout.offsets["x"]), 4.0);
        }
    }
    #[test]
    fn cancelled_dedicated_export_cleans_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("final.ply");
        write_fixture(&source);
        let output = directory.path().join("html-model.ply");
        let progress = std::cell::Cell::new(0);
        let result = export_transformed_ply_to(
            &source,
            &output,
            GaussianTransform::default(),
            GaussianExportEdits::default(),
            |done, _| progress.set(done),
            || {
                if progress.get() > 0 {
                    Err(SplatError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(result, Err(SplatError::Cancelled)));
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
