//! Preserve original PNG camera metadata when only Alpha premultiplication
//! changes RGB values, not the pixel coordinate map. Metadata is never parsed
//! into intrinsics here. Raw eXIf / standard XMP iTXt chunks are CRC-checked and
//! streamed once into the new PNG; IDAT, ICC and unrelated text are not copied.
use std::{
    fs::{File, Metadata},
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

use image::{codecs::png::PngEncoder, DynamicImage};
use tokio_util::sync::CancellationToken;

use crate::error::{Result, SplatError};

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";

pub(super) struct PngCameraMetadata {
    source: File,
    identity: Metadata,
    dimensions: (u32, u32),
    spans: Vec<(u64, u64)>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid PNG camera metadata")
}

fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(SplatError::Cancelled)
    } else {
        Ok(())
    }
}

fn verify_crc(
    file: &mut File,
    payload: u64,
    length: u32,
    kind: &[u8; 4],
    cancel: &CancellationToken,
) -> Result<()> {
    file.seek(SeekFrom::Start(payload))?;
    let mut hash = crc32fast::Hasher::new();
    hash.update(kind);
    let mut remaining = u64::from(length);
    let mut buffer = [0; 64 * 1024];
    while remaining > 0 {
        check_cancel(cancel)?;
        let count = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    let mut checksum = [0; 4];
    file.read_exact(&mut checksum)?;
    if hash.finalize() != u32::from_be_bytes(checksum) {
        return Err(invalid().into());
    }
    Ok(())
}

impl PngCameraMetadata {
    pub(super) fn read(
        source: &Path,
        dimensions: (u32, u32),
        cancel: &CancellationToken,
    ) -> Result<Option<Self>> {
        check_cancel(cancel)?;
        let mut file = File::open(source)?;
        let identity = file.metadata()?;
        let size = identity.len();
        let mut header = [0; 33];
        if size < 8 {
            return Ok(None);
        }
        file.read_exact(&mut header[..8])?;
        if &header[..8] != SIGNATURE {
            return Ok(None);
        }
        file.read_exact(&mut header[8..])?;
        if header[8..12] != 13_u32.to_be_bytes() || &header[12..16] != b"IHDR" {
            return Err(invalid().into());
        }
        verify_crc(&mut file, 16, 13, b"IHDR", cancel)?;
        let width = u32::from_be_bytes(header[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(header[20..24].try_into().unwrap());
        // A resized/rotated image needs a different calibration, not a copied
        // original. Production callers also omit this source after rotation.
        if (width, height) != dimensions {
            return Ok(None);
        }
        let mut spans = Vec::new();
        let mut saw_data = false;
        loop {
            check_cancel(cancel)?;
            let start = file.stream_position()?;
            if start.checked_add(12).is_none_or(|end| end > size) {
                return Err(invalid().into());
            }
            let mut chunk = [0; 8];
            file.read_exact(&mut chunk)?;
            let length = u32::from_be_bytes(chunk[..4].try_into().unwrap());
            let kind: [u8; 4] = chunk[4..].try_into().unwrap();
            let end = start
                .checked_add(u64::from(length) + 12)
                .filter(|end| *end <= size)
                .ok_or_else(invalid)?;
            let camera = if &kind == b"eXIf" {
                true
            } else if &kind == b"iTXt" {
                let mut keyword = [0; 80];
                let count = (length as usize).min(keyword.len());
                file.read_exact(&mut keyword[..count])?;
                keyword[..count]
                    .iter()
                    .position(|byte| *byte == 0)
                    .is_some_and(|at| &keyword[..at] == XMP_KEYWORD)
            } else {
                false
            };
            if camera {
                // Large EXIF thumbnails / compressed XMP use fixed memory;
                // preparation does not silently discard a prior at 64 KiB.
                verify_crc(&mut file, start + 8, length, &kind, cancel)?;
                spans.push((start, u64::from(length) + 12));
            } else if &kind == b"IHDR" {
                return Err(invalid().into());
            } else if &kind == b"IDAT" {
                saw_data = true;
            } else if &kind == b"IEND" {
                if length != 0 || !saw_data || end != size {
                    return Err(invalid().into());
                }
                verify_crc(&mut file, start + 8, length, &kind, cancel)?;
                break;
            }
            // Never read the original pixel payload a second time.
            file.seek(SeekFrom::Start(end))?;
        }
        let value = Self {
            source: file,
            identity,
            dimensions,
            spans,
        };
        value.verify_identity()?;
        Ok((!value.spans.is_empty()).then_some(value))
    }

    fn verify_identity(&self) -> io::Result<()> {
        let current = self.source.metadata()?;
        if current.len() != self.identity.len()
            || current.modified()? != self.identity.modified()?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PNG changed while preserving camera metadata",
            ));
        }
        Ok(())
    }

    pub(super) fn encode(
        self,
        rgb: &DynamicImage,
        destination: &Path,
        cancel: &CancellationToken,
    ) -> Result<()> {
        check_cancel(cancel)?;
        let mut writer = MetadataWriter {
            output: BufWriter::new(File::create(destination)?),
            metadata: self,
            header: Vec::with_capacity(33),
            injected: false,
            cancel,
        };
        let encoded = rgb.write_with_encoder(PngEncoder::new(&mut writer));
        check_cancel(cancel)?;
        encoded.map_err(|error| match error {
            image::ImageError::IoError(error) => SplatError::Io(error),
            error => SplatError::Process(error.to_string()),
        })?;
        if !writer.injected {
            return Err(invalid().into());
        }
        writer.metadata.verify_identity()?;
        writer.flush()?;
        check_cancel(cancel)
    }
}

