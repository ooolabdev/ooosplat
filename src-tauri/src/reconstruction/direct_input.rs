//! Strict, read-only admission for a single explicitly selected COLMAP model.
use super::colmap_model::{read_cameras, ColmapCamera};
use crate::{
    error::{Result, SplatError},
    pipeline::inputs::{ColmapInput, INVALID_COLMAP},
    video::panorama::PosedImage,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::File,
    io::{BufRead, BufReader, Read, Seek},
    path::{Component, Path, PathBuf},
};

pub fn invalid() -> SplatError {
    SplatError::Process(INVALID_COLMAP.into())
}
fn invalid_model(reason: &str) -> SplatError {
    tracing::debug!(reason, "Imported COLMAP validation failed");
    #[cfg(test)]
    eprintln!("Imported COLMAP validation failed: {reason}");
    invalid()
}
pub fn image_path(root: &Path, name: &str) -> Result<PathBuf> {
    // Accept portable relative names only, including on Unix when reading a Windows model.
    if name.is_empty()
        || name.contains(['\\', ':', '\0', '\n', '\r'])
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid());
    }
    let root = std::fs::canonicalize(root)?;
    let path = std::fs::canonicalize(root.join(name))?;
    if !path.starts_with(root) || !path.is_file() {
        return Err(invalid());
    }
    Ok(path)
}

pub(crate) fn open_immutable_database(path: &Path) -> Result<rusqlite::Connection> {
    let canonical = std::fs::canonicalize(path)?;
    let raw = canonical.to_string_lossy();
    let portable = raw
        .strip_prefix(r"\\?\UNC\")
        .map(|value| format!("//{value}"))
        .or_else(|| raw.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| raw.into_owned())
        .replace('\\', "/");
    let mut uri = String::with_capacity(portable.len() + 24);
    uri.push_str("file:");
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in portable.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(byte));
        } else {
            uri.push('%');
            uri.push(char::from(HEX[(byte >> 4) as usize]));
            uri.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    uri.push_str("?immutable=1");
    rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(database_read_error)
}

/// A malformed cache is replaceable; an inaccessible database is not. Keep
/// operational SQLite failures in the I/O source chain instead of turning them
/// into the same validation string used for inconsistent reconstruction data.
pub(crate) fn database_read_error(error: rusqlite::Error) -> SplatError {
    use rusqlite::ErrorCode;
    let kind = match error.sqlite_error_code() {
        Some(ErrorCode::PermissionDenied | ErrorCode::ReadOnly) => {
            Some(std::io::ErrorKind::PermissionDenied)
        }
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => {
            Some(std::io::ErrorKind::WouldBlock)
        }
        Some(ErrorCode::DiskFull) => Some(std::io::ErrorKind::StorageFull),
        Some(ErrorCode::OperationInterrupted) => Some(std::io::ErrorKind::Interrupted),
        Some(
            ErrorCode::SystemIoFailure
            | ErrorCode::CannotOpen
            | ErrorCode::OutOfMemory
            | ErrorCode::FileLockingProtocolFailed,
        ) => Some(std::io::ErrorKind::Other),
        _ => None,
    };
    if let Some(kind) = kind {
        SplatError::Io(std::io::Error::new(kind, error))
    } else {
        invalid_model(&format!("COLMAP database validation failed: {error}"))
    }
}

