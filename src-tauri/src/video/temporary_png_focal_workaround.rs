//! TEMPORARY compatibility workaround for OpenImageIO's PNG/XMP rational parsing.
//! OIIO 3.1.14 reads e.g. "8192000/1419" as 8192000, corrupting COLMAP's
//! focal-plane initial focal length. https://github.com/ooolabdev/ooosplat/issues/73
//! After upgrading the runtime AND passing the original PNG regression, remove
//! this module, its Runner/checkpoint hooks and optional checkpoint field.
//! Disabling ENABLED bypasses detection without adding a user setting.
use crate::process::{ProcessManager, ProcessSpec};
use quick_xml::{events::Event, name::ResolveResult, NsReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

const ENABLED: bool = true;
const POLICY_VERSION: u32 = 1;
// Deliberately independent of the application's moving runtime lock.
const AFFECTED_COLMAP: &str = "4.2.1";
const AFFECTED_COMMIT: &str = "bd1fcf654d2dd8fefa1466999c190a246f83f4b9";
const AFFECTED_OIIO: &str = "3.1.14.0";
const METADATA_BUDGET: usize = 64 * 1024;
const EXIF: &[u8] = b"http://ns.adobe.com/exif/1.0/";
const TIFF: &[u8] = b"http://ns.adobe.com/tiff/1.0/";
const RDF: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Checkpoint {
    pub version: u32,
    pub image_name: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub camera_params: [f64; 4],
}

#[derive(Debug, Clone)]
pub(crate) struct FocalOverride {
    pub checkpoint: Checkpoint,
    pub raw_focal_length: String,
    pub raw_x_resolution: String,
    pub unit: u32,
}

impl FocalOverride {
    pub fn camera_params(&self) -> String {
        self.checkpoint
            .camera_params
            .map(|v| v.to_string())
            .join(",")
    }
}

pub(crate) struct Request<'a> {
    pub images: &'a Path,
    pub image_list: Option<&'a Path>,
    pub ordinary_images: bool,
    pub camera_model: &'a str,
    pub single_camera: bool,
    pub explicit_params: Option<&'a str>,
    pub existing_camera_id: Option<u32>,
    pub calibrated: bool,
}

impl Request<'_> {
    fn eligible(&self) -> bool {
        self.ordinary_images
            && self.camera_model == "SIMPLE_RADIAL"
            && self.single_camera
            && self.explicit_params.is_none()
            && self.existing_camera_id.is_none()
            && !self.calibrated
    }
}

/// None means unchanged behavior, including malformed or uncertain metadata.
pub(crate) async fn detect(
    executable: &Path,
    manager: &ProcessManager,
    request: Request<'_>,
) -> Option<FocalOverride> {
    detect_selected(executable, manager, request, None).await
}

/// Inspect the same in-memory image group that will become COLMAP's
/// `image_list`. This keeps recovery preflight read-only even when the list
/// file has not been emitted yet. It never searches unrelated directory files.
pub(crate) async fn detect_image_names(
    executable: &Path,
    manager: &ProcessManager,
    request: Request<'_>,
    image_names: &[String],
) -> Option<FocalOverride> {
    detect_selected(executable, manager, request, Some(image_names.to_vec())).await
}

async fn detect_selected(
    executable: &Path,
    manager: &ProcessManager,
    request: Request<'_>,
    image_names: Option<Vec<String>>,
) -> Option<FocalOverride> {
    if !ENABLED || !request.eligible() {
        return None;
    }
    let images = request.images.to_path_buf();
    let list = request.image_list.map(Path::to_path_buf);
    let binary = executable.to_path_buf();
    let (name, path) = tokio::task::spawn_blocking(move || {
        let (name, path) = match image_names {
            Some(names) => first_named_image(&images, &names),
            None => first_image(&images, list.as_deref()),
        }?;
        if !path.extension()?.eq_ignore_ascii_case("png") {
            return None;
        }
        runtime_metadata_matches(&binary).then_some((name, path))
    })
    .await
    .ok()??;
    let help = manager
        .run(ProcessSpec {
            executable: executable.to_path_buf(),
            args: vec!["feature_extractor".into(), "-h".into()],
            working_directory: executable.parent().map(Path::to_path_buf),
            log_path: None,
            observer: None,
        })
        .await
        .ok()?;
    if !help.success || !affected_banner(&format!("{}\n{}", help.stdout, help.stderr)) {
        return None;
    }
    // Hash only after both runtime identity and the full PNG/XMP path match.
    tokio::task::spawn_blocking(move || read_candidate(&name, &path))
        .await
        .ok()?
}

