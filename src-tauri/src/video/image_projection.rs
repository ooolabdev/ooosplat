//! Header-only, bounded image inspection. Never decode pixels during preflight.
use crate::{
    error::{Result, SplatError},
    video::{
        image_sequence::{list_images, ImageSequenceInfo, LARGE_SEQUENCE_WARNING_COUNT},
        panorama::ProjectionDetection,
    },
};
use image::ImageDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub const METADATA_BUDGET: usize = 64 * 1024;
pub const UNSUPPORTED: &str =
    "不支持此文件夹的全景格式。请使用同尺寸、已拼接、单目、完整 360°×180° 的 2:1 ERP 图片。";
pub const OVER_BUDGET: &str = "全景图片超出处理预算，请减少图片数量或降低档位。";
const GPANO: &[u8] = b"http://ns.google.com/photos/1.0/panorama/";
const SPHERICAL: &[u8] = b"http://ns.google.com/videos/1.0/spherical/";

#[cfg(test)]
fn header_reads() -> &'static std::sync::Mutex<BTreeMap<PathBuf, usize>> {
    static READS: std::sync::OnceLock<std::sync::Mutex<BTreeMap<PathBuf, usize>>> =
        std::sync::OnceLock::new();
    READS.get_or_init(Default::default)
}
#[cfg(test)]
pub(crate) fn header_reads_for_test(dir: &Path) -> usize {
    header_reads()
        .lock()
        .unwrap()
        .iter()
        .filter(|(p, _)| p.parent() == Some(dir))
        .map(|(_, n)| *n)
        .sum()
}