fn ensure_legacy_database_has_no_pending_writes(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        match std::fs::symlink_metadata(&sidecar) {
            Ok(metadata) if !metadata.file_type().is_file() || metadata.len() > 0 => {
                return Err(invalid_model(
                    "legacy COLMAP database has an uncheckpointed sidecar",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn update_legacy_content_identity(
    hasher: &mut Sha256,
    role: &[u8],
    name: &str,
    path: &Path,
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(invalid_model("legacy identity input is not a regular file"));
    }
    hasher.update((role.len() as u64).to_le_bytes());
    hasher.update(role);
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.update(metadata.len().to_le_bytes());
    let mut reader = BufReader::new(File::open(path)?);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct ModelImage {
    pub pose: PosedImage,
    pub observations: Vec<Observation>,
}
#[derive(Clone, Debug)]
pub struct Observation {
    pub x: f64,
    pub y: f64,
    pub point: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct Point {
    pub id: u64,
    pub xyz: [f64; 3],
    pub rgb: [u8; 3],
    pub error: f64,
    pub track: Vec<(u32, u32)>,
}
pub struct Model {
    pub cameras: Vec<ColmapCamera>,
    pub images: Vec<ModelImage>,
    pub points: Vec<Point>,
}
fn bytes<const N: usize>(r: &mut impl Read) -> Result<[u8; N]> {
    let mut b = [0; N];
    r.read_exact(&mut b)?;
    Ok(b)
}
fn u64r(r: &mut impl Read) -> Result<u64> {
    Ok(u64::from_le_bytes(bytes(r)?))
}
fn f64r(r: &mut impl Read) -> Result<f64> {
    Ok(f64::from_le_bytes(bytes(r)?))
}
fn u32r(r: &mut impl Read) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(r)?))
}
fn count(r: &mut (impl Read + Seek), unit: u64, remaining: u64) -> Result<usize> {
    let n = u64r(r)?;
    if n > remaining.saturating_sub(r.stream_position()?) / unit {
        return Err(invalid());
    }
    usize::try_from(n).map_err(|_| invalid())
}
fn parse<T: std::str::FromStr>(s: &str) -> Result<T> {
    s.parse().map_err(|_| invalid())
}
pub fn read_model(path: &Path) -> Result<Model> {
    // The lower-level binary/text readers choose a representation using
    // `is_file()`. Probe every candidate first so a permission error cannot be
    // mistaken for a missing binary model and silently fall back to text.
    for name in [
        "cameras.bin",
        "images.bin",
        "points3D.bin",
        "cameras.txt",
        "images.txt",
        "points3D.txt",
    ] {
        match std::fs::symlink_metadata(path.join(name)) {
            Ok(metadata) if !metadata.file_type().is_file() => return Err(invalid()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for name in [
        "cameras.bin",
        "images.bin",
        "points3D.bin",
        "cameras.txt",
        "images.txt",
        "points3D.txt",
    ] {
        if let Ok(stat) = std::fs::symlink_metadata(path.join(name)) {
            if !stat.file_type().is_file() {
                return Err(invalid());
            }
        }
    }
    let binary = ["cameras.bin", "images.bin", "points3D.bin"].map(|n| path.join(n).is_file());
    let text = ["cameras.txt", "images.txt", "points3D.txt"].map(|n| path.join(n).is_file());
    if !binary.iter().all(|b| *b) && (binary.iter().any(|b| *b) || !text.iter().all(|b| *b)) {
        return Err(invalid());
    }
    if text.iter().any(|b| *b) && !text.iter().all(|b| *b) {
        return Err(invalid());
    }
    let model = read_model_format(path, binary[0])?;
    if binary[0] && text[0] && !same_model(&model, &read_model_format(path, false)?) {
        return Err(invalid());
    }
    Ok(model)
}
fn same_model(a: &Model, b: &Model) -> bool {
    let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * (1. + a.abs().max(b.abs()));
    if a.cameras.len() != b.cameras.len()
        || a.images.len() != b.images.len()
        || a.points.len() != b.points.len()
    {
        return false;
    }
    let cams = b
        .cameras
        .iter()
        .map(|c| (c.id, c))
        .collect::<BTreeMap<_, _>>();
    for c in &a.cameras {
        let Some(d) = cams.get(&c.id) else {
            return false;
        };
        if c.model_id != d.model_id
            || c.width != d.width
            || c.height != d.height
            || c.params.len() != d.params.len()
            || !c.params.iter().zip(&d.params).all(|(a, b)| close(*a, *b))
        {
            return false;
        }
    }
    let images = b
        .images
        .iter()
        .map(|i| (i.pose.id, i))
        .collect::<BTreeMap<_, _>>();
    for i in &a.images {
        let Some(j) = images.get(&i.pose.id) else {
            return false;
        };
        if i.pose.name != j.pose.name
            || i.pose.camera_id != j.pose.camera_id
            || !i
                .pose
                .qvec
                .iter()
                .chain(i.pose.tvec.iter())
                .zip(j.pose.qvec.iter().chain(j.pose.tvec.iter()))
                .all(|(a, b)| close(*a, *b))
            || i.observations.len() != j.observations.len()
            || !i
                .observations
                .iter()
                .zip(&j.observations)
                .all(|(a, b)| a.point == b.point && close(a.x, b.x) && close(a.y, b.y))
        {
            return false;
        }
    }
    let points = b
        .points
        .iter()
        .map(|p| (p.id, p))
        .collect::<BTreeMap<_, _>>();
    for p in &a.points {
        let Some(q) = points.get(&p.id) else {
            return false;
        };
        let mut p_track = p.track.clone();
        let mut q_track = q.track.clone();
        p_track.sort();
        q_track.sort();
        if p.rgb != q.rgb
            || !close(p.error, q.error)
            || !p.xyz.iter().zip(q.xyz).all(|(a, b)| close(*a, b))
            || p_track != q_track
        {
            return false;
        }
    }
    true
}
fn read_model_format(path: &Path, use_binary: bool) -> Result<Model> {
    let binary = [use_binary; 3];
    let cameras = if use_binary {
        read_cameras(path)?
    } else {
        super::colmap_model::read_cameras_text(&path.join("cameras.txt"))?
    };
    if binary[0] {
        let mut file = File::open(path.join("images.bin"))?;
        let len = file.metadata()?.len();
        count(&mut file, 73, len)?;
    }
    let mut images = Vec::new();
    let mut points = Vec::new();
    if binary[0] {
        let file = File::open(path.join("images.bin"))?;
        let len = file.metadata()?.len();
        let mut r = BufReader::new(file);
        let n = count(&mut r, 73, len)?;
        for _ in 0..n {
            let id = u32r(&mut r)?;
            let qvec = [f64r(&mut r)?, f64r(&mut r)?, f64r(&mut r)?, f64r(&mut r)?];
            let tvec = [f64r(&mut r)?, f64r(&mut r)?, f64r(&mut r)?];
            let camera = u32r(&mut r)?;
            let mut name = Vec::new();
            loop {
                let b = bytes::<1>(&mut r)?[0];
                if b == 0 {
                    break;
                }
                name.push(b);
                if name.len() > 32768 {
                    return Err(invalid());
                }
            }
            let pose = PosedImage {
                id,
                camera_id: camera,
                name: String::from_utf8(name).map_err(|_| invalid())?,
                qvec,
                tvec,
            };
            let n = count(&mut r, 24, len)?;
            let mut observations = Vec::with_capacity(n);
            for _ in 0..n {
                let x = f64r(&mut r)?;
                let y = f64r(&mut r)?;
                let p = u64r(&mut r)?;
                observations.push(Observation {
                    x,
                    y,
                    point: (p != u64::MAX).then_some(p),
                });
            }
            images.push(ModelImage { pose, observations });
        }
        if r.read(&mut [0; 1])? != 0 {
            return Err(invalid());
        }
        let file = File::open(path.join("points3D.bin"))?;
        let len = file.metadata()?.len();
        let mut r = BufReader::new(file);
        let n = count(&mut r, 51, len)?;
        for _ in 0..n {
            let id = u64r(&mut r)?;
            let xyz = [f64r(&mut r)?, f64r(&mut r)?, f64r(&mut r)?];
            let rgb = bytes(&mut r)?;
            let error = f64r(&mut r)?;
            let n = count(&mut r, 8, len)?;
            let mut track = Vec::with_capacity(n);
            for _ in 0..n {
                track.push((u32r(&mut r)?, u32r(&mut r)?));
            }
            points.push(Point {
                id,
                xyz,
                rgb,
                error,
                track,
            });
        }
        if r.read(&mut [0; 1])? != 0 {
            return Err(invalid());
        }
    } else {
        let mut lines = BufReader::new(File::open(path.join("images.txt"))?).lines();
        while let Some(row) = lines.next() {
            let row = row?;
            if row.trim().is_empty() || row.trim_start().starts_with('#') {
                continue;
            }
            let fields = row.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 10 {
                return Err(invalid_model("invalid image name"));
            }
            let id = parse::<u32>(fields[0])?;
            let pose = PosedImage {
                id,
                camera_id: parse(fields[8])?,
                name: fields[9..].join(" "),
                qvec: [
                    parse(fields[1])?,
                    parse(fields[2])?,
                    parse(fields[3])?,
                    parse(fields[4])?,
                ],
                tvec: [parse(fields[5])?, parse(fields[6])?, parse(fields[7])?],
            };
            let row = lines.next().ok_or_else(invalid)??;
            let fields = row.split_whitespace().collect::<Vec<_>>();
            if !fields.len().is_multiple_of(3) {
                return Err(invalid());
            }
            let observations = fields
                .as_chunks::<3>()
                .0
                .iter()
                .map(|f| {
                    Ok(Observation {
                        x: parse(f[0])?,
                        y: parse(f[1])?,
                        point: if f[2] == "-1" {
                            None
                        } else {
                            Some(parse(f[2])?)
                        },
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            images.push(ModelImage { pose, observations });
        }
        for line in BufReader::new(File::open(path.join("points3D.txt"))?).lines() {
            let line = line?;
            let f = line.split_whitespace().collect::<Vec<_>>();
            if f.is_empty() || f[0].starts_with('#') {
                continue;
            }
            if f.len() < 8 || !(f.len() - 8).is_multiple_of(2) {
                return Err(invalid());
            }
            let track = f[8..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|f| Ok((parse(f[0])?, parse(f[1])?)))
                .collect::<Result<Vec<_>>>()?;
            points.push(Point {
                id: parse(f[0])?,
                xyz: [parse(f[1])?, parse(f[2])?, parse(f[3])?],
                rgb: [parse(f[4])?, parse(f[5])?, parse(f[6])?],
                error: parse(f[7])?,
                track,
            });
        }
    }
    let model = Model {
        cameras,
        images,
        points,
    };
    model.validate()?;
    Ok(model)
}
impl Model {
    pub fn validate(&self) -> Result<()> {
        if self.images.is_empty() || self.points.is_empty() {
            return Err(invalid_model("empty model"));
        }
        let cameras = self
            .cameras
            .iter()
            .map(|c| (c.id, c))
            .collect::<BTreeMap<_, _>>();
        let images = self
            .images
            .iter()
            .map(|i| (i.pose.id, i))
            .collect::<BTreeMap<_, _>>();
        let points = self
            .points
            .iter()
            .map(|p| (p.id, p))
            .collect::<BTreeMap<_, _>>();
        if cameras.len() != self.cameras.len()
            || images.len() != self.images.len()
            || points.len() != self.points.len()
        {
            return Err(invalid_model("duplicate image/point id"));
        }
        let mut names = HashSet::new();
        for image in &self.images {
            let p = &image.pose;
            if p.name.is_empty()
                || p.name.contains(['\\', ':', '\0', '\n', '\r'])
                || Path::new(&p.name)
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
            {
                return Err(invalid_model("unsafe image name"));
            }
            if !names.insert(&p.name)
                || !cameras.contains_key(&p.camera_id)
                || p.qvec.iter().chain(p.tvec.iter()).any(|v| !v.is_finite())
                || (p.qvec.iter().map(|v| v * v).sum::<f64>() - 1.).abs() > 1e-5
            {
                return Err(invalid_model("invalid camera reference/pose"));
            }
            for (index, o) in image.observations.iter().enumerate() {
                if !o.x.is_finite() || !o.y.is_finite() {
                    return Err(invalid_model("invalid observation"));
                }
                if let Some(id) = o.point {
                    if !points
                        .get(&id)
                        .is_some_and(|p| p.track.contains(&(image.pose.id, index as u32)))
                    {
                        return Err(invalid_model("missing reciprocal observation"));
                    }
                }
            }
        }
        for point in &self.points {
            if point.xyz.iter().any(|v| !v.is_finite())
                || !point.error.is_finite()
                || point.error < 0.
            {
                return Err(invalid_model("invalid sparse point/error"));
            }
            let mut unique = HashSet::new();
            for &(id, index) in &point.track {
                if !unique.insert((id, index))
                    || !images
                        .get(&id)
                        .and_then(|i| i.observations.get(index as usize))
                        .is_some_and(|o| o.point == Some(point.id))
                {
                    return Err(invalid_model("invalid reciprocal track"));
                }
            }
        }
        Ok(())
    }
    pub fn write(&self, path: &Path) -> Result<()> {
        use std::io::{BufWriter, Write};
        std::fs::create_dir_all(path)?;
        let mut w = BufWriter::new(File::create(path.join("cameras.bin"))?);
        w.write_all(&(self.cameras.len() as u64).to_le_bytes())?;
        for c in &self.cameras {
            w.write_all(&c.id.to_le_bytes())?;
            w.write_all(&c.model_id.to_le_bytes())?;
            w.write_all(&c.width.to_le_bytes())?;
            w.write_all(&c.height.to_le_bytes())?;
            for p in &c.params {
                w.write_all(&p.to_le_bytes())?;
            }
        }
        w.flush()?;
        let mut w = BufWriter::new(File::create(path.join("images.bin"))?);
        w.write_all(&(self.images.len() as u64).to_le_bytes())?;
        for i in &self.images {
            w.write_all(&i.pose.id.to_le_bytes())?;
            for v in i.pose.qvec.iter().chain(i.pose.tvec.iter()) {
                w.write_all(&v.to_le_bytes())?;
            }
            w.write_all(&i.pose.camera_id.to_le_bytes())?;
            w.write_all(i.pose.name.as_bytes())?;
            w.write_all(&[0])?;
            w.write_all(&(i.observations.len() as u64).to_le_bytes())?;
            for o in &i.observations {
                w.write_all(&o.x.to_le_bytes())?;
                w.write_all(&o.y.to_le_bytes())?;
                w.write_all(&o.point.unwrap_or(u64::MAX).to_le_bytes())?;
            }
        }
        w.flush()?;
        let mut w = BufWriter::new(File::create(path.join("points3D.bin"))?);
        w.write_all(&(self.points.len() as u64).to_le_bytes())?;
        for p in &self.points {
            w.write_all(&p.id.to_le_bytes())?;
            for v in p.xyz {
                w.write_all(&v.to_le_bytes())?;
            }
            w.write_all(&p.rgb)?;
            w.write_all(&p.error.to_le_bytes())?;
            w.write_all(&(p.track.len() as u64).to_le_bytes())?;
            for (id, index) in &p.track {
                w.write_all(&id.to_le_bytes())?;
                w.write_all(&index.to_le_bytes())?;
            }
        }
        w.flush()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub identity: String,
    pub image_count: u64,
    pub points: u64,
    pub camera_count: usize,
    pub has_erp: bool,
    pub reshoot_eligible: bool,
    #[serde(skip)]
    names: Vec<String>,
}
pub fn inspect(input: &ColmapInput) -> Result<Inspection> {
    let mut normalized = input.clone();
    if let Some(db) = &mut normalized.database {
        *db = if db.exists() {
            std::fs::canonicalize(&*db)?
        } else {
            std::path::absolute(&*db)?
        };
    }
    let input = &normalized;
    type Entry = (PathBuf, PathBuf, Option<PathBuf>, String, Inspection);
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<Entry>>> =
        std::sync::OnceLock::new();
    let key = (
        std::fs::canonicalize(&input.model)?,
        std::fs::canonicalize(&input.images)?,
        input.database.clone(),
    );
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some((_, _, _, stamp, value)) = cache
        .iter()
        .find(|(m, i, d, _, _)| m == &key.0 && i == &key.1 && d == &key.2)
    {
        if quick_stamp(input, &value.names)? == *stamp {
            if input
                .identity
                .as_ref()
                .is_some_and(|i| i != &value.identity)
            {
                return Err(SplatError::Process("INPUT_CHANGED".into()));
            }
            return Ok(value.clone());
        }
    }
    let value = inspect_impl(input)?;
    let stamp = quick_stamp(input, &value.names)?;
    if cache.len() >= 8 {
        cache.pop_front();
    }
    cache.push_back((key.0, key.1, key.2, stamp, value.clone()));
    Ok(value)
}
fn quick_stamp(input: &ColmapInput, names: &[String]) -> Result<String> {
    let mut hash = Sha256::new();
    let mut paths = super::colmap_model::reusable_model_files(&input.model)?;
    for name in [
        "cameras.bin",
        "images.bin",
        "points3D.bin",
        "cameras.txt",
        "images.txt",
        "points3D.txt",
    ] {
        let path = input.model.join(name);
        hash.update(name.as_bytes());
        if let Ok(stat) = std::fs::symlink_metadata(&path) {
            if !stat.file_type().is_file() {
                return Err(invalid());
            }
            if !paths.contains(&path) {
                paths.push(path);
            }
        } else {
            hash.update(b"missing");
        }
    }
    paths.extend(
        names
            .iter()
            .map(|n| image_path(&input.images, n))
            .collect::<Result<Vec<_>>>()?,
    );
    if let Some(db) = &input.database {
        hash.update(db.as_os_str().as_encoded_bytes());
        if db.is_file() {
            paths.push(db.clone());
        } else {
            hash.update(b"missing");
        }
    }
    for path in paths {
        let s = std::fs::metadata(&path)?;
        hash.update(path.as_os_str().as_encoded_bytes());
        hash.update(s.len().to_le_bytes());
        hash.update(
            s.modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid())?
                .as_nanos()
                .to_le_bytes(),
        );
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn inspect_impl(input: &ColmapInput) -> Result<Inspection> {
    let model_stamp = quick_stamp(input, &[])?;
    let model = read_model(&input.model)?;
    if quick_stamp(input, &[])? != model_stamp {
        return Err(SplatError::Process("INPUT_CHANGED".into()));
    }
    let names = model
        .images
        .iter()
        .map(|i| i.pose.name.clone())
        .collect::<Vec<_>>();
    let before = quick_stamp(input, &names)?;
    let mut hash = Sha256::new();
    if model.images.len() < 2 {
        return Err(invalid());
    }
    for camera in &model.cameras {
        if camera
            .width
            .checked_mul(camera.height)
            .and_then(|n| n.checked_mul(4))
            .is_none_or(|n| n > crate::video::panorama::MAX_DECODE_BYTES)
        {
            return Err(invalid());
        }
        if ![0, 1, 2, 3, 4, 6, 17].contains(&camera.model_id)
            || camera
                .params
                .iter()
                .take(if [1, 4, 6].contains(&camera.model_id) {
                    2
                } else {
                    1
                })
                .any(|v| *v <= 0.)
        {
            return Err(invalid());
        }
        if camera.model_id == 17
            && (camera.width != camera.height.checked_mul(2).ok_or_else(invalid)?
                || camera.params != vec![camera.width as f64, camera.height as f64])
        {
            return Err(invalid());
        }
    }
    for file in super::colmap_model::reusable_model_files(&input.model)? {
        hash.update(std::fs::read(file)?);
    }
    for name in ["cameras.txt", "images.txt", "points3D.txt"] {
        let path = input.model.join(name);
        if path.is_file() {
            hash.update(std::fs::read(path)?);
        }
    }
    for image in &model.images {
        let path = image_path(&input.images, &image.pose.name)?;
        let header = crate::video::image_projection::inspect(&path)?;
        let camera = model
            .cameras
            .iter()
            .find(|c| c.id == image.pose.camera_id)
            .ok_or_else(invalid)?;
        if header.orientation != 1
            || (camera.model_id == 17 && header.unsupported)
            || (u64::from(header.width), u64::from(header.height)) != (camera.width, camera.height)
        {
            return Err(invalid());
        }
        let metadata = std::fs::metadata(&path)?;
        hash.update(path.as_os_str().as_encoded_bytes());
        hash.update(metadata.len().to_le_bytes());
        hash.update(
            metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid())?
                .as_nanos()
                .to_le_bytes(),
        );
    }
    if let Some(path) = &input.database {
        if let Ok(s) = std::fs::metadata(path) {
            hash.update(path.as_os_str().as_encoded_bytes());
            hash.update(s.len().to_le_bytes());
            hash.update(
                s.modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| invalid())?
                    .as_nanos()
                    .to_le_bytes(),
            );
        }
    }
    let identity = format!("{:x}", hash.finalize());
    if input.identity.as_ref().is_some_and(|old| old != &identity) {
        return Err(SplatError::Process("INPUT_CHANGED".into()));
    }
    // A database must preserve feature observation indices, not merely file names.
    let reshoot_eligible = input
        .database
        .as_ref()
        .is_some_and(|db| database_matches(db, &model).unwrap_or(false));
    if quick_stamp(input, &names)? != before {
        return Err(SplatError::Process("INPUT_CHANGED".into()));
    }
    Ok(Inspection {
        identity,
        image_count: model.images.len() as u64,
        points: model.points.len() as u64,
        camera_count: model.cameras.len(),
        has_erp: model.images.iter().any(|i| {
            model
                .cameras
                .iter()
                .any(|c| c.id == i.pose.camera_id && c.model_id == 17)
        }),
        reshoot_eligible,
        names,
    })
}
fn database_matches(path: &Path, model: &Model) -> Result<bool> {
    let db = open_immutable_database(path)?;
    for camera in &model.cameras {
        let row = db
            .query_row(
                "SELECT model,width,height FROM cameras WHERE camera_id=?1",
                [camera.id],
                |r| {
                    Ok((
                        r.get::<_, i32>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, u64>(2)?,
                    ))
                },
            )
            .map_err(database_read_error)?;
        if row != (camera.model_id, camera.width, camera.height) {
            return Ok(false);
        }
    }
    for i in &model.images {
        // The direct-training copy stores absolute image poses as a flat model.
        // Only certify a database whose trivial rigs/frames use that same ID
        // mapping. Complex capture rigs can still train, but are not promised
        // safe reshoot caches merely because their feature rows match.
        let frame:(u32,u32,u32,u32,u32,u32)=db.query_row("SELECT d.frame_id,d.sensor_id,d.sensor_type,f.rig_id,r.ref_sensor_id,r.ref_sensor_type FROM frame_data d JOIN frames f USING(frame_id) JOIN rigs r USING(rig_id) WHERE d.data_id=?1 AND d.sensor_type=0",[i.pose.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).map_err(database_read_error)?;
        if frame
            != (
                i.pose.id,
                i.pose.camera_id,
                0,
                i.pose.camera_id,
                i.pose.camera_id,
                0,
            )
        {
            return Ok(false);
        }
        let entries: u64 = db
            .query_row(
                "SELECT COUNT(*) FROM frame_data WHERE frame_id=?1",
                [frame.0],
                |r| r.get(0),
            )
            .map_err(database_read_error)?;
        let additional_sensors: u64 = db
            .query_row(
                "SELECT COUNT(*) FROM rig_sensors WHERE rig_id=?1",
                [frame.3],
                |r| r.get(0),
            )
            .map_err(database_read_error)?;
        if entries != 1 || additional_sensors != 0 {
            return Ok(false);
        }
        let row=db.query_row("SELECT images.name, images.camera_id, keypoints.rows FROM images JOIN keypoints USING(image_id) WHERE image_id=?1",[i.pose.id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u32>(1)?,r.get::<_,usize>(2)?))).map_err(database_read_error)?;
        if row != (i.pose.name.clone(), i.pose.camera_id, i.observations.len()) {
            return Ok(false);
        }
        let (columns, data): (usize, Option<Vec<u8>>) = db
            .query_row(
                "SELECT cols,data FROM keypoints WHERE image_id=?1",
                [i.pose.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(database_read_error)?;
        let data = data.unwrap_or_default();
        if !(2..=6).contains(&columns)
            || data.len()
                != i.observations
                    .len()
                    .checked_mul(columns)
                    .and_then(|n| n.checked_mul(4))
                    .ok_or_else(invalid)?
        {
            return Ok(false);
        }
        for (point, bytes) in i.observations.iter().zip(data.chunks_exact(columns * 4)) {
            let x = f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64;
            let y = f32::from_le_bytes(bytes[4..8].try_into().unwrap()) as f64;
            if !x.is_finite()
                || !y.is_finite()
                || (x - point.x).abs() > 0.001
                || (y - point.y).abs() > 0.001
            {
                return Ok(false);
            }
        }
        let (rows, columns, bytes): (usize, usize, Option<usize>) = db
            .query_row(
                "SELECT rows,cols,length(data) FROM descriptors WHERE image_id=?1",
                [i.pose.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(database_read_error)?;
        if rows != i.observations.len()
            || columns != 128
            || bytes.unwrap_or(0) != rows.checked_mul(128).ok_or_else(invalid)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LegacySharedFramesValidation {
    pub image_count: u64,
    pub registered_images: u64,
    pub content_identity: [u8; 32],
}

/// Certifies the pre-split layout where COLMAP read the project's `frames`
/// directory directly. This is deliberately stricter than ordinary project
/// recovery: it is used only to decide whether an old feature database can be
/// copied into a new, independently-owned reshoot project.
pub(crate) fn validate_legacy_shared_frames(
    frames: &Path,
    database: &Path,
    model_path: &Path,
    expected_count: u64,
) -> Result<LegacySharedFramesValidation> {
    if expected_count == 0 {
        return Err(invalid_model("legacy frame count is zero"));
    }
    let model = read_model(model_path)?;
    if model.cameras.len() != 1 || model.cameras[0].model_id == 17 {
        return Err(invalid_model(
            "legacy shared frames require one ordinary camera",
        ));
    }
    let camera = &model.cameras[0];

    let canonical_root = std::fs::canonicalize(frames)?;
    if !canonical_root.is_dir() {
        return Err(invalid_model("legacy frames root is not a directory"));
    }
    let frame_paths = crate::video::list_images(frames)?;
    if frame_paths.len() as u64 != expected_count {
        return Err(invalid_model("legacy frame count differs from checkpoint"));
    }
    let mut frame_names = BTreeSet::new();
    for path in &frame_paths {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_file() || metadata.len() == 0 {
            return Err(invalid_model("legacy frame is missing or redirected"));
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid_model("legacy frame name is not valid UTF-8"))?;
        if Path::new(name).components().count() != 1 || !frame_names.insert(name.to_owned()) {
            return Err(invalid_model("legacy frame name is unsafe or duplicated"));
        }
        let resolved = image_path(&canonical_root, name)?;
        if resolved.parent() != Some(canonical_root.as_path()) {
            return Err(invalid_model("legacy frame escapes the flat image root"));
        }
        let header = crate::video::image_projection::inspect(&resolved)?;
        if header.alpha
            || header.orientation != 1
            || header.complete_erp
            || header.unsupported
            || (u64::from(header.width), u64::from(header.height)) != (camera.width, camera.height)
        {
            return Err(invalid_model(
                "legacy frame projection, alpha, orientation, or size is incompatible",
            ));
        }
    }

    // Immutable SQLite access must not silently ignore uncheckpointed data.
    // A completed legacy source is accepted only when its base database is
    // self-contained; empty stale sidecars are harmless.
    ensure_legacy_database_has_no_pending_writes(database)?;
    let db = open_immutable_database(database)?;
    let database_camera_count: u64 = db
        .query_row("SELECT COUNT(*) FROM cameras", [], |row| row.get(0))
        .map_err(database_read_error)?;
    if database_camera_count != 1 {
        return Err(invalid_model(
            "legacy COLMAP database does not contain one camera",
        ));
    }
    let database_camera: (i32, u64, u64) = db
        .query_row(
            "SELECT model,width,height FROM cameras WHERE camera_id=?1",
            [camera.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(database_read_error)?;
    if database_camera != (camera.model_id, camera.width, camera.height) {
        return Err(invalid_model(
            "legacy database and sparse camera geometry differ",
        ));
    }

    let mut query = db
        .prepare("SELECT image_id,name,camera_id FROM images ORDER BY image_id")
        .map_err(database_read_error)?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(database_read_error)?;
    let mut database_images = Vec::new();
    for row in rows {
        database_images.push(row.map_err(database_read_error)?);
    }
    drop(query);
    if database_images.len() as u64 != expected_count {
        return Err(invalid_model(
            "legacy database image count differs from checkpoint",
        ));
    }
    for table in ["keypoints", "descriptors"] {
        let sql = format!("SELECT COUNT(*),COUNT(DISTINCT image_id) FROM {table}");
        let (rows, unique_images): (u64, u64) = db
            .query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(database_read_error)?;
        if rows != expected_count || unique_images != expected_count {
            return Err(invalid_model(
                "legacy feature table does not contain exactly one row per image",
            ));
        }
    }

    let registered = model
        .images
        .iter()
        .map(|image| (image.pose.id, image))
        .collect::<BTreeMap<_, _>>();
    let mut database_names = BTreeSet::new();
    let mut database_ids = BTreeSet::new();
    for (raw_id, name, raw_camera_id) in database_images {
        let image_id = u32::try_from(raw_id)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid_model("legacy database image id is invalid"))?;
        let camera_id = u32::try_from(raw_camera_id)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid_model("legacy database camera id is invalid"))?;
        if camera_id != camera.id
            || !database_ids.insert(image_id)
            || !database_names.insert(name.clone())
            || !frame_names.contains(&name)
        {
            return Err(invalid_model(
                "legacy database image identity differs from frames",
            ));
        }
        let path = image_path(&canonical_root, &name)?;
        if path.parent() != Some(canonical_root.as_path()) {
            return Err(invalid_model("legacy database image path is not flat"));
        }

        let frame: (u32, u32, u32, u32, u32, u32) = db
            .query_row(
                "SELECT d.frame_id,d.sensor_id,d.sensor_type,f.rig_id,r.ref_sensor_id,r.ref_sensor_type FROM frame_data d JOIN frames f USING(frame_id) JOIN rigs r USING(rig_id) WHERE d.data_id=?1 AND d.sensor_type=0",
                [image_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .map_err(database_read_error)?;
        if frame != (image_id, camera_id, 0, camera_id, camera_id, 0) {
            return Err(invalid_model("legacy image/frame mapping differs"));
        }
        let frame_entries: u64 = db
            .query_row(
                "SELECT COUNT(*) FROM frame_data WHERE frame_id=?1",
                [frame.0],
                |row| row.get(0),
            )
            .map_err(database_read_error)?;
        let additional_sensors: u64 = db
            .query_row(
                "SELECT COUNT(*) FROM rig_sensors WHERE rig_id=?1",
                [frame.3],
                |row| row.get(0),
            )
            .map_err(database_read_error)?;
        if frame_entries != 1 || additional_sensors != 0 {
            return Err(invalid_model("legacy frame uses an unsupported rig"));
        }

        let (keypoint_rows, keypoint_columns, keypoint_data): (i64, i64, Option<Vec<u8>>) = db
            .query_row(
                "SELECT rows,cols,data FROM keypoints WHERE image_id=?1",
                [image_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(database_read_error)?;
        let keypoint_rows = usize::try_from(keypoint_rows)
            .map_err(|_| invalid_model("legacy keypoint count is invalid"))?;
        let keypoint_columns = usize::try_from(keypoint_columns)
            .map_err(|_| invalid_model("legacy keypoint width is invalid"))?;
        if !(2..=6).contains(&keypoint_columns) {
            return Err(invalid_model("legacy keypoint width is unsupported"));
        }
        let expected_keypoint_bytes = keypoint_rows
            .checked_mul(keypoint_columns)
            .and_then(|value| value.checked_mul(std::mem::size_of::<f32>()))
            .ok_or_else(|| invalid_model("legacy keypoint data is too large"))?;
        let keypoint_data = keypoint_data.unwrap_or_default();
        if keypoint_data.len() != expected_keypoint_bytes {
            return Err(invalid_model("legacy keypoint data length differs"));
        }

        let (descriptor_rows, descriptor_columns, descriptor_bytes): (i64, i64, Option<i64>) = db
            .query_row(
                "SELECT rows,cols,length(data) FROM descriptors WHERE image_id=?1",
                [image_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(database_read_error)?;
        let descriptor_rows = usize::try_from(descriptor_rows)
            .map_err(|_| invalid_model("legacy descriptor count is invalid"))?;
        let descriptor_columns = usize::try_from(descriptor_columns)
            .map_err(|_| invalid_model("legacy descriptor width is invalid"))?;
        let expected_descriptor_bytes = descriptor_rows
            .checked_mul(descriptor_columns)
            .ok_or_else(|| invalid_model("legacy descriptor data is too large"))?;
        let descriptor_bytes: usize = descriptor_bytes
            .unwrap_or(0)
            .try_into()
            .map_err(|_| invalid_model("legacy descriptor data length is invalid"))?;
        if descriptor_rows != keypoint_rows
            || descriptor_columns != 128
            || descriptor_bytes != expected_descriptor_bytes
        {
            return Err(invalid_model("legacy descriptors do not match keypoints"));
        }

        if let Some(model_image) = registered.get(&image_id) {
            if model_image.pose.name != name
                || model_image.pose.camera_id != camera_id
                || model_image.observations.len() != keypoint_rows
            {
                return Err(invalid_model(
                    "legacy sparse image identity differs from database",
                ));
            }
            for (observation, bytes) in model_image
                .observations
                .iter()
                .zip(keypoint_data.chunks_exact(keypoint_columns * 4))
            {
                let x = f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64;
                let y = f32::from_le_bytes(bytes[4..8].try_into().unwrap()) as f64;
                if !x.is_finite()
                    || !y.is_finite()
                    || (x - observation.x).abs() > 0.001
                    || (y - observation.y).abs() > 0.001
                {
                    return Err(invalid_model(
                        "legacy sparse observations differ from keypoints",
                    ));
                }
            }
        }
    }
    if database_names != frame_names || registered.keys().any(|id| !database_ids.contains(id)) {
        return Err(invalid_model(
            "legacy frames, database, and sparse model do not match",
        ));
    }
    drop(db);
    let mut content_identity = Sha256::new();
    for path in &frame_paths {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid_model("legacy frame name is not valid UTF-8"))?;
        update_legacy_content_identity(&mut content_identity, b"frame", name, path)?;
    }
    update_legacy_content_identity(&mut content_identity, b"database", "database.db", database)?;
    let mut model_files = super::colmap_model::reusable_model_files(model_path)?;
    model_files.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    for path in model_files {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid_model("legacy model file name is not valid UTF-8"))?;
        update_legacy_content_identity(&mut content_identity, b"model", name, &path)?;
    }
    Ok(LegacySharedFramesValidation {
        image_count: expected_count,
        registered_images: model.images.len() as u64,
        content_identity: content_identity.finalize().into(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn fixture(root: &Path, erp: bool) -> ColmapInput {
        let images = root.join("images");
        let model = root.join("model");
        std::fs::create_dir_all(&images).unwrap();
        let camera = ColmapCamera {
            id: 7,
            model_id: 1,
            model: "PINHOLE".into(),
            width: 64,
            height: 48,
            params: vec![50., 50., 32., 24.],
        };
        let second = if erp {
            ColmapCamera {
                id: 9,
                model_id: 17,
                model: "EQUIRECTANGULAR".into(),
                width: 128,
                height: 64,
                params: vec![128., 64.],
            }
        } else {
            camera.clone()
        };
        let mut rgba = image::RgbaImage::from_fn(64, 48, |x, y| {
            image::Rgba([
                (x * 3) as u8,
                (y * 4) as u8,
                120,
                if x < 16 { 0 } else { 255 },
            ])
        });
        rgba.put_pixel(32, 24, image::Rgba([128, 128, 128, 255]));
        rgba.save(images.join("normal.png")).unwrap();
        image::RgbImage::from_fn(second.width as u32, second.height as u32, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 40])
        })
        .save(images.join("second.png"))
        .unwrap();
        let points = vec![Point {
            id: 100,
            xyz: [0., 0., 5.],
            rgb: [128; 3],
            error: 0.,
            track: vec![(42, 0), (5, 0)],
        }];
        let images = vec![
            ModelImage {
                pose: PosedImage {
                    id: 42,
                    camera_id: 7,
                    name: "normal.png".into(),
                    qvec: [1., 0., 0., 0.],
                    tvec: [0.; 3],
                },
                observations: vec![Observation {
                    x: 32.,
                    y: 24.,
                    point: Some(100),
                }],
            },
            ModelImage {
                pose: PosedImage {
                    id: 5,
                    camera_id: second.id,
                    name: "second.png".into(),
                    qvec: [1., 0., 0., 0.],
                    tvec: [0.; 3],
                },
                observations: vec![Observation {
                    x: second.width as f64 / 2.,
                    y: second.height as f64 / 2.,
                    point: Some(100),
                }],
            },
        ];
        Model {
            cameras: if erp {
                vec![camera, second]
            } else {
                vec![camera]
            },
            images,
            points,
        }
        .write(&model)
        .unwrap();
        ColmapInput {
            model,
            images: root.join("images"),
            database: None,
            identity: None,
        }
    }

    pub(crate) fn legacy_shared_frames_fixture(root: &Path) -> (ColmapInput, PathBuf) {
        let input = fixture(root, false);
        image::RgbImage::from_pixel(64, 48, image::Rgb([1, 2, 3]))
            .save(input.images.join("normal.png"))
            .unwrap();
        image::RgbImage::from_pixel(64, 48, image::Rgb([4, 5, 6]))
            .save(input.images.join("unregistered.png"))
            .unwrap();
        let model = read_model(&input.model).unwrap();
        let path = root.join("legacy.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE cameras(camera_id INTEGER,model INTEGER,width INTEGER,height INTEGER);CREATE TABLE images(image_id INTEGER,name TEXT,camera_id INTEGER);CREATE TABLE keypoints(image_id INTEGER,rows INTEGER,cols INTEGER,data BLOB);CREATE TABLE descriptors(image_id INTEGER,rows INTEGER,cols INTEGER,data BLOB);CREATE TABLE rigs(rig_id INTEGER,ref_sensor_id INTEGER,ref_sensor_type INTEGER);CREATE TABLE rig_sensors(rig_id INTEGER);CREATE TABLE frames(frame_id INTEGER,rig_id INTEGER);CREATE TABLE frame_data(frame_id INTEGER,data_id INTEGER,sensor_id INTEGER,sensor_type INTEGER);").unwrap();
        let camera = &model.cameras[0];
        db.execute(
            "INSERT INTO cameras VALUES(?1,?2,?3,?4)",
            rusqlite::params![camera.id, camera.model_id, camera.width, camera.height],
        )
        .unwrap();
        db.execute("INSERT INTO rigs VALUES(?1,?1,0)", [camera.id])
            .unwrap();
        for image in &model.images {
            let xy = image
                .observations
                .iter()
                .flat_map(|observation| [observation.x as f32, observation.y as f32])
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>();
            db.execute(
                "INSERT INTO images VALUES(?1,?2,?3)",
                rusqlite::params![image.pose.id, image.pose.name, image.pose.camera_id],
            )
            .unwrap();
            db.execute(
                "INSERT INTO keypoints VALUES(?1,?2,2,?3)",
                rusqlite::params![image.pose.id, image.observations.len(), xy],
            )
            .unwrap();
            db.execute(
                "INSERT INTO descriptors VALUES(?1,?2,128,?3)",
                rusqlite::params![
                    image.pose.id,
                    image.observations.len(),
                    vec![0u8; image.observations.len() * 128]
                ],
            )
            .unwrap();
            db.execute(
                "INSERT INTO frames VALUES(?1,?2)",
                rusqlite::params![image.pose.id, image.pose.camera_id],
            )
            .unwrap();
            db.execute(
                "INSERT INTO frame_data VALUES(?1,?1,?2,0)",
                rusqlite::params![image.pose.id, image.pose.camera_id],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO images VALUES(99,'unregistered.png',?1)",
            [camera.id],
        )
        .unwrap();
        db.execute(
            "INSERT INTO keypoints VALUES(99,0,2,?1)",
            [Vec::<u8>::new()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO descriptors VALUES(99,0,128,?1)",
            [Vec::<u8>::new()],
        )
        .unwrap();
        db.execute("INSERT INTO frames VALUES(99,?1)", [camera.id])
            .unwrap();
        db.execute("INSERT INTO frame_data VALUES(99,99,?1,0)", [camera.id])
            .unwrap();
        drop(db);
        (input, path)
    }

    #[test]
    fn legacy_shared_frames_require_exact_database_model_and_image_mapping() {
        let root = tempfile::tempdir().unwrap();
        let (input, database) = legacy_shared_frames_fixture(root.path());
        let validation =
            validate_legacy_shared_frames(&input.images, &database, &input.model, 3).unwrap();
        assert_eq!(validation.image_count, 3);
        assert_eq!(validation.registered_images, 2);
        assert_ne!(validation.content_identity, [0; 32]);

        image::RgbImage::from_pixel(64, 48, image::Rgb([9, 8, 7]))
            .save(input.images.join("normal.png"))
            .unwrap();
        let changed =
            validate_legacy_shared_frames(&input.images, &database, &input.model, 3).unwrap();
        assert_ne!(validation.content_identity, changed.content_identity);

        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute(
            "UPDATE keypoints SET data=?1 WHERE image_id=42",
            [vec![0_u8; 8]],
        )
        .unwrap();
        drop(db);
        assert!(validate_legacy_shared_frames(&input.images, &database, &input.model, 3).is_err());
    }

    #[test]
    fn legacy_shared_frames_reject_alpha_names_dimensions_and_feature_corruption() {
        let root = tempfile::tempdir().unwrap();
        let (input, database) = legacy_shared_frames_fixture(root.path());
        image::RgbaImage::from_pixel(64, 48, image::Rgba([1, 2, 3, 255]))
            .save(input.images.join("normal.png"))
            .unwrap();
        assert!(validate_legacy_shared_frames(&input.images, &database, &input.model, 3).is_err());

        let root = tempfile::tempdir().unwrap();
        let (input, database) = legacy_shared_frames_fixture(root.path());
        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute("UPDATE images SET name='wrong.png' WHERE image_id=99", [])
            .unwrap();
        drop(db);
        assert!(validate_legacy_shared_frames(&input.images, &database, &input.model, 3).is_err());

        let root = tempfile::tempdir().unwrap();
        let (input, database) = legacy_shared_frames_fixture(root.path());
        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute("UPDATE cameras SET width=63", []).unwrap();
        drop(db);
        assert!(validate_legacy_shared_frames(&input.images, &database, &input.model, 3).is_err());

        let root = tempfile::tempdir().unwrap();
        let (input, database) = legacy_shared_frames_fixture(root.path());
        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute(
            "UPDATE descriptors SET rows=0,data=?1 WHERE image_id=42",
            [Vec::<u8>::new()],
        )
        .unwrap();
        drop(db);
        assert!(validate_legacy_shared_frames(&input.images, &database, &input.model, 3).is_err());

        let root = tempfile::tempdir().unwrap();
        let (input, database) = legacy_shared_frames_fixture(root.path());
        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute("UPDATE frame_data SET sensor_id=99 WHERE data_id=42", [])
            .unwrap();
        drop(db);
        assert!(validate_legacy_shared_frames(&input.images, &database, &input.model, 3).is_err());

        let root = tempfile::tempdir().unwrap();
        let (input, database) = legacy_shared_frames_fixture(root.path());
        let db = rusqlite::Connection::open(&database).unwrap();
        db.execute(
            "INSERT INTO keypoints SELECT image_id,rows,cols,data FROM keypoints WHERE image_id=42",
            [],
        )
        .unwrap();
        drop(db);
        assert!(validate_legacy_shared_frames(&input.images, &database, &input.model, 3).is_err());

        let root = tempfile::tempdir().unwrap();
        let (input, database) = legacy_shared_frames_fixture(root.path());
        let mut wal = database.as_os_str().to_os_string();
        wal.push("-wal");
        std::fs::write(PathBuf::from(wal), b"pending").unwrap();
        assert!(validate_legacy_shared_frames(&input.images, &database, &input.model, 3).is_err());
    }

    #[test]
    fn immutable_database_uri_supports_unicode_and_spaces_without_sidecars() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("资料 space");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("database.db");
        let writable = rusqlite::Connection::open(&path).unwrap();
        writable
            .execute_batch("CREATE TABLE value(number INTEGER); INSERT INTO value VALUES(7);")
            .unwrap();
        drop(writable);
        let readonly = open_immutable_database(&path).unwrap();
        assert_eq!(
            readonly
                .query_row("SELECT number FROM value", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            7
        );
        assert!(!directory.join("database.db-wal").exists());
        assert!(!directory.join("database.db-shm").exists());
    }
    #[test]
    fn imported_binary_model_keeps_pose_ids_observations_and_initial_points() {
        let root = tempfile::tempdir().unwrap();
        let input = fixture(root.path(), true);
        let a = inspect(&input).unwrap();
        assert_eq!((a.image_count, a.camera_count, a.points), (2, 2, 1));
        assert!(a.has_erp);
        assert!(!a.reshoot_eligible);
        let model = read_model(&input.model).unwrap();
        assert_eq!(model.images[0].pose.id, 42);
        assert_eq!(model.points[0].track, vec![(42, 0), (5, 0)]);
        let copy = root.path().join("copy");
        model.write(&copy).unwrap();
        for name in ["cameras.bin", "images.bin", "points3D.bin"] {
            assert_eq!(
                std::fs::read(copy.join(name)).unwrap(),
                std::fs::read(input.model.join(name)).unwrap()
            );
        }
    }
    #[test]
    fn imported_validation_rejects_missing_dimensions_tracks_poses_and_unsupported_camera() {
        let root = tempfile::tempdir().unwrap();
        let input = fixture(root.path(), false);
        let mut model = read_model(&input.model).unwrap();
        model.images[0].observations[0].point = Some(999);
        model.write(&input.model).unwrap();
        assert!(inspect(&input).is_err());
        let input = fixture(root.path(), false);
        let mut model = read_model(&input.model).unwrap();
        model.images[0].pose.qvec = [2., 0., 0., 0.];
        model.write(&input.model).unwrap();
        assert!(inspect(&input).is_err());
        let input = fixture(root.path(), false);
        image::RgbImage::new(63, 48)
            .save(input.images.join("normal.png"))
            .unwrap();
        assert!(inspect(&input).is_err());
        let input = fixture(root.path(), false);
        let mut model = read_model(&input.model).unwrap();
        model.cameras[0].model_id = 5;
        model.cameras[0].model = "OPENCV_FISHEYE".into();
        model.cameras[0].params = vec![50., 50., 32., 24., 0., 0., 0., 0.];
        model.write(&input.model).unwrap();
        assert!(inspect(&input).is_err());
    }
    #[test]
    fn imported_input_identity_is_revalidated_without_requiring_a_database() {
        let root = tempfile::tempdir().unwrap();
        let mut input = fixture(root.path(), false);
        input.database = Some(root.path().join("missing.db"));
        let before = inspect(&input).unwrap();
        input.identity = Some(before.identity.clone());
        assert_eq!(inspect(&input).unwrap().identity, before.identity);
        image::RgbaImage::new(64, 48)
            .save(input.images.join("normal.png"))
            .unwrap();
        assert!(inspect(&input)
            .unwrap_err()
            .to_string()
            .contains("INPUT_CHANGED"));
    }
    #[test]
    fn imported_paths_and_corrupt_counts_cannot_escape_or_allocate_from_untrusted_counts() {
        let root = tempfile::tempdir().unwrap();
        let input = fixture(root.path(), false);
        for name in [
            "../outside.png",
            "/outside.png",
            "C:/outside.png",
            "sub/../../outside.png",
            "sub\\image.png",
        ] {
            assert!(image_path(&input.images, name).is_err());
        }
        std::fs::write(input.model.join("images.bin"), u64::MAX.to_le_bytes()).unwrap();
        assert!(read_model(&input.model).is_err());
    }
    #[test]
    fn imported_text_model_uses_complete_groups_and_validates_reciprocal_tracks() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        std::fs::write(dir.join("cameras.txt"), "7 PINHOLE 64 48 50 50 32 24\n").unwrap();
        std::fs::write(
            dir.join("images.txt"),
            "42 1 0 0 0 0 0 0 7 normal.png\n32 24 100\n5 1 0 0 0 0 0 0 7 second.png\n32 24 100\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("points3D.txt"),
            "100 0 0 5 128 128 128 0 42 0 5 0\n",
        )
        .unwrap();
        assert_eq!(read_model(dir).unwrap().images.len(), 2);
        std::fs::write(dir.join("images.bin"), [0; 8]).unwrap();
        assert!(read_model(dir).is_err());
    }
    #[test]
    fn conflicting_complete_text_and_binary_models_are_not_silently_selected() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        std::fs::write(dir.join("cameras.txt"), "7 PINHOLE 64 48 50 50 32 24\n").unwrap();
        std::fs::write(
            dir.join("images.txt"),
            "42 1 0 0 0 0 0 0 7 normal.png\n32 24 100\n5 1 0 0 0 0 0 0 7 second.png\n32 24 100\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("points3D.txt"),
            "100 0 0 5 128 128 128 0 42 0 5 0\n",
        )
        .unwrap();
        read_model(dir).unwrap().write(dir).unwrap();
        assert_eq!(read_model(dir).unwrap().images.len(), 2);
        std::fs::write(dir.join("cameras.txt"), "7 PINHOLE 64 48 55 50 32 24\n").unwrap();
        assert!(read_model(dir).is_err());
    }
    #[test]
    fn a_track_may_reference_distinct_observations_in_one_image_but_not_duplicate_an_element() {
        let root = tempfile::tempdir().unwrap();
        let input = fixture(root.path(), false);
        let mut model = read_model(&input.model).unwrap();
        let observation = model.images[0].observations[0].clone();
        model.images[0].observations.push(observation);
        model.points[0].track.push((model.images[0].pose.id, 1));
        assert!(model.validate().is_ok());
        model.points[0].track.push((model.images[0].pose.id, 1));
        assert!(model.validate().is_err());
    }
    #[test]
    fn unused_spherical_camera_does_not_claim_panorama_training() {
        let root = tempfile::tempdir().unwrap();
        let input = fixture(root.path(), false);
        let mut model = read_model(&input.model).unwrap();
        model.cameras.push(ColmapCamera {
            id: 99,
            model_id: 17,
            model: "EQUIRECTANGULAR".into(),
            width: 128,
            height: 64,
            params: vec![128., 64.],
        });
        model.write(&input.model).unwrap();
        assert!(!inspect(&input).unwrap().has_erp);
    }
    #[test]
    fn database_eligibility_checks_features_and_flat_frame_mapping_without_blocking_training() {
        let root = tempfile::tempdir().unwrap();
        let mut input = fixture(root.path(), false);
        let model = read_model(&input.model).unwrap();
        let path = root.path().join("database.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE cameras(camera_id INTEGER,model INTEGER,width INTEGER,height INTEGER);CREATE TABLE images(image_id INTEGER,name TEXT,camera_id INTEGER);CREATE TABLE keypoints(image_id INTEGER,rows INTEGER,cols INTEGER,data BLOB);CREATE TABLE descriptors(image_id INTEGER,rows INTEGER,cols INTEGER,data BLOB);CREATE TABLE rigs(rig_id INTEGER,ref_sensor_id INTEGER,ref_sensor_type INTEGER);CREATE TABLE rig_sensors(rig_id INTEGER);CREATE TABLE frames(frame_id INTEGER,rig_id INTEGER);CREATE TABLE frame_data(frame_id INTEGER,data_id INTEGER,sensor_id INTEGER,sensor_type INTEGER);").unwrap();
        for c in &model.cameras {
            db.execute(
                "INSERT INTO cameras VALUES(?1,?2,?3,?4)",
                rusqlite::params![c.id, c.model_id, c.width, c.height],
            )
            .unwrap();
            db.execute("INSERT INTO rigs VALUES(?1,?1,0)", [c.id])
                .unwrap();
        }
        for i in &model.images {
            db.execute(
                "INSERT INTO images VALUES(?1,?2,?3)",
                rusqlite::params![i.pose.id, i.pose.name, i.pose.camera_id],
            )
            .unwrap();
            let xy = i
                .observations
                .iter()
                .flat_map(|o| [o.x as f32, o.y as f32])
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>();
            db.execute(
                "INSERT INTO keypoints VALUES(?1,?2,2,?3)",
                rusqlite::params![i.pose.id, i.observations.len(), xy],
            )
            .unwrap();
            db.execute(
                "INSERT INTO descriptors VALUES(?1,?2,128,?3)",
                rusqlite::params![
                    i.pose.id,
                    i.observations.len(),
                    vec![0u8; i.observations.len() * 128]
                ],
            )
            .unwrap();
            db.execute(
                "INSERT INTO frames VALUES(?1,?2)",
                rusqlite::params![i.pose.id, i.pose.camera_id],
            )
            .unwrap();
            db.execute(
                "INSERT INTO frame_data VALUES(?1,?1,?2,0)",
                rusqlite::params![i.pose.id, i.pose.camera_id],
            )
            .unwrap();
        }
        assert!(database_matches(&path, &model).unwrap());
        input.database = Some(path.clone());
        assert!(inspect(&input).unwrap().reshoot_eligible);
        db.execute("UPDATE frame_data SET sensor_id=999", [])
            .unwrap();
        assert!(!database_matches(&path, &model).unwrap());
        input.identity = None;
        assert!(read_model(&input.model).is_ok());
    }
}