fn affected_banner(help: &str) -> bool {
    [AFFECTED_COMMIT, &AFFECTED_COMMIT[..7]]
        .into_iter()
        .any(|commit| {
            let marker = format!("COLMAP {AFFECTED_COLMAP} (Commit {commit}");
            help.match_indices(&marker).any(|(i, _)| {
                help[i + marker.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_whitespace() || c == ')')
            })
        })
}

fn bounded_json(path: &Path) -> Option<serde_json::Value> {
    let mut data = Vec::new();
    File::open(path)
        .ok()?
        .take(METADATA_BUDGET as u64 + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() <= METADATA_BUDGET).then_some(())?;
    serde_json::from_slice(&data).ok()
}

fn runtime_metadata_matches(executable: &Path) -> bool {
    let check = || -> Option<bool> {
        let root = executable.parent()?.parent()?;
        let info = bounded_json(&root.join("BUILD-INFO.json"))?;
        let components = bounded_json(&root.join("BUNDLED-COMPONENTS.json"))?;
        let contains = |values: &serde_json::Value, name: &str, version: &str| {
            values.as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item["name"] == name && item["version"] == version)
            })
        };
        Some(
            info["sourceCommit"] == AFFECTED_COMMIT
                && info["colmapVersion"] == AFFECTED_COLMAP
                && info["runtimeRevision"] == 1
                && contains(&info["dependencyVersions"], "openimageio", AFFECTED_OIIO)
                && contains(&components["components"], "COLMAP", AFFECTED_COLMAP)
                && contains(&components["components"], "openimageio", AFFECTED_OIIO)
                && components["components"]
                    .as_array()?
                    .iter()
                    .any(|v| v["name"] == "COLMAP" && v["source"] == AFFECTED_COMMIT),
        )
    };
    check() == Some(true)
}

/// COLMAP sorts image_list entries too; source-directory natural order is NOT used.
fn first_image(images: &Path, image_list: Option<&Path>) -> Option<(String, PathBuf)> {
    let mut names = Vec::new();
    if let Some(list) = image_list {
        let data = std::fs::read_to_string(list).ok()?;
        for line in data.lines().filter(|line| !line.trim().is_empty()) {
            if line.trim() != line {
                return None;
            }
            let path = Path::new(line);
            if path.is_absolute()
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
            {
                return None;
            }
            names.push(line.replace('\\', "/"));
        }
    } else {
        fn walk(root: &Path, dir: &Path, names: &mut Vec<String>) -> Option<()> {
            for entry in std::fs::read_dir(dir).ok()? {
                let entry = entry.ok()?;
                let kind = entry.file_type().ok()?;
                if kind.is_symlink() {
                    return None;
                }
                if kind.is_dir() {
                    walk(root, &entry.path(), names)?;
                } else if kind.is_file() {
                    names.push(
                        entry
                            .path()
                            .strip_prefix(root)
                            .ok()?
                            .to_str()?
                            .replace('\\', "/"),
                    );
                }
            }
            Some(())
        }
        walk(images, images, &mut names)?;
    }
    first_named_image(images, &names)
}

fn first_named_image(images: &Path, names: &[String]) -> Option<(String, PathBuf)> {
    let mut names = names.to_vec();
    for name in &mut names {
        if name.is_empty()
            || name.trim() != name.as_str()
            || name.contains(['\n', '\r', '\0', ':'])
            || Path::new(name.as_str()).is_absolute()
            || Path::new(name.as_str())
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return None;
        }
        *name = name.replace('\\', "/");
    }
    names.sort();
    let name = names.into_iter().next()?;
    let path = images.join(&name);
    let root = std::fs::canonicalize(images).ok()?;
    let canonical = std::fs::canonicalize(&path).ok()?;
    (canonical.starts_with(root) && canonical.is_file()).then_some((name, path))
}

