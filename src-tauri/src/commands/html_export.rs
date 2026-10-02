//! Offline export: model bytes never cross IPC and never become one large String.
use super::{
    catalog, parse_project_id, preview_client_path, GaussianVideoExportReservation,
    PreviewController,
};
use crate::{
    error::{Result, SplatError},
    project::GaussianTransform,
    reconstruction::{
        edit_mask::read_mask,
        ply::{inspect_gaussian_ply, PlyInfo},
        splat_transform::{export_transformed_ply_to, GaussianExportEdits},
    },
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};
use tauri::{Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub enum HtmlLocale {
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en")]
    English,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HtmlView {
    target: [f64; 3],
    yaw: f64,
    pitch: f64,
    distance: f64,
    horizontal_frame_offset: f64,
    projection: u32,
    ortho_height: f64,
    orthographic_view: Option<String>,
    fov: f64,
    near_clip: f64,
    far_clip: f64,
}
impl HtmlView {
    fn validate(&self) -> Result<()> {
        let values = self.target.into_iter().chain([
            self.yaw,
            self.pitch,
            self.distance,
            self.horizontal_frame_offset,
            self.ortho_height,
            self.fov,
            self.near_clip,
            self.far_clip,
        ]);
        if values.into_iter().any(|v| !v.is_finite() || v.abs() > 1e15)
            || self.distance <= 0.0
            || self.ortho_height <= 0.0
            || !(0.0..180.0).contains(&self.fov)
            || self.fov == 0.0
            || self.near_clip <= 0.0
            || self.far_clip <= self.near_clip
            || self.projection > 1
            || self
                .orthographic_view
                .as_deref()
                .is_some_and(|v| !["side", "front", "top"].contains(&v))
        {
            return Err(SplatError::Process("Invalid HTML camera state".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(super) struct HtmlExportSession {
    pub(super) export_id: Uuid,
    pub(super) project_id: Uuid,
    pub(super) cancel: CancellationToken,
    pub(super) running: bool,
    root: PathBuf,
    destination: PathBuf,
    revision: u64,
    transform: GaussianTransform,
    view: HtmlView,
    locale: HtmlLocale,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HtmlExportProgress {
    export_id: Uuid,
    phase: &'static str,
    progress: f64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HtmlExportResult {
    path: PathBuf,
    file_size: u64,
    splat_count: u64,
}

fn next_html_path(root: &Path) -> PathBuf {
    for n in 1_u64.. {
        let path = root.join(if n == 1 {
            "preview.html".into()
        } else {
            format!("preview-{n}.html")
        });
        if !path.exists() {
            return path;
        }
    }
    unreachable!()
}
fn cancelled(token: &CancellationToken) -> Result<()> {
    if token.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

fn claim_session(slot: &mut Option<HtmlExportSession>, id: Uuid) -> Result<HtmlExportSession> {
    let session = slot
        .as_mut()
        .filter(|session| session.export_id == id && !session.running)
        .ok_or_else(|| {
            SplatError::Process("HTML export token is expired or already in use".into())
        })?;
    session.running = true;
    Ok(session.clone())
}

#[tauri::command]
pub async fn begin_gaussian_html_export(
    state: State<'_, PreviewController>,
    project_id: String,
    edit_revision: u64,
    view: HtmlView,
    locale: HtmlLocale,
) -> Result<GaussianVideoExportReservation> {
    view.validate()?;
    let _lifecycle = state.lifecycle.lock().await;
    let id = parse_project_id(&project_id)?;
    if state
        .active
        .lock()
        .await
        .as_ref()
        .is_none_or(|s| s.project_id != id)
    {
        return Err(SplatError::Process(
            "Open this project in preview before exporting".into(),
        ));
    }
    if state.video_export.lock().await.is_some() {
        return Err(SplatError::Process(
            "A video export is already running".into(),
        ));
    }
    let mut slot = state.html_export.lock().await;
    if slot.is_some() {
        return Err(SplatError::Process(
            "An HTML export is already running".into(),
        ));
    }
    let (root, _, metadata) = catalog::registered_final_ply_for_project(id).await?;
    if metadata.editing.revision != edit_revision {
        return Err(SplatError::Process(
            "Edit revision conflict; wait for edits to save and retry".into(),
        ));
    }
    let export_id = Uuid::new_v4();
    let destination = next_html_path(&root);
    *slot = Some(HtmlExportSession {
        export_id,
        project_id: id,
        cancel: CancellationToken::new(),
        running: false,
        root,
        destination: destination.clone(),
        revision: edit_revision,
        transform: metadata.transform,
        view,
        locale,
    });
    Ok(GaussianVideoExportReservation {
        export_id,
        destination_path: preview_client_path(&destination),
    })
}

#[tauri::command]
pub async fn cancel_gaussian_html_export(
    state: State<'_, PreviewController>,
    export_id: String,
) -> Result<()> {
    let id = Uuid::parse_str(&export_id)
        .map_err(|_| SplatError::Process("Invalid HTML export token".into()))?;
    let mut slot = state.html_export.lock().await;
    if let Some(session) = slot.as_ref().filter(|s| s.export_id == id) {
        session.cancel.cancel();
        if !session.running {
            *slot = None;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn commit_gaussian_html_export(
    app: tauri::AppHandle,
    state: State<'_, PreviewController>,
    export_id: String,
) -> Result<HtmlExportResult> {
    let id = Uuid::parse_str(&export_id)
        .map_err(|_| SplatError::Process("Invalid HTML export token".into()))?;
    let session = {
        let mut slot = state.html_export.lock().await;
        claim_session(&mut slot, id)?
    };
    let result = run_export(&app, &state, &session).await;
    let mut slot = state.html_export.lock().await;
    if slot.as_ref().is_some_and(|s| s.export_id == id) {
        *slot = None;
    }
    result
}

async fn run_export(
    app: &tauri::AppHandle,
    state: &PreviewController,
    session: &HtmlExportSession,
) -> Result<HtmlExportResult> {
    let _export = state.export.lock().await;
    let _metadata = state.metadata_write.lock().await;
    cancelled(&session.cancel)?;
    let (root, source, metadata) =
        catalog::registered_final_ply_for_project(session.project_id).await?;
    if metadata.editing.revision != session.revision || metadata.transform != session.transform {
        return Err(SplatError::Process(
            "Edit revision conflict; wait for edits to save and retry".into(),
        ));
    }
    let runtime = app
        .path()
        .resolve(
            "html-viewer/runtime.js",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|error| {
            SplatError::Process(format!("Could not locate offline viewer: {error}"))
        })?;
    // The dev viewer is built before Tauri starts, just like the production resource.
    let runtime = if runtime.is_file() {
        runtime
    } else if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.cache/html-viewer/runtime.js")
    } else {
        runtime
    };
    let session = session.clone();
    let emitter = app.clone();
    tokio::task::spawn_blocking(move || {
        let info = inspect_gaussian_ply(&source)?;
        let editing = metadata.editing.validate(info.splat_count)?;
        let mask = read_mask(&root, editing)?;
        let model = session
            .root
            .join(format!(".ooosplat-html-{}.ply.tmp", session.export_id));
        let html = session
            .root
            .join(format!(".ooosplat-html-{}.html.tmp", session.export_id));
        let mut last_progress = None;
        let mut progress = |phase: &'static str, p: f64| {
            let key = (phase, p.floor() as u32);
            if last_progress != Some(key) {
                let _ = emitter.emit(
                    "gaussian-html-export-progress",
                    HtmlExportProgress {
                        export_id: session.export_id,
                        phase,
                        progress: p,
                    },
                );
                last_progress = Some(key);
            }
        };
        let result = (|| {
            progress("model", 0.0);
            let info = export_transformed_ply_to(
                &source,
                &model,
                session.transform,
                GaussianExportEdits {
                    crop: editing.crop,
                    deleted_mask: Some(&mask),
                },
                |done, total| progress("model", done as f64 / total.max(1) as f64 * 45.0),
                || cancelled(&session.cancel),
            )?;
            progress("packing", 45.0);
            write_html(
                &model,
                &html,
                &runtime,
                &session.view,
                session.locale,
                &info,
                &session.cancel,
                |done, total| progress("packing", 45.0 + done as f64 / total.max(1) as f64 * 54.0),
            )?;
            cancelled(&session.cancel)?;
            // Same-volume hard link publishes a complete file atomically without
            // overwriting an existing user file, unlike rename on Unix.
            publish_html_file(&html, &session.destination)?;
            progress("completed", 100.0);
            Ok(HtmlExportResult {
                path: preview_client_path(&session.destination),
                file_size: std::fs::metadata(&session.destination)?.len(),
                splat_count: info.splat_count,
            })
        })();
        let _ = std::fs::remove_file(&model);
        let _ = std::fs::remove_file(&html);
        result
    })
    .await
    .map_err(|e| SplatError::Process(format!("HTML export worker failed: {e}")))?
}

const CHUNK_BYTES: usize = 768 * 1024;
#[cfg(windows)]
pub(super) fn publish_html_file(source: &Path, destination: &Path) -> Result<()> {
    use std::{iter, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
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
    // SAFETY: Owned, NUL-terminated paths remain alive for the entire call.
    // Omitting REPLACE_EXISTING guarantees that unrelated files are never overwritten.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
#[cfg(not(windows))]
pub(super) fn publish_html_file(source: &Path, destination: &Path) -> Result<()> {
    publish_html_file_by_link(source, destination)
}
#[cfg(any(not(windows), test))]
fn publish_html_file_by_link(source: &Path, destination: &Path) -> Result<()> {
    std::fs::hard_link(source, destination)?;
    // Consume the temporary path just like MoveFileExW: otherwise reusing it
    // would modify the published file through the shared hard-link inode.
    std::fs::remove_file(source)?;
    Ok(())
}
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
fn encode_base64(bytes: &[u8], output: &mut Vec<u8>) {
    output.clear();
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.extend_from_slice(&[
            BASE64[(a >> 2) as usize],
            BASE64[(((a & 3) << 4) | (b >> 4)) as usize],
            if chunk.len() > 1 {
                BASE64[(((b & 15) << 2) | (c >> 6)) as usize]
            } else {
                b'='
            },
            if chunk.len() > 2 {
                BASE64[(c & 63) as usize]
            } else {
                b'='
            },
        ]);
    }
}
fn safe_json(value: &impl Serialize) -> Result<String> {
    Ok(serde_json::to_string(value)?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029"))
}

#[allow(clippy::too_many_arguments)]
fn write_html(
    model: &Path,
    output: &Path,
    runtime: &Path,
    view: &HtmlView,
    locale: HtmlLocale,
    info: &PlyInfo,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<()> {
    cancelled(cancel)?;
    let mut writer = BufWriter::new(
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(output)?,
    );
    let language = match locale {
        HtmlLocale::Chinese => "zh-CN",
        HtmlLocale::English => "en",
    };
    write!(writer,"<!doctype html><html lang=\"{language}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>OOOSplat Preview</title><style>{}</style></head><body>",include_str!("../../../html-viewer/style.css"))?;
    writer.write_all(include_str!("../../../html-viewer/body.html").as_bytes())?;
    write!(
        writer,
        "<script type=\"application/json\" id=\"viewer-config\">{}</script>",
        safe_json(&serde_json::json!({"view":view,"locale":locale,"splatCount":info.splat_count}))?
    )?;
    let mut bytes = vec![0_u8; CHUNK_BYTES];
    let mut encoded = Vec::with_capacity(CHUNK_BYTES / 3 * 4 + 4);
    let mut reader = BufReader::new(File::open(model)?);
    let mut done = 0;
    loop {
        cancelled(cancel)?;
        let read = reader.read(&mut bytes)?;
        if read == 0 {
            break;
        }
        encode_base64(&bytes[..read], &mut encoded);
        writer.write_all(b"<script type=\"application/octet-stream\" data-ply-chunk>")?;
        writer.write_all(&encoded)?;
        writer.write_all(b"</script>")?;
        done += read as u64;
        progress(done, info.file_size);
    }
    writer.write_all(b"<script type=\"application/octet-stream\" id=\"viewer-runtime\">")?;
    let mut runtime_bytes = Vec::new();
    File::open(runtime)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut runtime_bytes)?;
    if runtime_bytes.len() > 16 * 1024 * 1024 {
        return Err(SplatError::Process(
            "Offline runtime exceeds safety limit".into(),
        ));
    }
    encode_base64(&runtime_bytes, &mut encoded);
    writer.write_all(&encoded)?;
    writer.write_all(b"</script><script>const r=document.getElementById('viewer-runtime');const s=document.createElement('script');s.textContent=new TextDecoder().decode(Uint8Array.from(atob(r.textContent),c=>c.charCodeAt(0)));r.remove();document.body.append(s);</script><details id=\"licenses\"><summary>Licenses</summary><pre>")?;
    for text in [
        include_str!("../../../licenses/PlayCanvas-MIT.txt"),
        include_str!("../../../LICENSE"),
        include_str!("../../../NOTICE"),
    ] {
        writer.write_all(
            text.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .as_bytes(),
        )?;
    }
    writer.write_all(b"</pre></details></body></html>")?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base64_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            let mut encoded = Vec::new();
            encode_base64(input.as_bytes(), &mut encoded);
            assert_eq!(encoded, expected.as_bytes());
        }
    }
    #[test]
    fn safe_script_json() {
        assert!(!safe_json(&"</script><script>alert(1)</script>")
            .unwrap()
            .contains('<'));
    }
    #[test]
    fn automatic_numbering() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(next_html_path(dir.path()), dir.path().join("preview.html"));
        std::fs::write(dir.path().join("preview.html"), b"old").unwrap();
        assert_eq!(
            next_html_path(dir.path()),
            dir.path().join("preview-2.html")
        );
    }
    #[test]
    fn cancellation_is_structured() {
        let token = CancellationToken::new();
        token.cancel();
        assert!(matches!(cancelled(&token), Err(SplatError::Cancelled)));
    }
    #[test]
    fn publication_is_atomic_and_never_overwrites() {
        check_publication(publish_html_file);
    }

    #[test]
    fn hard_link_publication_consumes_the_source_without_overwriting() {
        // Exercise the Unix path on Windows too, so CI-only differences cannot regress.
        check_publication(publish_html_file_by_link);
    }

    fn check_publication(publish: fn(&Path, &Path) -> Result<()>) {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("temp");
        let target = root.path().join("preview.html");
        std::fs::write(&source, b"complete").unwrap();
        publish(&source, &target).unwrap();
        assert!(
            !source.exists(),
            "publication must consume the temporary path"
        );
        std::fs::write(&source, b"new").unwrap();
        assert!(publish(&source, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"complete");
        assert_eq!(std::fs::read(&source).unwrap(), b"new");
    }

    fn view() -> HtmlView {
        HtmlView {
            target: [0.0; 3],
            yaw: 35.0,
            pitch: 22.0,
            distance: 8.0,
            horizontal_frame_offset: 0.0,
            projection: 0,
            ortho_height: 5.0,
            orthographic_view: None,
            fov: 52.0,
            near_clip: 0.01,
            far_clip: 10000.0,
        }
    }
    #[test]
    fn validates_camera_payload() {
        let mut camera = view();
        assert!(camera.validate().is_ok());
        camera.distance = 0.0;
        assert!(camera.validate().is_err());
        camera = view();
        camera.target[0] = f64::NAN;
        assert!(camera.validate().is_err());
        camera = view();
        camera.orthographic_view = Some("</script>".into());
        assert!(camera.validate().is_err());
        camera = view();
        camera.far_clip = camera.near_clip;
        assert!(camera.validate().is_err());
    }

    #[test]
    fn forged_expired_and_reused_tokens_are_rejected() {
        let id = Uuid::new_v4();
        let mut slot = None;
        assert!(claim_session(&mut slot, id).is_err());
        slot = Some(HtmlExportSession {
            export_id: id,
            project_id: Uuid::new_v4(),
            cancel: CancellationToken::new(),
            running: false,
            root: PathBuf::new(),
            destination: PathBuf::new(),
            revision: 0,
            transform: GaussianTransform::default(),
            view: view(),
            locale: HtmlLocale::English,
        });
        assert!(claim_session(&mut slot, Uuid::new_v4()).is_err());
        assert!(!slot.as_ref().unwrap().running);
        assert_eq!(claim_session(&mut slot, id).unwrap().export_id, id);
        assert!(claim_session(&mut slot, id).is_err());
    }
    #[test]
    fn streaming_html_is_self_contained_and_does_not_touch_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.ply");
        let runtime = dir.path().join("runtime.js");
        let output = dir.path().join("preview.html");
        let bytes = vec![42_u8; CHUNK_BYTES + 1];
        std::fs::write(&model, &bytes).unwrap();
        std::fs::write(&runtime, b"console.log('offline');").unwrap();
        let info = PlyInfo {
            file_size: bytes.len() as u64,
            splat_count: 1,
        };
        let mut updates = Vec::new();
        write_html(
            &model,
            &output,
            &runtime,
            &view(),
            HtmlLocale::English,
            &info,
            &CancellationToken::new(),
            |done, total| updates.push((done, total)),
        )
        .unwrap();
        let html = std::fs::read_to_string(output).unwrap();
        assert!(html.matches(" data-ply-chunk>").count() > 1);
        assert!(html.starts_with("<!doctype html><html lang=\"en\">"));
        assert!(html.contains("id=\"viewer-runtime\""));
        assert!(!html.contains("src=\"http"));
        assert!(!html.contains(dir.path().to_str().unwrap()));
        assert_eq!(std::fs::read(&model).unwrap(), bytes);
        assert_eq!(updates.last(), Some(&(info.file_size, info.file_size)));
    }
    #[test]
    fn cancelled_html_does_not_create_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("preview.html");
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            write_html(
                Path::new("missing"),
                &output,
                Path::new("missing"),
                &view(),
                HtmlLocale::Chinese,
                &PlyInfo {
                    file_size: 0,
                    splat_count: 1
                },
                &cancel,
                |_, _| {}
            ),
            Err(SplatError::Cancelled)
        ));
        assert!(!output.exists());
    }
    #[test]
    #[ignore = "Build the offline runtime first, then generate a browser verification fixture"]
    fn offline_viewer_browser_fixture() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.cache/html-viewer");
        std::fs::create_dir_all(&root).unwrap();
        let properties = [
            "x", "y", "z", "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
            "opacity", "f_dc_0", "f_dc_1", "f_dc_2",
        ];
        let mut bytes = format!(
            "ply\nformat binary_little_endian 1.0\nelement vertex 8\n{}end_header\n",
            properties
                .iter()
                .map(|name| format!("property float {name}\n"))
                .collect::<String>()
        )
        .into_bytes();
        for i in 0_i32..8 {
            for value in [
                ((i & 1) * 2 - 1) as f32,
                (((i >> 1) & 1) * 2 - 1) as f32,
                (((i >> 2) & 1) * 2 - 1) as f32,
                -1.0,
                -1.0,
                -1.0,
                1.0,
                0.0,
                0.0,
                0.0,
                3.0,
                0.1,
                0.4,
                0.8,
            ] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let model = root.join("sample.ply");
        std::fs::write(&model, &bytes).unwrap();
        for (name, locale) in [
            ("sample.html", HtmlLocale::English),
            ("sample-zh.html", HtmlLocale::Chinese),
        ] {
            let output = root.join(name);
            if output.exists() {
                std::fs::remove_file(&output).unwrap();
            }
            write_html(
                &model,
                &output,
                &root.join("runtime.js"),
                &view(),
                locale,
                &inspect_gaussian_ply(&model).unwrap(),
                &CancellationToken::new(),
                |_, _| {},
            )
            .unwrap();
        }
        let html = std::fs::read_to_string(root.join("sample-zh.html")).unwrap();
        std::fs::write(
            root.join("sample-webgl.html"),
            html.replacen(
                "<head>",
                "<head><script>Object.defineProperty(navigator,'gpu',{value:undefined});</script>",
                1,
            ),
        )
        .unwrap();
    }
}