struct MetadataWriter<'a, W: Write> {
    output: W,
    metadata: PngCameraMetadata,
    header: Vec<u8>,
    injected: bool,
    cancel: &'a CancellationToken,
}

impl<W: Write> Write for MetadataWriter<'_, W> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("PNG metadata preservation cancelled"));
        }
        let length = bytes.len();
        if !self.injected {
            let count = (33 - self.header.len()).min(bytes.len());
            self.header.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
            if self.header.len() == 33 {
                if &self.header[..8] != SIGNATURE
                    || &self.header[12..16] != b"IHDR"
                    || self.header[16..20] != self.metadata.dimensions.0.to_be_bytes()
                    || self.header[20..24] != self.metadata.dimensions.1.to_be_bytes()
                {
                    return Err(invalid());
                }
                self.metadata.verify_identity()?;
                self.output.write_all(&self.header)?;
                let mut buffer = [0; 64 * 1024];
                for &(start, mut remaining) in &self.metadata.spans {
                    self.metadata.source.seek(SeekFrom::Start(start))?;
                    while remaining > 0 {
                        if self.cancel.is_cancelled() {
                            return Err(io::Error::other("PNG metadata preservation cancelled"));
                        }
                        let count = remaining.min(buffer.len() as u64) as usize;
                        self.metadata.source.read_exact(&mut buffer[..count])?;
                        self.output.write_all(&buffer[..count])?;
                        remaining -= count as u64;
                    }
                }
                self.metadata.verify_identity()?;
                self.injected = true;
            }
        }
        self.output.write_all(bytes)?;
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = (data.len() as u32).to_be_bytes().to_vec();
        bytes.extend(kind);
        bytes.extend(data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(data);
        bytes.extend(crc.finalize().to_be_bytes());
        bytes
    }

    fn fixture(
        root: &Path,
        sixteen: bool,
        compressed: bool,
        large: bool,
    ) -> (std::path::PathBuf, DynamicImage) {
        let decoded = if sixteen {
            DynamicImage::ImageRgba16(image::ImageBuffer::from_fn(2, 1, |x, _| {
                image::Rgba([50_000, 30_000, 10_000, if x == 0 { 12_345 } else { 65_535 }])
            }))
        } else {
            DynamicImage::ImageRgba8(image::RgbaImage::from_fn(2, 1, |x, _| {
                image::Rgba([200, 100, 50, if x == 0 { 64 } else { 255 }])
            }))
        };
        let source = root.join("original.png");
        decoded.save(&source).unwrap();
        let original = std::fs::read(&source).unwrap();
        // IFD0 points to an Exif IFD with a real 35mm-equivalent focal tag.
        let mut exif = b"II\x2a\0\x08\0\0\0".to_vec();
        exif.extend(1_u16.to_le_bytes());
        exif.extend(0x8769_u16.to_le_bytes());
        exif.extend(4_u16.to_le_bytes());
        exif.extend(1_u32.to_le_bytes());
        exif.extend(26_u32.to_le_bytes());
        exif.extend(0_u32.to_le_bytes());
        exif.extend(1_u16.to_le_bytes());
        exif.extend(0xa405_u16.to_le_bytes());
        exif.extend(3_u16.to_le_bytes());
        exif.extend(1_u32.to_le_bytes());
        exif.extend(100_u32.to_le_bytes());
        exif.extend(0_u32.to_le_bytes());
        if large {
            exif.resize(150_000, 0);
        }
        let xml = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:exif="http://ns.adobe.com/exif/1.0/" exif:FocalLengthIn35mmFilm="100"/></rdf:RDF></x:xmpmeta>"#;
        let mut xmp = b"XML:com.adobe.xmp\0".to_vec();
        xmp.extend([u8::from(compressed), 0, 0, 0]);
        if compressed {
            let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
            encoder.write_all(xml).unwrap();
            xmp.extend(encoder.finish().unwrap());
        } else {
            xmp.extend(xml);
        }
        let mut bytes = original[..33].to_vec();
        bytes.extend(chunk(b"eXIf", &exif));
        bytes.extend(chunk(b"iTXt", &xmp));
        bytes.extend(chunk(b"tEXt", b"Comment\0unrelated text"));
        bytes.extend(&original[33..]);
        std::fs::write(&source, bytes).unwrap();
        (source, decoded)
    }

    fn raw_camera_chunks(path: &Path) -> Vec<Vec<u8>> {
        let mut metadata = PngCameraMetadata::read(path, (2, 1), &CancellationToken::new())
            .unwrap()
            .unwrap();
        metadata
            .spans
            .iter()
            .map(|&(start, len)| {
                metadata.source.seek(SeekFrom::Start(start)).unwrap();
                let mut bytes = vec![0; len as usize];
                metadata.source.read_exact(&mut bytes).unwrap();
                bytes
            })
            .collect()
    }

    #[test]
    fn camera_chunks_survive_rgb8_rgb16_encoding_without_changing_pixels_or_masks() {
        for sixteen in [false, true] {
            for compressed in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let (source, decoded) = fixture(root.path(), sixteen, compressed, false);
                let source_before = std::fs::read(&source).unwrap();
                let output = root.path().join("rgb.png");
                // Exercise unlink protection against an existing source hard link.
                std::fs::hard_link(&source, &output).unwrap();
                let mask = root.path().join("mask.png");
                let cancel = CancellationToken::new();
                crate::video::image_sequence::write_ordinary_sfm_derivatives_from_source(
                    Some(&source),
                    &decoded,
                    &output,
                    &mask,
                    &cancel,
                )
                .unwrap();
                assert_eq!(raw_camera_chunks(&source), raw_camera_chunks(&output));
                assert_eq!(std::fs::read(&source).unwrap(), source_before);
                crate::video::image_sequence::validate_ordinary_sfm_derivatives(
                    &source, &output, &mask, &cancel,
                )
                .unwrap();
                assert_eq!(
                    image::open(&output).unwrap().color(),
                    if sixteen {
                        image::ColorType::Rgb16
                    } else {
                        image::ColorType::Rgb8
                    }
                );
            }
        }
    }

    #[test]
    fn large_exif_is_streamed_without_silently_dropping_camera_priors() {
        let root = tempfile::tempdir().unwrap();
        let (source, decoded) = fixture(root.path(), false, true, true);
        let output = root.path().join("rgb.png");
        crate::video::image_sequence::write_ordinary_sfm_derivatives_from_source(
            Some(&source),
            &decoded,
            &output,
            &root.path().join("mask.png"),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(raw_camera_chunks(&source), raw_camera_chunks(&output));
        assert!(raw_camera_chunks(&output)[0].len() > 64 * 1024);
    }

    #[test]
    fn camera_metadata_checks_crc_boundaries_pixel_map_and_cancellation() {
        for corrupt in ["crc", "truncated", "length"] {
            let root = tempfile::tempdir().unwrap();
            let (source, _) = fixture(root.path(), false, false, false);
            let mut bytes = std::fs::read(&source).unwrap();
            match corrupt {
                "crc" => bytes[41] ^= 1,
                "truncated" => {
                    bytes.pop();
                }
                "length" => bytes[33..37].copy_from_slice(&u32::MAX.to_be_bytes()),
                _ => unreachable!(),
            }
            std::fs::write(&source, bytes).unwrap();
            assert!(
                PngCameraMetadata::read(&source, (2, 1), &CancellationToken::new()).is_err(),
                "{corrupt}"
            );
        }
        let root = tempfile::tempdir().unwrap();
        let (source, _) = fixture(root.path(), false, false, false);
        assert!(
            PngCameraMetadata::read(&source, (1, 2), &CancellationToken::new())
                .unwrap()
                .is_none()
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            PngCameraMetadata::read(&source, (2, 1), &cancel),
            Err(SplatError::Cancelled)
        ));
    }
}