fn field(namespace: ResolveResult<'_>, local: &[u8]) -> Option<&'static str> {
    let ResolveResult::Bound(ns) = namespace else {
        return None;
    };
    match (ns.as_ref(), local) {
        (EXIF, b"FocalLength") => Some("focal"),
        (EXIF, b"FocalPlaneXResolution") => Some("xres"),
        (EXIF, b"FocalPlaneResolutionUnit") => Some("unit"),
        (EXIF, b"FocalLengthIn35mmFilm") => Some("equivalent"),
        (EXIF, b"PixelXDimension") => Some("pixel_width"),
        (EXIF, b"PixelYDimension") => Some("pixel_height"),
        (TIFF, b"ImageWidth") => Some("width"),
        (TIFF, b"ImageLength") => Some("height"),
        (TIFF, b"Orientation") => Some("orientation"),
        _ => None,
    }
}

fn xmp_fields(xml: &[u8]) -> Option<BTreeMap<&'static str, String>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut values = BTreeMap::new();
    let mut depth = 0_usize;
    let mut roots = 0;
    let mut description = None;
    let mut descriptions = 0;
    let mut rdf = false;
    let mut pending: Option<(&'static str, usize, String)> = None;
    let insert = |values: &mut BTreeMap<_, _>, key, value: String| -> Option<()> {
        if value.contains('&') {
            return None;
        }
        if values
            .insert(key, value.clone())
            .is_some_and(|old| old != value)
        {
            return None;
        }
        Some(())
    };
    loop {
        let (ns, event) = reader.read_resolved_event().ok()?;
        let key = match &event {
            Event::Start(e) | Event::Empty(e) => field(ns.clone(), e.local_name().as_ref()),
            _ => None,
        };
        let is_rdf = matches!(&ns, ResolveResult::Bound(n) if n.as_ref() == RDF);
        let event = event.into_owned();
        match event {
            Event::Start(e) => {
                if pending.is_some() || depth >= 64 {
                    return None;
                }
                if depth == 0 {
                    roots += 1;
                    if roots != 1 {
                        return None;
                    }
                }
                let is_description = is_rdf && e.local_name().as_ref() == b"Description";
                rdf |= is_rdf && e.local_name().as_ref() == b"RDF";
                if is_description {
                    descriptions += 1;
                    if !rdf || descriptions != 1 {
                        return None;
                    }
                    description = Some(depth + 1);
                }
                for attr in e.attributes() {
                    let attr = attr.ok()?;
                    if attr.value.contains(&b'&') {
                        return None;
                    }
                    let (ns, local) = reader.resolver().resolve_attribute(attr.key);
                    if let Some(key) = field(ns, local.as_ref()) {
                        if !is_description {
                            return None;
                        }
                        let value = attr
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .ok()?;
                        insert(&mut values, key, value.trim().to_owned())?;
                    }
                }
                if let Some(key) = key {
                    if description != Some(depth) {
                        return None;
                    }
                    pending = Some((key, depth + 1, String::new()));
                }
                depth += 1;
            }
            Event::End(_) => {
                if let Some((key, at, _)) = &pending {
                    if *at == depth {
                        let key = *key;
                        let (_, _, value) = pending.take()?;
                        insert(&mut values, key, value.trim().to_owned())?;
                    }
                }
                if description == Some(depth) {
                    description = None;
                }
                depth = depth.checked_sub(1)?;
            }
            Event::Text(e) => {
                if depth == 0 && !e.decode().ok()?.trim().is_empty() {
                    return None;
                }
                if let Some((_, _, value)) = &mut pending {
                    value.push_str(&e.decode().ok()?);
                }
            }
            Event::CData(e) => {
                if depth == 0 {
                    return None;
                }
                if let Some((_, _, value)) = &mut pending {
                    value.push_str(&e.decode().ok()?);
                }
            }
            Event::DocType(_) | Event::GeneralRef(_) => return None,
            Event::Eof => {
                return (roots == 1 && rdf && descriptions == 1 && depth == 0 && pending.is_none())
                    .then_some(values)
            }
            _ => {}
        }
    }
}

fn positive_number(value: &str) -> Option<(f64, Option<u64>)> {
    let integer = |s: &str| {
        let s = s.trim();
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then_some(())?;
        s.parse::<u64>().ok()
    };
    let value = value.trim();
    let (number, denominator) = if let Some((n, d)) = value.split_once('/') {
        let (n, d) = (integer(n)?, integer(d)?);
        if d == 0 {
            return None;
        }
        (n as f64 / d as f64, Some(d))
    } else {
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        (value.parse::<f64>().ok()?, None)
    };
    (number.is_finite() && number > 0.0).then_some((number, denominator))
}

fn read_candidate(name: &str, path: &Path) -> Option<FocalOverride> {
    if !path.extension()?.eq_ignore_ascii_case("png") {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let original = file.metadata().ok()?;
    let size = original.len();
    let mut magic = [0; 8];
    file.read_exact(&mut magic).ok()?;
    if magic != *b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut dimensions = None;
    let mut fields = None;
    let mut remaining = METADATA_BUDGET;
    let mut saw_data = false;
    let mut ended = false;
    for index in 0..4096 {
        let mut header = [0; 8];
        file.read_exact(&mut header).ok()?;
        let len = u32::from_be_bytes(header[..4].try_into().ok()?) as u64;
        let kind = &header[4..];
        let end = file
            .stream_position()
            .ok()?
            .checked_add(len)?
            .checked_add(4)?;
        if end > size || (index == 0 && kind != b"IHDR") {
            return None;
        }
        if kind == b"eXIf" {
            return None;
        }
        if kind == b"IDAT" {
            saw_data = true;
        }
        if matches!(kind, b"IHDR" | b"iTXt" | b"tEXt" | b"zTXt" | b"IEND") {
            if kind == b"IHDR" && (index != 0 || len != 13) {
                return None;
            }
            let len: usize = len.try_into().ok()?;
            if len > remaining {
                return None;
            }
            remaining -= len;
            let mut data = vec![0; len];
            file.read_exact(&mut data).ok()?;
            let mut crc = [0; 4];
            file.read_exact(&mut crc).ok()?;
            let mut check = crc32fast::Hasher::new();
            check.update(kind);
            check.update(&data);
            if check.finalize() != u32::from_be_bytes(crc) {
                return None;
            }
            match kind {
                b"IHDR" => {
                    let width = u32::from_be_bytes(data[..4].try_into().ok()?);
                    let height = u32::from_be_bytes(data[4..8].try_into().ok()?);
                    if width == 0 || height == 0 {
                        return None;
                    }
                    dimensions = Some((width, height));
                }
                b"iTXt" | b"tEXt" | b"zTXt" => {
                    let zero = data.iter().position(|b| *b == 0)?;
                    let keyword = std::str::from_utf8(&data[..zero]).ok()?;
                    if keyword.eq_ignore_ascii_case("Raw profile type exif")
                        || keyword.to_ascii_lowercase().contains("focal")
                        || keyword.eq_ignore_ascii_case("Orientation")
                    {
                        return None;
                    }
                    if keyword.eq_ignore_ascii_case("XML:com.adobe.xmp") {
                        // OIIO reads metadata with png_read_info, before IDAT.
                        if kind != b"iTXt" || saw_data || fields.is_some() {
                            return None;
                        }
                        let text = data.get(zero + 1..)?;
                        let flag = *text.first()?;
                        if flag > 1 || *text.get(1)? != 0 {
                            return None;
                        }
                        let mut payload = text.get(2..)?;
                        for _ in 0..2 {
                            let zero = payload.iter().position(|b| *b == 0)?;
                            payload = payload.get(zero + 1..)?;
                        }
                        let mut inflated = Vec::new();
                        let xml = if flag == 1 {
                            let mut decoder = flate2::read::ZlibDecoder::new(payload);
                            decoder
                                .by_ref()
                                .take(remaining as u64 + 1)
                                .read_to_end(&mut inflated)
                                .ok()?;
                            if inflated.len() > remaining
                                || decoder.total_in() != payload.len() as u64
                            {
                                return None;
                            }
                            remaining -= inflated.len();
                            inflated.as_slice()
                        } else {
                            payload
                        };
                        fields = Some(xmp_fields(xml)?);
                    }
                }
                b"IEND" => {
                    ended = len == 0 && saw_data && end == size;
                    break;
                }
                _ => {}
            }
        } else {
            file.seek(SeekFrom::Start(end)).ok()?;
        }
    }
    if !ended {
        return None;
    }
    let (width, height) = dimensions?;
    let fields = fields?;
    if fields.contains_key("equivalent") || fields.get("orientation").is_some_and(|v| v != "1") {
        return None;
    }
    let mut verified_dimensions = false;
    for (wk, hk) in [("width", "height"), ("pixel_width", "pixel_height")] {
        match (fields.get(wk), fields.get(hk)) {
            (Some(w), Some(h))
                if w.parse::<u32>().ok()? == width && h.parse::<u32>().ok()? == height =>
            {
                verified_dimensions = true
            }
            (None, None) => {}
            _ => return None,
        }
    }
    if !verified_dimensions {
        return None;
    }
    let raw_focal_length = fields.get("focal")?.clone();
    let raw_x_resolution = fields.get("xres")?.clone();
    let focal = positive_number(&raw_focal_length)?.0;
    let (resolution, denominator) = positive_number(&raw_x_resolution)?;
    if denominator.is_none_or(|d| d == 1) {
        return None;
    }
    let unit = fields.get("unit")?.parse::<u32>().ok()?;
    let pixels_per_mm = match unit {
        2 => resolution / 25.4,
        3 => resolution / 10.0,
        4 => resolution,
        5 => resolution * 1000.0,
        _ => return None,
    };
    let f = focal * pixels_per_mm;
    if !f.is_finite() || f <= 0.0 {
        return None;
    }
    file.rewind().ok()?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).ok()?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    let after = file.metadata().ok()?;
    if after.len() != size || after.modified().ok()? != original.modified().ok()? {
        return None;
    }
    Some(FocalOverride {
        checkpoint: Checkpoint {
            version: POLICY_VERSION,
            image_name: name.to_owned(),
            sha256: format!("{:x}", hash.finalize()),
            width,
            height,
            camera_params: [f, width as f64 / 2.0, height as f64 / 2.0, 0.0],
        },
        raw_focal_length,
        raw_x_resolution,
        unit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xml(extra: &str) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><r:Description xmlns:e="http://ns.adobe.com/exif/1.0/" xmlns:t="http://ns.adobe.com/tiff/1.0/" t:ImageWidth="8192" t:ImageLength="5464" e:FocalLength="100/1" e:FocalPlaneXResolution="8192000/1419" e:FocalPlaneResolutionUnit="2" {extra}/></r:RDF></x:xmpmeta>"#
        )
    }

    fn chunk(file: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        file.extend((data.len() as u32).to_be_bytes());
        file.extend(kind);
        file.extend(data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(data);
        file.extend(crc.finalize().to_be_bytes());
    }

    fn png(xml: &str, compressed: bool, exif: bool, after_data: bool) -> Vec<u8> {
        use std::io::Write;
        let mut file = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend(8192_u32.to_be_bytes());
        ihdr.extend(5464_u32.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        chunk(&mut file, b"IHDR", &ihdr);
        if exif {
            chunk(&mut file, b"eXIf", b"uncertain");
        }
        if after_data {
            chunk(&mut file, b"IDAT", &[]);
        }
        let mut text = b"XML:com.adobe.xmp\0".to_vec();
        text.extend([u8::from(compressed), 0, 0, 0]);
        if compressed {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(xml.as_bytes()).unwrap();
            text.extend(encoder.finish().unwrap());
        } else {
            text.extend(xml.as_bytes());
        }
        chunk(&mut file, b"iTXt", &text);
        if !after_data {
            chunk(&mut file, b"IDAT", &[]);
        }
        chunk(&mut file, b"IEND", &[]);
        file
    }

    fn inspect_bytes(bytes: &[u8]) -> Option<FocalOverride> {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("first.png");
        std::fs::write(&path, bytes).unwrap();
        let result = read_candidate("first.png", &path);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        result
    }

    #[test]
    fn rational_example_namespaces_elements_and_units() {
        for compressed in [false, true] {
            let example = inspect_bytes(&png(&xml(""), compressed, false, false)).unwrap();
            assert!((example.checkpoint.camera_params[0] - 22728.65980253445).abs() < 0.01);
            assert_eq!(
                &example.checkpoint.camera_params[1..],
                &[4096.0, 2732.0, 0.0]
            );
            assert_eq!(example.raw_x_resolution, "8192000/1419");
            assert_eq!(example.unit, 2);
            assert_eq!(example.checkpoint.sha256.len(), 64);
        }
        let elements = xml("")
            .replace("e:FocalLength=\"100/1\"", "")
            .replace("e:FocalPlaneXResolution=\"8192000/1419\"", "")
            .replace("e:FocalPlaneResolutionUnit=\"2\"", "")
            .replace("/></r:RDF>", "><e:FocalLength>100/1</e:FocalLength><e:FocalPlaneXResolution>8192000/1419</e:FocalPlaneXResolution><e:FocalPlaneResolutionUnit>2</e:FocalPlaneResolutionUnit></r:Description></r:RDF>");
        assert!(inspect_bytes(&png(&elements, false, false, false)).is_some());
        let renamed = xml("")
            .replace("xmlns:e=", "xmlns:camera=")
            .replace("e:Focal", "camera:Focal");
        assert!(inspect_bytes(&png(&renamed, false, false, false)).is_some());
        for (unit, factor) in [(2, 1.0 / 25.4), (3, 0.1), (4, 1.0), (5, 1000.0)] {
            let xml = xml("").replace("Unit=\"2\"", &format!("Unit=\"{unit}\""));
            let result = inspect_bytes(&png(&xml, false, false, false)).unwrap();
            let expected = 100.0 * (8192000.0 / 1419.0) * factor;
            assert!((result.checkpoint.camera_params[0] - expected).abs() < expected * 1e-12);
        }
    }

    #[test]
    fn malformed_or_uncertain_metadata_never_changes_the_input() {
        for value in [
            "8192000/0",
            "8192000/1419junk",
            "8192000/1419/2",
            "-1/2",
            "NaN",
            "inf",
            "1e309",
            "0/2",
            "8192000/1",
            "8192000",
            "18446744073709551616/2",
        ] {
            let xml = xml("").replace("8192000/1419", value);
            assert!(
                inspect_bytes(&png(&xml, false, false, false)).is_none(),
                "{value}"
            );
        }
        for extra in [
            "e:FocalLengthIn35mmFilm=\"100\"",
            "e:FocalLengthIn35mmFilm=\"0\"",
            "t:Orientation=\"6\"",
        ] {
            assert!(inspect_bytes(&png(&xml(extra), false, false, false)).is_none());
        }
        for bad in [
            xml("").replace("http://ns.adobe.com/exif/1.0/", "http://example.invalid/"),
            xml("").replace("8192", "4096"),
            xml("").replace("Unit=\"2\"", "Unit=\"1\""),
            xml("").replace("FocalLength=\"100/1\"", "FocalLength=\"0/1\""),
            xml("").replace(
                "/></r:RDF>",
                "><e:FocalLength>101/1</e:FocalLength></r:Description></r:RDF>",
            ),
            format!("<!DOCTYPE x [<!ENTITY boom '100'>]>{}", xml("")),
            format!("{}<extra/>", xml("")),
            format!("{}garbage", xml("")),
            xml("padding=\"&unresolved;\""),
            "<broken".into(),
        ] {
            assert!(inspect_bytes(&png(&bad, false, false, false)).is_none());
        }
        assert!(inspect_bytes(&png(&xml(""), false, true, false)).is_none());
        assert!(inspect_bytes(&png(&xml(""), false, false, true)).is_none());
        let mut corrupt = png(&xml(""), false, false, false);
        corrupt[29] ^= 1; // IHDR CRC
        assert!(inspect_bytes(&corrupt).is_none());
        let mut truncated = png(&xml(""), false, false, false);
        truncated.truncate(truncated.len() - 1);
        assert!(inspect_bytes(&truncated).is_none());
        assert!(inspect_bytes(&png(
            &xml(&format!("padding=\"{}\"", " ".repeat(METADATA_BUDGET))),
            true,
            false,
            false
        ))
        .is_none());
        assert!(inspect_bytes(b"not a PNG").is_none());
    }

    #[test]
    fn colmap_first_image_uses_sorted_work_names_and_sorted_image_list() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["frame_10.png", "frame_2.png", "frame_1.jpg"] {
            std::fs::write(temp.path().join(name), png(&xml(""), false, false, false)).unwrap();
        }
        let (name, path) = first_image(temp.path(), None).unwrap();
        assert_eq!(name, "frame_1.jpg");
        assert!(read_candidate(&name, &path).is_none()); // Never search for a later PNG.
        let list = temp.path().join("focal-test-list.txt");
        std::fs::write(&list, "frame_2.png\nframe_10.png\n").unwrap();
        assert_eq!(
            first_image(temp.path(), Some(&list)).unwrap().0,
            "frame_10.png"
        );
        assert_eq!(
            first_named_image(temp.path(), &["frame_2.png".into(), "frame_10.png".into()])
                .unwrap()
                .0,
            "frame_10.png",
            "preflight and the later image_list must choose the same dictionary order"
        );
        assert_eq!(
            first_named_image(temp.path(), &["frame_2.png".into()])
                .unwrap()
                .0,
            "frame_2.png",
            "unlisted directory images must not initialize this camera"
        );
        for name in [
            "../outside.png",
            " frame_2.png",
            "frame_2.png\nframe_10.png",
        ] {
            assert!(first_named_image(temp.path(), &[name.into()]).is_none());
        }
        std::fs::write(&list, "../outside.png\n").unwrap();
        assert!(first_image(temp.path(), Some(&list)).is_none());
    }

    #[test]
    fn hardlinked_png_is_read_only_and_checkpoint_state_is_optional() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original.png");
        let work = temp.path().join("frame_000001.png");
        let bytes = png(&xml(""), false, false, false);
        std::fs::write(&original, &bytes).unwrap();
        std::fs::hard_link(&original, &work).unwrap();
        let modified = std::fs::metadata(&original).unwrap().modified().unwrap();
        let focal = read_candidate("frame_000001.png", &work).unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&original).unwrap().modified().unwrap(),
            modified
        );
        let mut state = crate::project::PipelineStateFile::created(crate::presets::Quality::Fast);
        let old = serde_json::to_vec(&state).unwrap();
        assert!(!String::from_utf8_lossy(&old).contains("temporaryPngFocal"));
        assert!(
            serde_json::from_slice::<crate::project::PipelineStateFile>(&old)
                .unwrap()
                .temporary_png_focal
                .is_none()
        );
        state.temporary_png_focal = Some(focal.checkpoint);
        let restored: crate::project::PipelineStateFile =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(restored.temporary_png_focal, state.temporary_png_focal);
    }

    #[test]
    fn runtime_package_must_match_the_frozen_affected_versions() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let binary = bin.join("colmap");
        let mut info = serde_json::json!({"colmapVersion": AFFECTED_COLMAP,
            "sourceCommit": AFFECTED_COMMIT, "runtimeRevision": 1,
            "dependencyVersions": [{"name": "openimageio", "version": AFFECTED_OIIO}] });
        let components = serde_json::json!({"components": [
            {"name": "COLMAP", "version": AFFECTED_COLMAP, "source": AFFECTED_COMMIT},
            {"name": "openimageio", "version": AFFECTED_OIIO}] });
        std::fs::write(
            temp.path().join("BUNDLED-COMPONENTS.json"),
            serde_json::to_vec(&components).unwrap(),
        )
        .unwrap();
        let write_info = |info: &serde_json::Value| {
            std::fs::write(
                temp.path().join("BUILD-INFO.json"),
                serde_json::to_vec(info).unwrap(),
            )
            .unwrap()
        };
        write_info(&info);
        assert!(runtime_metadata_matches(&binary));
        info["dependencyVersions"][0]["version"] = "3.2.0".into();
        write_info(&info);
        assert!(!runtime_metadata_matches(&binary));
        info["dependencyVersions"][0]["version"] = AFFECTED_OIIO.into();
        info["runtimeRevision"] = 2.into();
        write_info(&info);
        assert!(!runtime_metadata_matches(&binary));
    }

    #[tokio::test]
    async fn explicit_calibration_other_models_and_non_images_are_not_overridden() {
        let missing = Path::new("not-an-engine");
        let manager = ProcessManager::new();
        for case in 0..6 {
            let mut request = Request {
                images: missing,
                image_list: None,
                ordinary_images: true,
                camera_model: "SIMPLE_RADIAL",
                single_camera: true,
                explicit_params: None,
                existing_camera_id: None,
                calibrated: false,
            };
            match case {
                0 => request.ordinary_images = false,
                1 => request.camera_model = "EQUIRECTANGULAR",
                2 => request.single_camera = false,
                3 => request.explicit_params = Some("100,20,30,0.1"),
                4 => request.existing_camera_id = Some(1),
                _ => request.calibrated = true,
            }
            assert!(!request.eligible());
            assert!(detect(missing, &manager, request).await.is_none());
        }
        assert!(affected_banner("COLMAP 4.2.1 (Commit bd1fcf6 on date)"));
        assert!(!affected_banner("COLMAP 4.2.1 (Commit bd1fcf60 on date)"));
        assert!(!affected_banner("COLMAP 4.3.0 (Commit bd1fcf6 on date)"));
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("input.png");
        std::fs::write(&path, png(&xml(""), false, false, false)).unwrap();
        assert!(detect(
            missing,
            &manager,
            Request {
                images: temp.path(),
                image_list: None,
                ordinary_images: true,
                camera_model: "SIMPLE_RADIAL",
                single_camera: true,
                explicit_params: None,
                existing_camera_id: None,
                calibrated: false
            }
        )
        .await
        .is_none());
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Read-only original JPG/PNG + bundled COLMAP; writes new isolated diagnostic files only"]
    async fn bundled_png_focal_regression() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let source = Path::new("D:/浏览器下载");
        let executable = root.join("engines/colmap/bin/colmap.exe");
        let output = tempfile::Builder::new()
            .prefix("png-focal-workaround-")
            .tempdir_in(root.join(".tmp"))
            .unwrap()
            .keep();
        let manager = ProcessManager::new();
        let mut evidence = Vec::new();
        for extension in ["JPG", "png"] {
            let original = source.join(format!("003A9246.{extension}"));
            let before = format!("{:x}", Sha256::digest(std::fs::read(&original).unwrap()));
            let case = output.join(extension);
            let images = case.join("images");
            std::fs::create_dir_all(&images).unwrap();
            let name = format!("frame_000001.{extension}");
            let work_image = images.join(&name);
            std::fs::copy(&original, &work_image).unwrap();
            let focal = detect(
                &executable,
                &manager,
                Request {
                    images: &images,
                    image_list: None,
                    ordinary_images: true,
                    camera_model: "SIMPLE_RADIAL",
                    single_camera: true,
                    explicit_params: None,
                    existing_camera_id: None,
                    calibrated: false,
                },
            )
            .await;
            assert_eq!(focal.is_some(), extension == "png");
            let params = focal.as_ref().map(FocalOverride::camera_params);
            let database = case.join("database.db");
            crate::engines::colmap::extract_features_quality_v2(
                &executable,
                &database,
                Path::new("images"),
                None,
                None,
                512,
                512,
                case.join("colmap.log"),
                &manager,
                None,
                None,
                params.as_deref(),
            )
            .await
            .unwrap();
            let db = rusqlite::Connection::open_with_flags(
                &database,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let (model, width, height, bytes, prior): (u32, u32, u32, Vec<u8>, bool) = db
                .query_row(
                    "SELECT model,width,height,params,prior_focal_length FROM cameras",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .unwrap();
            let params: Vec<f64> = bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|v| f64::from_le_bytes(*v))
                .collect();
            assert_eq!((model, width, height, prior), (2, 8192, 5464, true));
            assert!((params[0] - 22728.65980253445).abs() < 0.01);
            assert_eq!(&params[1..], &[4096.0, 2732.0, 0.0]);
            assert_eq!(
                format!("{:x}", Sha256::digest(std::fs::read(&original).unwrap())),
                before
            );
            assert_eq!(
                format!("{:x}", Sha256::digest(std::fs::read(&work_image).unwrap())),
                before
            );
            evidence.push(serde_json::json!({ "input": original, "sha256": before,
                "workImage": work_image, "cameraParams": params, "override": focal.map(|v| v.checkpoint), "database": database }));
        }
        std::fs::write(
            output.join("result.json"),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
        println!("PNG focal real regression: {}", output.display());
    }
}