#[derive(Clone, Debug)]
pub struct InspectedImage {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub alpha: bool,
    pub orientation: u8,
    pub complete_erp: bool,
    pub unsupported: bool,
}
#[derive(Clone, Debug)]
pub struct ImageFolderAnalysis {
    pub info: ImageSequenceInfo,
    pub detection: ProjectionDetection,
    pub images: Vec<InspectedImage>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageSourceMapping {
    pub source_image_index: u64,
    pub source_name: String,
    pub source_sha256: String,
    pub orientation: u8,
    pub prepared_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<OriginalImageIdentity>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OriginalImageIdentity {
    pub path: PathBuf,
    pub size: u64,
    pub modified_nanos: String,
}
fn invalid() -> SplatError {
    SplatError::InvalidVideo(UNSUPPORTED.into())
}

// Generation only: re-read complete EXIF through the decoder, since bounded
// preflight metadata may intentionally be incomplete. No pixel decode here.
fn generation_decoder(path: &Path) -> Result<impl ImageDecoder> {
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(super::panorama::MAX_DECODE_BYTES);
    reader.limits(limits);
    reader.into_decoder().map_err(|_| invalid())
}
fn generation_orientation(
    decoder: &mut impl ImageDecoder,
    image: &InspectedImage,
) -> Result<image::metadata::Orientation> {
    let actual = decoder.orientation().map_err(|_| invalid())?;
    // PNG decoders expose only pre-IDAT EXIF here. Generation can seek directly
    // to TIFF's orientation entry, even beyond preflight's metadata budget.
    let mut file = File::open(&image.path)?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    if magic == *b"\x89PNG\r\n\x1a\n" {
        let length = file.metadata()?.len();
        let mut found = None;
        for _ in 0..4096 {
            let mut head = [0; 8];
            file.read_exact(&mut head)?;
            let size = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
            let start = file.stream_position()?;
            let end = start
                .checked_add(size + 4)
                .filter(|n| *n <= length)
                .ok_or_else(invalid)?;
            if &head[4..] == b"eXIf" {
                let value = tiff_orientation_from_file(&mut file, start, size)?;
                if found.is_some_and(|old| old != value) {
                    return Err(invalid());
                }
                found = Some(value);
            }
            if &head[4..] == b"IEND" {
                return image::metadata::Orientation::from_exif(found.unwrap_or(1))
                    .ok_or_else(invalid);
            }
            file.seek(SeekFrom::Start(end))?;
        }
        return Err(invalid());
    }
    Ok(actual)
}

fn tiff_orientation_from_file(file: &mut File, start: u64, size: u64) -> Result<u8> {
    if size < 8 {
        return Err(invalid());
    }
    let mut header = [0; 8];
    file.read_exact(&mut header)?;
    let le = header.starts_with(b"II");
    if !le && !header.starts_with(b"MM") {
        return Err(invalid());
    }
    let u16_at = |v: [u8; 2]| {
        if le {
            u16::from_le_bytes(v)
        } else {
            u16::from_be_bytes(v)
        }
    };
    let u32_at = |v: [u8; 4]| {
        if le {
            u32::from_le_bytes(v)
        } else {
            u32::from_be_bytes(v)
        }
    };
    if u16_at([header[2], header[3]]) != 42 {
        return Err(invalid());
    }
    let offset = u32_at(header[4..].try_into().unwrap()) as u64;
    if offset.checked_add(2).is_none_or(|n| n > size) {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(start + offset))?;
    let mut count = [0; 2];
    file.read_exact(&mut count)?;
    let bytes = u16_at(count) as u64 * 12;
    if offset + 2 + bytes > size {
        return Err(invalid());
    }
    let mut entries = vec![0; bytes as usize];
    file.read_exact(&mut entries)?;
    for entry in entries.as_chunks::<12>().0 {
        if u16_at(entry[..2].try_into().unwrap()) == 0x112 {
            if u16_at(entry[2..4].try_into().unwrap()) != 3
                || u32_at(entry[4..8].try_into().unwrap()) != 1
            {
                return Err(invalid());
            }
            let value = u16_at(entry[8..10].try_into().unwrap());
            return if (1..=8).contains(&value) {
                Ok(value as u8)
            } else {
                Err(invalid())
            };
        }
    }
    Ok(1)
}

/// Membership, per-file size and mtime, not the directory's mtime alone.
pub fn source_identity(dir: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(std::fs::canonicalize(dir)?.as_os_str().as_encoded_bytes());
    for path in list_images(dir)? {
        let info = std::fs::metadata(&path)?;
        let name = path.file_name().ok_or_else(invalid)?.as_encoded_bytes();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update(info.len().to_le_bytes());
        hash.update(
            info.modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| invalid())?
                .as_nanos()
                .to_le_bytes(),
        );
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn analyze(dir: &Path) -> Result<ImageFolderAnalysis> {
    analyze_impl(dir, false)
}

/// Reshoot-only input inspection: never weaken ordinary generation validation.
pub fn analyze_reshoot(dir: &Path) -> Result<ImageFolderAnalysis> {
    analyze_impl(dir, true)
}

fn analyze_impl(dir: &Path, mixed_sizes: bool) -> Result<ImageFolderAnalysis> {
    let identity = source_identity(dir)?;
    let files = list_images(dir)?;
    let result = analyze_files(&files, identity.clone(), mixed_sizes)?;
    if source_identity(dir)? != identity {
        return Err(SplatError::Process(
            "素材文件已发生变化，请重新分析后再生成".into(),
        ));
    }
    Ok(result)
}

/// Shared bounded inspection for explicitly selected image sets. No staging or pixel reads.
pub fn analyze_files(
    files: &[PathBuf],
    identity: String,
    mixed_sizes: bool,
) -> Result<ImageFolderAnalysis> {
    if files.len() < 2 {
        return Err(SplatError::InvalidVideo(
            "图片序列文件夹中至少需要 2 张 JPG、JPEG 或 PNG 图片".into(),
        ));
    }
    // Four bounded header readers; preserve natural order regardless of completion order.
    let images = std::thread::scope(|scope| -> Result<Vec<InspectedImage>> {
        let jobs = files
            .chunks(files.len().div_ceil(4))
            .map(|chunk| {
                scope.spawn(move || chunk.iter().map(|p| inspect(p)).collect::<Result<Vec<_>>>())
            })
            .collect::<Vec<_>>();
        let mut images = Vec::with_capacity(files.len());
        for job in jobs {
            images.extend(job.join().map_err(|_| invalid())??);
        }
        Ok(images)
    })?;
    let first = &images[0];
    if let Some(other) = images
        .iter()
        .find(|p| !mixed_sizes && (p.width, p.height) != (first.width, first.height))
    {
        return Err(SplatError::InvalidVideo(format!(
            "图片序列分辨率不一致：{} 为 {}×{}，预期 {}×{}",
            other.path.display(),
            other.width,
            other.height,
            first.width,
            first.height
        )));
    }
    let same_dimensions = images
        .iter()
        .all(|p| (p.width, p.height) == (first.width, first.height));
    let ratio = images
        .iter()
        .all(|p| p.width >= 64 && p.width == p.height.saturating_mul(2));
    let unsupported = images.iter().any(|p| p.unsupported);
    let confirmed =
        same_dimensions && ratio && !unsupported && images.iter().all(|p| p.complete_erp);
    let info = ImageSequenceInfo {
        image_count: images.len() as u64,
        width: images.iter().map(|p| p.width).max().unwrap_or(first.width),
        height: images
            .iter()
            .map(|p| p.height)
            .max()
            .unwrap_or(first.height),
        has_alpha: images.iter().any(|p| p.alpha),
        requires_large_sequence_confirmation: images.len() as u64 > LARGE_SEQUENCE_WARNING_COUNT,
    };
    Ok(ImageFolderAnalysis {
        info,
        detection: ProjectionDetection {
            source_identity: identity,
            unsupported,
            confirmed_erp: confirmed,
            requires_choice: ratio && !confirmed,
            supported_erp: same_dimensions && ratio && !unsupported,
            reason: String::new(),
            preview_jpeg: Vec::new(),
        },
        images,
    })
}

fn orientation(tiff: &[u8]) -> u8 {
    let le = tiff.starts_with(b"II");
    if !le && !tiff.starts_with(b"MM") {
        return 1;
    }
    let u16_at = |p: usize| -> Option<u16> {
        let a: [u8; 2] = tiff.get(p..p.checked_add(2)?)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        })
    };
    let u32_at = |p: usize| -> Option<u32> {
        let a: [u8; 4] = tiff.get(p..p.checked_add(4)?)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    };
    if u16_at(2) != Some(42) {
        return 1;
    }
    let Some(offset) = u32_at(4).map(|p| p as usize) else {
        return 1;
    };
    for i in 0..u16_at(offset).unwrap_or(0).min(1024) as usize {
        let Some(p) = offset.checked_add(2 + i * 12) else {
            return 1;
        };
        if u16_at(p) == Some(0x112) && u16_at(p + 2) == Some(3) && u32_at(p + 4) == Some(1) {
            return u16_at(p + 8).filter(|v| (1..=8).contains(v)).unwrap_or(1) as u8;
        }
    }
    1
}

fn xmp_values(xml: &[u8]) -> Option<BTreeMap<String, String>> {
    use quick_xml::{events::Event, name::ResolveResult, NsReader};
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut out = BTreeMap::new();
    let mut current = None;
    loop {
        let (ns, event) = reader.read_resolved_event().ok()?;
        let gp = matches!(ns,ResolveResult::Bound(v) if v.as_ref()==GPANO || v.as_ref()==SPHERICAL);
        let event = event.into_owned();
        match event {
            Event::Start(e) | Event::Empty(e) => {
                current = gp.then(|| String::from_utf8_lossy(e.local_name().as_ref()).into_owned());
                for attribute in e.attributes() {
                    let a = attribute.ok()?;
                    let (ns, key) = reader.resolver().resolve_attribute(a.key);
                    if matches!(ns,ResolveResult::Bound(v) if v.as_ref()==GPANO || v.as_ref()==SPHERICAL)
                    {
                        let key = String::from_utf8_lossy(key.as_ref()).into_owned();
                        let value = a
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .ok()?
                            .into_owned();
                        if out
                            .insert(key, value.clone())
                            .is_some_and(|old| old != value)
                        {
                            return Some(BTreeMap::from([(
                                "ProjectionType".into(),
                                "conflicting".into(),
                            )]));
                        }
                    }
                }
            }
            Event::Text(e) => {
                if let Some(key) = &current {
                    let value = e.decode().ok()?.into_owned();
                    if out
                        .insert(key.clone(), value.clone())
                        .is_some_and(|old| old != value)
                    {
                        return Some(BTreeMap::from([(
                            "ProjectionType".into(),
                            "conflicting".into(),
                        )]));
                    }
                }
            }
            Event::End(_) => current = None,
            Event::DocType(_) | Event::GeneralRef(_) => return None,
            Event::Eof => return Some(out),
            _ => {}
        }
    }
}

struct Metadata {
    remaining: usize,
    unknown: bool,
    orientation: u8,
    values: BTreeMap<String, String>,
    unsupported: bool,
}
impl Metadata {
    fn read(&mut self, file: &mut File, size: usize) -> Result<Option<Vec<u8>>> {
        if size > self.remaining {
            self.unknown = true;
            return Ok(None);
        }
        self.remaining -= size;
        let mut data = vec![0; size];
        file.read_exact(&mut data)?;
        Ok(Some(data))
    }
    fn exif(&mut self, data: &[u8]) {
        self.orientation = orientation(data.strip_prefix(b"Exif\0\0").unwrap_or(data));
    }
    fn xmp(&mut self, data: &[u8]) {
        if let Some(values) = xmp_values(data) {
            self.unsupported |= unsupported_metadata(&values);
            for (k, v) in values {
                if self.values.get(&k).is_some_and(|old| old != &v) {
                    self.unknown = true;
                    self.unsupported = true;
                }
                self.values.insert(k, v);
            }
        } else {
            self.unknown = true;
        }
    }
}
fn unsupported_metadata(values: &BTreeMap<String, String>) -> bool {
    let number = |key: &str| values.get(key).and_then(|v| v.parse::<i64>().ok());
    ["Stitched", "Spherical"].iter().any(|key| {
        values
            .get(*key)
            .is_some_and(|v| v.eq_ignore_ascii_case("false"))
    }) || number("CroppedAreaLeftPixels").is_some_and(|v| v != 0)
        || number("CroppedAreaTopPixels").is_some_and(|v| v != 0)
        || matches!((number("CroppedAreaImageWidthPixels"),number("FullPanoWidthPixels")),(Some(a),Some(b)) if a!=b)
        || matches!((number("CroppedAreaImageHeightPixels"),number("FullPanoHeightPixels")),(Some(a),Some(b)) if a!=b)
        || values
            .get("ProjectionType")
            .is_some_and(|v| !v.eq_ignore_ascii_case("equirectangular"))
        || values.iter().any(|(k, v)| {
            k.to_ascii_lowercase().contains("stereo")
                && !matches!(
                    v.to_ascii_lowercase().as_str(),
                    "mono" | "monoscopic" | "none" | "0"
                )
        })
}

pub fn inspect(path: &Path) -> Result<InspectedImage> {
    #[cfg(test)]
    {
        *header_reads()
            .lock()
            .unwrap()
            .entry(path.to_path_buf())
            .or_default() += 1;
    }
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    let mut width = 0;
    let mut height = 0;
    let mut alpha = false;
    let mut terminated = false;
    let mut meta = Metadata {
        remaining: METADATA_BUDGET,
        unknown: false,
        orientation: 1,
        values: BTreeMap::new(),
        unsupported: false,
    };
    if magic.starts_with(&[0xff, 0xd8]) {
        file.seek(SeekFrom::Start(2))?;
        for _ in 0..4096 {
            let mut marker = [0; 2];
            file.read_exact(&mut marker)?;
            if marker[0] != 0xff {
                return Err(invalid());
            }
            while marker[1] == 0xff {
                file.read_exact(&mut marker[1..])?;
            }
            if matches!(marker[1], 0xda | 0xd9) {
                terminated = true;
                break;
            }
            if marker[1] == 0x01 || (0xd0..=0xd7).contains(&marker[1]) {
                continue;
            }
            let mut len = [0; 2];
            file.read_exact(&mut len)?;
            let size = u16::from_be_bytes(len).checked_sub(2).ok_or_else(invalid)? as usize;
            let end = file
                .stream_position()?
                .checked_add(size as u64)
                .filter(|p| *p <= length)
                .ok_or_else(invalid)?;
            if matches!(marker[1],0xc0..=0xc3|0xc5..=0xc7|0xc9..=0xcb|0xcd..=0xcf) {
                if size < 6 {
                    return Err(invalid());
                }
                let mut b = [0; 6];
                file.read_exact(&mut b)?;
                height = u16::from_be_bytes([b[1], b[2]]) as u32;
                width = u16::from_be_bytes([b[3], b[4]]) as u32;
            } else if marker[1] == 0xe1 {
                if let Some(data) = meta.read(&mut file, size)? {
                    if data.starts_with(b"Exif\0\0") {
                        meta.exif(&data);
                    } else if let Some(xml) = data.strip_prefix(b"http://ns.adobe.com/xap/1.0/\0") {
                        meta.xmp(xml);
                    } else if data.starts_with(b"http://ns.adobe.com/xmp/extension/") {
                        meta.unknown = true;
                    }
                }
            }
            file.seek(SeekFrom::Start(end))?;
        }
    } else if magic == *b"\x89PNG\r\n\x1a\n" {
        for _ in 0..4096 {
            let mut head = [0; 8];
            file.read_exact(&mut head)?;
            let size = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
            let end = file
                .stream_position()?
                .checked_add(size + 4)
                .filter(|p| *p <= length)
                .ok_or_else(invalid)?;
            match &head[4..] {
                b"IHDR" => {
                    if size != 13 {
                        return Err(invalid());
                    }
                    let mut b = [0; 13];
                    file.read_exact(&mut b)?;
                    width = u32::from_be_bytes(b[..4].try_into().unwrap());
                    height = u32::from_be_bytes(b[4..8].try_into().unwrap());
                    alpha = matches!(b[9], 4 | 6);
                }
                b"tRNS" => alpha = true,
                b"eXIf" | b"iTXt" => {
                    if let Some(data) = meta.read(&mut file, size as usize)? {
                        if &head[4..] == b"eXIf" {
                            meta.exif(&data);
                        } else if let Some(rest) = data.strip_prefix(b"XML:com.adobe.xmp\0") {
                            if rest.len() < 2 {
                                meta.unknown = true;
                            } else {
                                let compressed = rest[0];
                                let method = rest[1];
                                let body = rest
                                    .get(2..)
                                    .and_then(|s| s.split_once_byte(0))
                                    .and_then(|(_, s)| s.split_once_byte(0))
                                    .map(|(_, s)| s);
                                if let Some(body) = body {
                                    if compressed == 0 && method == 0 {
                                        meta.xmp(body);
                                    } else if compressed == 1 && method == 0 {
                                        let mut decoded = Vec::new();
                                        if flate2::read::ZlibDecoder::new(body)
                                            .take(meta.remaining as u64 + 1)
                                            .read_to_end(&mut decoded)
                                            .is_ok()
                                            && decoded.len() <= meta.remaining
                                        {
                                            meta.remaining -= decoded.len();
                                            meta.xmp(&decoded);
                                        } else {
                                            meta.unknown = true;
                                        }
                                    } else {
                                        meta.unknown = true;
                                    }
                                } else {
                                    meta.unknown = true;
                                }
                            }
                        }
                    }
                }
                b"IEND" => {
                    terminated = true;
                    break;
                }
                _ => {}
            }
            file.seek(SeekFrom::Start(end))?;
        }
    } else {
        return Err(invalid());
    }
    if width == 0 || height == 0 {
        return Err(invalid());
    }
    // Evaluate crop metadata in the encoded image's coordinate system, before EXIF rotation.
    let number = |key: &str| meta.values.get(key).and_then(|v| v.parse::<u64>().ok());
    let projection = meta
        .values
        .get("ProjectionType")
        .map(|v| v.to_ascii_lowercase());
    let unsupported = meta.unsupported || unsupported_metadata(&meta.values);
    let complete = width == height.saturating_mul(2)
        && terminated
        && !meta.unknown
        && !unsupported
        && projection.as_deref() == Some("equirectangular")
        && number("CroppedAreaLeftPixels") == Some(0)
        && number("CroppedAreaTopPixels") == Some(0)
        && number("CroppedAreaImageWidthPixels") == Some(width as u64)
        && number("FullPanoWidthPixels") == Some(width as u64)
        && number("CroppedAreaImageHeightPixels") == Some(height as u64)
        && number("FullPanoHeightPixels") == Some(height as u64);
    if (5..=8).contains(&meta.orientation) {
        std::mem::swap(&mut width, &mut height);
    }
    Ok(InspectedImage {
        path: path.into(),
        width,
        height,
        alpha,
        orientation: meta.orientation,
        complete_erp: complete,
        unsupported,
    })
}

trait SplitByte {
    fn split_once_byte(&self, byte: u8) -> Option<(&[u8], &[u8])>;
}
impl SplitByte for [u8] {
    fn split_once_byte(&self, byte: u8) -> Option<(&[u8], &[u8])> {
        let i = self.iter().position(|v| *v == byte)?;
        Some((&self[..i], &self[i + 1..]))
    }
}

pub fn checkpoint(
    source: &Path,
    quality: crate::presets::Quality,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<super::panorama::PanoramaCheckpoint> {
    checkpoint_for_policy(source, quality, cancel, false)
}

pub fn checkpoint_for_policy(
    source: &Path,
    quality: crate::presets::Quality,
    cancel: &tokio_util::sync::CancellationToken,
    native: bool,
) -> Result<super::panorama::PanoramaCheckpoint> {
    let scan = analyze(source)?;
    if !scan.detection.supported_erp {
        return Err(invalid());
    }
    let (cap, face) = match quality {
        crate::presets::Quality::Fast => (2048, 512),
        crate::presets::Quality::Balanced => (3072, 768),
        crate::presets::Quality::High if native => (scan.info.width, scan.info.width / 4),
        crate::presets::Quality::High => (4096, 1024),
    };
    let width = if native && quality == crate::presets::Quality::High {
        scan.info.width
    } else {
        scan.info.width.min(cap) / 4 * 4
    };
    validate_cache_size(scan.info.image_count, width, width / 2, face.min(width / 4))?;
    if (scan.info.width as u64)
        .checked_mul(scan.info.height as u64)
        .and_then(|n| n.checked_mul(4))
        .is_none_or(|n| n > super::panorama::MAX_DECODE_BYTES)
    {
        return Err(SplatError::Process(OVER_BUDGET.into()));
    }
    let origins_path = source.parent().unwrap_or(source).join("image-origins.json");
    let origins: BTreeMap<String, OriginalImageIdentity> = if origins_path.is_file() {
        let file = File::open(&origins_path)?;
        if file.metadata()?.len() > 16 * 1024 * 1024 {
            return Err(invalid());
        }
        serde_json::from_reader(file)?
    } else {
        BTreeMap::new()
    };
    let mut hash = Sha256::new();
    let mut images = Vec::new();
    for (index, image) in scan.images.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let mut decoder = generation_decoder(&image.path)?;
        let actual_orientation = generation_orientation(&mut decoder, image)?;
        let mut dimensions = decoder.dimensions();
        if (5..=8).contains(&actual_orientation.to_exif()) {
            dimensions = (dimensions.1, dimensions.0);
        }
        if dimensions != (image.width, image.height) {
            return Err(invalid());
        }
        drop(decoder);
        let fingerprint = super::panorama::fingerprint_cancellable(&image.path, cancel)?;
        let name = image
            .path
            .file_name()
            .ok_or_else(invalid)?
            .to_str()
            .ok_or_else(invalid)?
            .to_owned();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update(fingerprint.as_bytes());
        hash.update([actual_orientation.to_exif()]);
        images.push(ImageSourceMapping {
            source_image_index: index as u64,
            original: origins.get(&name).cloned(),
            source_name: name,
            source_sha256: fingerprint,
            orientation: actual_orientation.to_exif(),
            prepared_name: format!("frame_{index:010}.png"),
        });
    }
    if source_identity(source)? != scan.detection.source_identity {
        return Err(SplatError::Process(
            "素材文件已发生变化，请重新分析后再生成".into(),
        ));
    }
    Ok(super::panorama::PanoramaCheckpoint {
        version: super::panorama::CONVERSION_VERSION,
        source_sha256: format!("{:x}", hash.finalize()),
        sfm_width: width,
        sfm_height: width / 2,
        face_size: face.min(width / 4),
        field_of_view_degrees: super::panorama::FOV_DEGREES,
        image_sources: Some(images),
    })
}

fn validate_cache_size(count: u64, width: u32, height: u32, face: u32) -> Result<()> {
    // Guard arithmetic and COLMAP face IDs only. Disk usage has no fixed cap.
    count
        .checked_mul(6)
        .filter(|n| *n <= u32::MAX as u64)
        .ok_or_else(invalid)?;
    let bytes = (width as u64)
        .checked_mul(height as u64)
        .and_then(|n| n.checked_mul(9))
        .and_then(|n| {
            (face as u64)
                .checked_mul(face as u64)
                .and_then(|f| f.checked_mul(36))
                .and_then(|f| n.checked_add(f))
        })
        .and_then(|n| n.checked_mul(count));
    bytes.ok_or_else(invalid)?;
    Ok(())
}

pub fn prepare(
    source: &Path,
    config: &super::panorama::PanoramaCheckpoint,
    frames: &Path,
    colmap: &Path,
    masks: &Path,
    cancel: &tokio_util::sync::CancellationToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<super::image_sequence::PreparedImageSequence> {
    let records = config.image_sources.as_ref().ok_or_else(invalid)?;
    if config.sfm_width == 0
        || config.sfm_height != config.sfm_width / 2
        || config.face_size == 0
        || config.face_size > config.sfm_width / 4
    {
        return Err(invalid());
    }
    let scan = analyze(source)?;
    if scan.images.len() != records.len() || !scan.detection.supported_erp {
        return Err(invalid());
    }
    validate_cache_size(
        records.len() as u64,
        config.sfm_width,
        config.sfm_height,
        config.face_size,
    )?;
    for (index, record) in records.iter().enumerate() {
        if record.prepared_name != format!("frame_{index:010}.png")
            || record.source_image_index != index as u64
        {
            return Err(invalid());
        }
    }
    let stage = frames
        .parent()
        .ok_or_else(invalid)?
        .join(format!(".panorama-images-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        for dir in ["frames", "colmap", "masks"] {
            std::fs::create_dir_all(stage.join(dir))?;
        }
        let mut transparent = false;
        for (image, record) in scan.images.iter().zip(records) {
            if cancel.is_cancelled() {
                return Err(SplatError::Cancelled);
            }
            if image.width as u64 * image.height as u64 * 4 > super::panorama::MAX_DECODE_BYTES {
                return Err(SplatError::Process(OVER_BUDGET.into()));
            }
            if image.path.file_name().and_then(|v| v.to_str()) != Some(&record.source_name) {
                return Err(SplatError::Process(
                    "素材文件已发生变化，请重新分析后再生成".into(),
                ));
            }
            let mut decoder = generation_decoder(&image.path)?;
            let actual_orientation = generation_orientation(&mut decoder, image)?;
            if actual_orientation.to_exif() != record.orientation {
                return Err(invalid());
            }
            let mut decoded = image::DynamicImage::from_decoder(decoder).map_err(|_| invalid())?;
            decoded.apply_orientation(actual_orientation);
            if (decoded.width(), decoded.height()) != (image.width, image.height) {
                return Err(invalid());
            }
            if super::panorama::fingerprint_cancellable(&image.path, cancel)?
                != record.source_sha256
            {
                return Err(SplatError::Process(
                    "素材文件已发生变化，请重新分析后再生成".into(),
                ));
            }
            let rgba = decoded
                .resize_exact(
                    config.sfm_width,
                    config.sfm_height,
                    image::imageops::FilterType::Lanczos3,
                )
                .into_rgba8();
            drop(decoded);
            transparent |= rgba.pixels().any(|p| p[3] < 255);
            let mask = image::GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
                image::Luma([if rgba.get_pixel(x, y)[3] >= 128 {
                    255
                } else {
                    0
                }])
            });
            mask.save(
                stage
                    .join("masks")
                    .join(format!("{}.png", record.prepared_name)),
            )
            .map_err(|_| invalid())?;
            let frame = stage.join("frames").join(&record.prepared_name);
            // A derived cache, not an archival image: avoid expensive PNG recompression.
            use image::ImageEncoder;
            image::codecs::png::PngEncoder::new_with_quality(
                File::create(&frame)?,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::NoFilter,
            )
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|_| invalid())?;
            let colmap_frame = stage.join("colmap").join(&record.prepared_name);
            if std::fs::hard_link(&frame, &colmap_frame).is_err() {
                std::fs::copy(&frame, &colmap_frame)?;
            }
            progress(record.source_image_index + 1, records.len() as u64);
        }
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        if source_identity(source)? != scan.detection.source_identity {
            return Err(SplatError::Process(
                "素材文件已发生变化，请重新分析后再生成".into(),
            ));
        }
        super::image_sequence::validate_prepared_image_sequence(
            &stage.join("frames"),
            &stage.join("colmap"),
            &stage.join("masks"),
            records.len() as u64,
            transparent,
        )?;
        for (directory, target) in [("frames", frames), ("colmap", colmap), ("masks", masks)] {
            if target.exists() {
                std::fs::remove_dir(target)?;
            } // only the empty directories owned by this preparation
            std::fs::rename(stage.join(directory), target)?;
        }
        Ok(super::image_sequence::PreparedImageSequence {
            image_count: records.len() as u64,
            mask_count: if transparent { records.len() as u64 } else { 0 },
            has_alpha: transparent,
        })
    })();
    let _ = std::fs::remove_dir_all(&stage);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tokio_util::sync::CancellationToken;

    fn xml(width: u32, height: u32) -> String {
        format!(
            r#"<rdf:Description xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:p="http://ns.google.com/photos/1.0/panorama/" p:ProjectionType="equirectangular" p:CroppedAreaLeftPixels="0" p:CroppedAreaTopPixels="0" p:CroppedAreaImageWidthPixels="{width}" p:CroppedAreaImageHeightPixels="{height}" p:FullPanoWidthPixels="{width}" p:FullPanoHeightPixels="{height}"/>"#
        )
    }
    // Intentionally no pixel stream: proves preflight does not invoke a pixel decoder.
    fn jpeg_header(path: &Path, width: u16, height: u16, packets: &[Vec<u8>]) {
        let mut f = File::create(path).unwrap();
        f.write_all(&[0xff, 0xd8]).unwrap();
        for packet in packets {
            f.write_all(&[0xff, 0xe1]).unwrap();
            f.write_all(&((packet.len() + 2) as u16).to_be_bytes())
                .unwrap();
            f.write_all(packet).unwrap();
        }
        f.write_all(&[0xff, 0xc0, 0, 8, 8]).unwrap();
        f.write_all(&height.to_be_bytes()).unwrap();
        f.write_all(&width.to_be_bytes()).unwrap();
        f.write_all(&[3, 0xff, 0xda]).unwrap();
    }
    fn packet(xml: &str) -> Vec<u8> {
        [b"http://ns.adobe.com/xap/1.0/\0".as_slice(), xml.as_bytes()].concat()
    }
    fn exif(rotation: u8) -> Vec<u8> {
        let mut v = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        v.extend([rotation, 0, 0, 0, 0, 0, 0, 0]);
        v
    }

    #[test]
    fn tiff_orientation_reads_fixed_entries_in_both_byte_orders_and_rejects_truncation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("metadata.tiff");
        for little_endian in [true, false] {
            let word = |value: u16| {
                if little_endian {
                    value.to_le_bytes()
                } else {
                    value.to_be_bytes()
                }
            };
            let long = |value: u32| {
                if little_endian {
                    value.to_le_bytes()
                } else {
                    value.to_be_bytes()
                }
            };
            let mut tiff = if little_endian { b"II" } else { b"MM" }.to_vec();
            tiff.extend(word(42));
            tiff.extend(long(8));
            tiff.extend(word(3));
            // The orientation is not the first entry: preserve full 12-byte strides.
            for (tag, value) in [(0x100, 64), (0x101, 32), (0x112, 6)] {
                tiff.extend(word(tag));
                tiff.extend(word(3));
                tiff.extend(long(1));
                tiff.extend(word(value));
                tiff.extend([0; 2]);
            }
            tiff.extend(long(0));
            std::fs::write(&path, &tiff).unwrap();
            assert_eq!(
                tiff_orientation_from_file(&mut File::open(&path).unwrap(), 0, tiff.len() as u64)
                    .unwrap(),
                6
            );

            let truncated = &tiff[..tiff.len() - 5];
            std::fs::write(&path, truncated).unwrap();
            assert!(tiff_orientation_from_file(
                &mut File::open(&path).unwrap(),
                0,
                truncated.len() as u64
            )
            .is_err());
        }
    }
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for b in bytes {
            crc ^= *b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb88320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn png_chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut c = (body.len() as u32).to_be_bytes().to_vec();
        c.extend(kind);
        c.extend(body);
        c.extend(crc32(&c[4..]).to_be_bytes());
        c
    }
    fn add_png_metadata(path: &Path, kind: &[u8; 4], data: &[u8]) {
        let mut bytes = std::fs::read(path).unwrap();
        let index = bytes.len() - 12;
        bytes.splice(index..index, png_chunk(kind, data));
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn bounded_headers_support_gpano_namespaces_and_never_decode_pixels() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("图片2.jpg");
        let b = d.path().join("图片10.JPEG");
        jpeg_header(&a, 64, 32, &[packet(&xml(64, 32))]);
        jpeg_header(&b, 64, 32, &[packet(&xml(64, 32))]);
        let scan = analyze(d.path()).unwrap();
        assert!(scan.detection.confirmed_erp);
        assert_eq!(scan.images[0].path, a);
        assert!(image::open(&a).is_err());
        assert_eq!(scan.info.image_count, 2);
        let element = r#"<x xmlns:p="http://ns.google.com/photos/1.0/panorama/"><p:ProjectionType>equirectangular</p:ProjectionType></x>"#;
        assert_eq!(
            xmp_values(element.as_bytes()).unwrap()["ProjectionType"],
            "equirectangular"
        );
    }
    #[test]
    fn missing_partial_conflicting_and_oversize_metadata_never_auto_confirm() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("1.jpg");
        let b = d.path().join("2.jpg");
        jpeg_header(&b, 64, 32, &[packet(&xml(64, 32))]);
        for packets in [
            vec![],
            vec![packet("<x/>")],
            vec![packet("<broken")],
            vec![packet(&xml(64, 32)), packet(&xml(128, 64))],
            vec![vec![0; 60000], packet(&xml(64, 32)), vec![0; 60000]],
        ] {
            jpeg_header(&a, 64, 32, &packets);
            let scan = analyze(d.path()).unwrap();
            assert!(!scan.detection.confirmed_erp);
            assert!(scan.detection.requires_choice);
        }
    }
    #[test]
    fn explicit_crop_stereo_non_erp_cannot_be_overridden() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("a.jpg");
        for xml in [
            xml(64, 32).replace("LeftPixels=\"0\"", "LeftPixels=\"-1\""),
            xml(64, 32).replace("equirectangular", "fisheye"),
            xml(64, 32).replace("/>", " p:StereoMode=\"top-bottom\"/>"),
        ] {
            jpeg_header(&path, 64, 32, &[packet(&xml)]);
            assert!(inspect(&path).unwrap().unsupported);
        }
    }
    #[test]
    fn exif_orientation_is_header_only_and_dimensions_are_normalized() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("1.jpg");
        let b = d.path().join("2.jpg");
        jpeg_header(&a, 32, 64, &[exif(6)]);
        jpeg_header(&b, 64, 32, &[]);
        let scan = analyze(d.path()).unwrap();
        assert_eq!((scan.info.width, scan.info.height), (64, 32));
        assert_eq!(scan.images[0].orientation, 6);
        assert!(scan.detection.requires_choice);
        jpeg_header(&b, 80, 32, &[]);
        assert!(analyze(d.path()).is_err());
    }
    #[test]
    fn png_alpha_compressed_xmp_and_decompression_budget() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("a.png");
        image::RgbaImage::new(64, 32).save(&path).unwrap();
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(xml(64, 32).as_bytes()).unwrap();
        let mut body = b"XML:com.adobe.xmp\0\x01\0\0\0".to_vec();
        body.extend(encoder.finish().unwrap());
        add_png_metadata(&path, b"iTXt", &body);
        let info = inspect(&path).unwrap();
        assert!(info.alpha && info.complete_erp);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&vec![b' '; METADATA_BUDGET + 1]).unwrap();
        let mut body = b"XML:com.adobe.xmp\0\x01\0\0\0".to_vec();
        body.extend(encoder.finish().unwrap());
        add_png_metadata(&path, b"iTXt", &body);
        assert!(!inspect(&path).unwrap().complete_erp);
    }
    #[test]
    fn folder_identity_covers_members_not_only_directory_mtime() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("1.jpg");
        let b = d.path().join("2.jpg");
        jpeg_header(&a, 64, 32, &[]);
        jpeg_header(&b, 64, 32, &[]);
        let id = source_identity(d.path()).unwrap();
        jpeg_header(&a, 64, 32, &[packet("<x/>")]);
        assert_ne!(source_identity(d.path()).unwrap(), id);
        let id = source_identity(d.path()).unwrap();
        std::fs::rename(&b, d.path().join("3.jpg")).unwrap();
        assert_ne!(source_identity(d.path()).unwrap(), id);
    }
    #[test]
    fn preparation_preserves_sources_rgba_masks_and_resumable_checkpoints() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("source");
        std::fs::create_dir(&source).unwrap();
        for name in ["2.png", "10.png"] {
            image::RgbaImage::from_fn(64, 32, |x, _| {
                image::Rgba([x as u8, 20, 30, if x < 32 { 0 } else { 200 }])
            })
            .save(source.join(name))
            .unwrap();
        }
        let id = source_identity(&source).unwrap();
        let cancel = CancellationToken::new();
        let config = checkpoint(&source, crate::presets::Quality::High, &cancel).unwrap();
        assert_eq!(config.sfm_width, 64);
        assert_eq!(config.face_size, 16);
        let paths = [
            d.path().join("frames"),
            d.path().join("colmap"),
            d.path().join("masks"),
        ];
        for p in &paths {
            std::fs::create_dir(p).unwrap();
        }
        let output = prepare(
            &source,
            &config,
            &paths[0],
            &paths[1],
            &paths[2],
            &cancel,
            |_, _| {},
        )
        .unwrap();
        assert!(output.has_alpha);
        assert_eq!(output.mask_count, 2);
        let name = &config.image_sources.as_ref().unwrap()[0].prepared_name;
        let frame = image::open(paths[0].join(name)).unwrap().into_rgba8();
        let mask = image::open(paths[2].join(format!("{name}.png")))
            .unwrap()
            .into_luma8();
        assert_eq!(frame.get_pixel(40, 10)[3], 200);
        assert_eq!(mask.get_pixel(40, 10)[0], 255);
        assert_eq!(mask.get_pixel(10, 10)[0], 0);
        super::super::image_sequence::validate_prepared_image_sequence(
            &paths[0], &paths[1], &paths[2], 2, true,
        )
        .unwrap();
        assert_eq!(source_identity(&source).unwrap(), id);
    }
    #[test]
    fn cancellation_and_changed_sources_publish_nothing() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("source");
        std::fs::create_dir(&source).unwrap();
        for name in ["1.png", "2.png"] {
            image::RgbImage::new(64, 32)
                .save(source.join(name))
                .unwrap();
        }
        let cancel = CancellationToken::new();
        let config = checkpoint(&source, crate::presets::Quality::Fast, &cancel).unwrap();
        cancel.cancel();
        let paths = [
            d.path().join("frames"),
            d.path().join("colmap"),
            d.path().join("masks"),
        ];
        for p in &paths {
            std::fs::create_dir(p).unwrap();
        }
        assert!(matches!(
            prepare(
                &source,
                &config,
                &paths[0],
                &paths[1],
                &paths[2],
                &cancel,
                |_, _| {}
            ),
            Err(SplatError::Cancelled)
        ));
        let mid = CancellationToken::new();
        assert!(matches!(
            prepare(
                &source,
                &config,
                &paths[0],
                &paths[1],
                &paths[2],
                &mid,
                |current, _| if current == 1 {
                    mid.cancel();
                }
            ),
            Err(SplatError::Cancelled)
        ));
        image::RgbImage::from_pixel(64, 32, image::Rgb([9, 9, 9]))
            .save(source.join("1.png"))
            .unwrap();
        assert!(prepare(
            &source,
            &config,
            &paths[0],
            &paths[1],
            &paths[2],
            &CancellationToken::new(),
            |_, _| {}
        )
        .is_err());
        assert!(std::fs::read_dir(&paths[0]).unwrap().next().is_none());
        assert!(!std::fs::read_dir(d.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".panorama-images-")));
    }
    #[test]
    fn preparation_normalizes_exif_rgb_and_alpha_with_the_same_mapping() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let first = source.join("1.png");
        image::RgbaImage::from_fn(32, 64, |x, y| {
            image::Rgba([x as u8, y as u8, 30, if x < 16 { 0 } else { 200 }])
        })
        .save(&first)
        .unwrap();
        add_png_metadata(&first, b"eXIf", &exif(6)[6..]);
        image::RgbImage::new(64, 32)
            .save(source.join("2.png"))
            .unwrap();
        let cancel = CancellationToken::new();
        let config = checkpoint(&source, crate::presets::Quality::Fast, &cancel).unwrap();
        let frames = d.path().join("frames");
        let colmap = d.path().join("colmap");
        let masks = d.path().join("masks");
        prepare(
            &source,
            &config,
            &frames,
            &colmap,
            &masks,
            &cancel,
            |_, _| {},
        )
        .unwrap();
        let name = &config.image_sources.as_ref().unwrap()[0].prepared_name;
        let rgba = image::open(frames.join(name)).unwrap().into_rgba8();
        let mask = image::open(masks.join(format!("{name}.png")))
            .unwrap()
            .into_luma8();
        assert_eq!(rgba.dimensions(), (64, 32));
        assert_eq!(rgba.get_pixel(10, 20).0, [20, 53, 30, 200]);
        assert_eq!(mask.get_pixel(10, 20)[0], 255);
        assert_eq!(mask.get_pixel(10, 5)[0], 0);
        // A late PNG eXIf chunk beyond the preflight budget stays unknown, but
        // generation seeks to its real TIFF entry instead of silently ignoring it.
        let oversized = d.path().join("oversized");
        std::fs::create_dir(&oversized).unwrap();
        let first = oversized.join("1.png");
        image::RgbaImage::from_fn(64, 32, |x, y| image::Rgba([x as u8, y as u8, 30, 200]))
            .save(&first)
            .unwrap();
        image::RgbImage::new(64, 32)
            .save(oversized.join("2.png"))
            .unwrap();
        let raw = exif(3);
        let raw = &raw[6..];
        let mut tiff = vec![0; 70000 + raw.len() - 8];
        tiff[..8].copy_from_slice(&raw[..8]);
        tiff[4..8].copy_from_slice(&70000u32.to_le_bytes());
        tiff[70000..].copy_from_slice(&raw[8..]);
        add_png_metadata(&first, b"eXIf", &tiff);
        let scan = analyze(&oversized).unwrap();
        assert!(scan.detection.requires_choice && !scan.detection.confirmed_erp);
        let config = checkpoint(&oversized, crate::presets::Quality::Fast, &cancel).unwrap();
        assert_eq!(config.image_sources.as_ref().unwrap()[0].orientation, 3);
        let frames = d.path().join("frames2");
        prepare(
            &oversized,
            &config,
            &frames,
            &d.path().join("colmap2"),
            &d.path().join("masks2"),
            &cancel,
            |_, _| {},
        )
        .unwrap();
        let rgba =
            image::open(frames.join(&config.image_sources.as_ref().unwrap()[0].prepared_name))
                .unwrap()
                .into_rgba8();
        assert_eq!(rgba.get_pixel(10, 20).0, [53, 11, 30, 200]);
    }
    #[test]
    fn source_decode_limit_is_retained_without_a_total_cache_cap() {
        let d = tempfile::tempdir().unwrap();
        for name in ["1.jpg", "2.jpg"] {
            jpeg_header(
                &d.path().join(name),
                32_768,
                16_384,
                &[packet(&xml(32_768, 16_384))],
            );
        }
        // Header-only inputs exceed 512 MiB RGBA at native resolution.
        // Fail before attempting pixel decoding or publishing any cache.
        let error = checkpoint(
            d.path(),
            crate::presets::Quality::High,
            &CancellationToken::new(),
        )
        .unwrap_err();
        assert!(error.to_string().contains(OVER_BUDGET));
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 2);
    }

    #[test]
    fn cache_sizes_above_twenty_gib_are_allowed_but_overflow_is_rejected() {
        assert!(validate_cache_size(94, 2048, 1024, 512).is_ok());
        assert!(validate_cache_size(1000, 4096, 2048, 1024).is_ok());
        assert!(validate_cache_size(u64::MAX, 4096, 2048, 1024).is_err());
        assert!(validate_cache_size(u32::MAX as u64 / 6 + 1, 64, 32, 16).is_err());
        assert!(validate_cache_size(1, u32::MAX, u32::MAX, 1024).is_err());
    }
    #[test]
    #[ignore = "Timing evidence with local real source; creates only isolated test header fixtures"]
    fn header_detection_benchmark() {
        for count in [100, 1000] {
            let d = tempfile::tempdir().unwrap();
            for i in 0..count {
                jpeg_header(
                    &d.path().join(format!("{i}.jpg")),
                    2048,
                    1024,
                    &[packet(&xml(2048, 1024))],
                );
            }
            for label in ["cold", "hot"] {
                let start = std::time::Instant::now();
                let scan = analyze(d.path()).unwrap();
                assert!(scan.detection.confirmed_erp);
                eprintln!("{count} files {label}: {} us", start.elapsed().as_micros());
            }
        }
        if let Some(source) = std::env::var_os("OOOSPLAT_PANORAMA_IMAGES") {
            for label in ["cold", "hot"] {
                let start = std::time::Instant::now();
                let scan = analyze(Path::new(&source)).unwrap();
                eprintln!(
                    "Ballintoy {} files {label}: {} us, confirmed={}, choice={}",
                    scan.info.image_count,
                    start.elapsed().as_micros(),
                    scan.detection.confirmed_erp,
                    scan.detection.requires_choice
                );
            }
        }
    }
}
