//! Validate only the current COLMAP feature group's prepared mask contract.
//! An existing masks directory says nothing about another source's masks.
use crate::{
    error::{Result, SplatError},
    reconstruction::direct_input::image_path,
    video::image_projection::inspect,
};
use std::{
    collections::HashSet,
    path::{Component, Path},
};
use tokio_util::sync::CancellationToken;

fn invalid(reason: &str, name: &str) -> SplatError {
    let name: String = name.chars().take(180).collect();
    SplatError::Process(format!(
        "COLMAP feature mask preflight: {reason}; image={name}"
    ))
}

fn contextual(error: SplatError, role: &str, name: &str) -> SplatError {
    let name: String = name.chars().take(180).collect();
    let context = format!("COLMAP feature mask preflight; role={role}; image={name}");
    if error.io_error().is_some() {
        error.with_io_diagnostic(context)
    } else {
        SplatError::Process(format!("{context}: {error}"))
    }
}

/// Use masks only when this group actually has them, never because a different
/// input populated the shared directory. A saved requirement cannot be ignored.
/// Validation reads image headers only and never creates or rewrites any file.
pub(crate) fn use_masks(
    images: &Path,
    masks: &Path,
    names: &[String],
    required: bool,
    cancel: &CancellationToken,
) -> Result<bool> {
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    if names.is_empty() {
        return Err(invalid("empty feature image group", ""));
    }
    let mut unique = HashSet::with_capacity(names.len());
    let mut members = Vec::with_capacity(names.len());
    let mut mask_count = 0;
    for name in names {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        if name.is_empty()
            || name.contains(['\\', ':', '\0', '\n', '\r'])
            || Path::new(name)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(invalid("unsafe feature image name", name));
        }
        if !unique.insert(name) {
            return Err(invalid("duplicate feature image name", name));
        }
        let image = image_path(images, name).map_err(|e| contextual(e, "image", name))?;
        let mask_name = format!("{name}.png");
        let mask = match std::fs::symlink_metadata(masks.join(&mask_name)) {
            Ok(_) => {
                mask_count += 1;
                Some(image_path(masks, &mask_name).map_err(|e| contextual(e, "mask", name))?)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(contextual(error.into(), "mask", name)),
        };
        members.push((name, image, mask));
    }
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    if mask_count == 0 && !required {
        return Ok(false);
    }
    if mask_count != names.len() {
        let missing = members.iter().find(|(_, _, mask)| mask.is_none()).unwrap();
        return Err(invalid("required mask is missing", missing.0));
    }
    for (name, image, mask) in members {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        let image = inspect(&image).map_err(|e| contextual(e, "image header", name))?;
        let mask =
            inspect(mask.as_ref().unwrap()).map_err(|e| contextual(e, "mask header", name))?;
        if (image.width, image.height) != (mask.width, mask.height) {
            return Err(invalid(
                "mask dimensions differ from the prepared image",
                name,
            ));
        }
    }
    if cancel.is_cancelled() {
        return Err(SplatError::Cancelled);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let images = root.path().join("images");
        let masks = root.path().join("masks");
        std::fs::create_dir(&images).unwrap();
        std::fs::create_dir(&masks).unwrap();
        (root, images, masks)
    }
    fn image(path: &Path, width: u32, height: u32) {
        image::RgbImage::new(width, height).save(path).unwrap();
    }
    fn mask(path: &Path, width: u32, height: u32) {
        image::GrayImage::from_pixel(width, height, image::Luma([255]))
            .save(path)
            .unwrap();
    }

    #[test]
    fn unmasked_ordinary_and_erp_groups_ignore_other_sources_masks() {
        for (width, height) in [(16, 12), (16, 8)] {
            let (_root, images, masks) = fixture();
            image(&images.join("frame.jpg"), width, height);
            mask(&masks.join("other-source.png.png"), 4, 4);
            let names = vec!["frame.jpg".into()];
            assert!(!use_masks(&images, &masks, &names, false, &CancellationToken::new()).unwrap());
            assert!(
                use_masks(&images, &masks, &names, true, &CancellationToken::new())
                    .unwrap_err()
                    .to_string()
                    .contains("required mask is missing")
            );
        }
    }

    #[test]
    fn complete_masks_follow_each_image_dimensions_and_do_not_change_files() {
        let (_root, images, masks) = fixture();
        let names: Vec<String> = vec!["小图.png".into(), "large.jpg".into()];
        for (name, width, height) in [(&names[0], 16, 8), (&names[1], 12, 16)] {
            image(&images.join(name), width, height);
            mask(&masks.join(format!("{name}.png")), width, height);
        }
        let files = names
            .iter()
            .flat_map(|name| [images.join(name), masks.join(format!("{name}.png"))])
            .collect::<Vec<_>>();
        let before = files
            .iter()
            .map(|p| std::fs::read(p).unwrap())
            .collect::<Vec<_>>();
        for required in [false, true] {
            assert!(
                use_masks(&images, &masks, &names, required, &CancellationToken::new()).unwrap()
            );
        }
        let after = files
            .iter()
            .map(|p| std::fs::read(p).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(before, after);
    }

    #[test]
    fn incomplete_corrupt_and_wrong_sized_masks_are_never_silently_ignored() {
        let (_root, images, masks) = fixture();
        let names: Vec<String> = vec!["a.png".into(), "b.png".into()];
        for name in &names {
            image(&images.join(name), 16, 8);
        }
        mask(&masks.join("a.png.png"), 16, 8);
        assert!(
            use_masks(&images, &masks, &names, false, &CancellationToken::new())
                .unwrap_err()
                .to_string()
                .contains("required mask is missing; image=b.png")
        );
        mask(&masks.join("b.png.png"), 8, 8);
        assert!(
            use_masks(&images, &masks, &names, false, &CancellationToken::new())
                .unwrap_err()
                .to_string()
                .contains("mask dimensions differ")
        );
        std::fs::write(masks.join("b.png.png"), b"not a PNG header").unwrap();
        assert!(
            use_masks(&images, &masks, &names, false, &CancellationToken::new())
                .unwrap_err()
                .to_string()
                .contains("mask header")
        );
    }

    #[test]
    fn invalid_groups_cancel_and_operational_io_keep_their_true_errors() {
        let (_root, images, masks) = fixture();
        image(&images.join("a.png"), 16, 8);
        for names in [
            vec![],
            vec!["a.png".into(), "a.png".into()],
            vec!["../a.png".into()],
            vec!["a\nb.png".into()],
        ] {
            assert!(matches!(
                use_masks(&images, &masks, &names, false, &CancellationToken::new()),
                Err(SplatError::Process(_))
            ));
        }
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            use_masks(&images, &masks, &["a.png".into()], false, &cancelled),
            Err(SplatError::Cancelled)
        ));
        let error = use_masks(
            &images,
            &masks,
            &["missing.png".into()],
            false,
            &CancellationToken::new(),
        )
        .unwrap_err();
        assert_eq!(
            error.io_error().unwrap().kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(error.to_string().contains("role=image; image=missing.png"));
    }

    #[test]
    fn masked_group_validation_never_decodes_pixel_payloads() {
        fn header(path: &Path) {
            fn chunk(bytes: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
                bytes.extend((body.len() as u32).to_be_bytes());
                let start = bytes.len();
                bytes.extend(kind);
                bytes.extend(body);
                let mut crc = !0_u32;
                for byte in &bytes[start..] {
                    crc ^= u32::from(*byte);
                    for _ in 0..8 {
                        crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
                    }
                }
                bytes.extend((!crc).to_be_bytes());
            }
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            let mut ihdr = 16_u32.to_be_bytes().to_vec();
            ihdr.extend(8_u32.to_be_bytes());
            ihdr.extend([8, 2, 0, 0, 0]);
            chunk(&mut bytes, b"IHDR", &ihdr);
            chunk(&mut bytes, b"IEND", &[]);
            std::fs::write(path, bytes).unwrap();
        }
        let (_root, images, masks) = fixture();
        header(&images.join("a.png"));
        header(&masks.join("a.png.png"));
        assert!(image::open(images.join("a.png")).is_err());
        assert!(image::open(masks.join("a.png.png")).is_err());
        assert!(use_masks(
            &images,
            &masks,
            &["a.png".into()],
            true,
            &CancellationToken::new()
        )
        .unwrap());
    }
}
