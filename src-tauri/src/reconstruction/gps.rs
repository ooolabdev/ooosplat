//! Local-only georeference records. Never include Manifest in IPC/telemetry.
use crate::{
    error::{Result, SplatError},
    video::{
        gps::GpsFix,
        panorama::{read_poses, PosedImage},
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub const POLICY_VERSION: u32 = 1;
pub const FALLBACK_MESSAGE: &str = "GPS 辅助未启用，已切换为不使用 GPS 的生成方式。";
pub const DETECTED_MESSAGE: &str = "[GPS] detected";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub version: u32,
    pub status: String,
    pub fingerprint: String,
    pub transform: Option<Similarity>,
    pub output_identity: Option<String>,
    #[serde(default)]
    pub input_identity: Option<String>,
    #[serde(default)]
    pub manifest_identity: Option<String>,
    #[serde(default)]
    pub reference_valid: bool,
}
impl Checkpoint {
    pub fn pending() -> Self {
        Self {
            version: POLICY_VERSION,
            status: "pending".into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub name: String,
    pub fix: GpsFix,
    pub source: PathBuf,
    pub source_identity: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    pub records: Vec<Record>,
    pub origin: Option<GpsFix>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Similarity {
    pub rotation: [[f64; 3]; 3],
    pub scale: f64,
    pub translation: [f64; 3],
}
pub fn transpose(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}
pub fn mul(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    m.map(|r| r.iter().zip(v).map(|(a, b)| a * b).sum())
}
pub fn rotation([w, x, y, z]: [f64; 4]) -> [[f64; 3]; 3] {
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
pub fn center(p: &PosedImage) -> [f64; 3] {
    mul(transpose(rotation(p.qvec)), p.tvec).map(|v| -v)
}
impl Similarity {
    pub fn point(self, p: [f64; 3]) -> [f64; 3] {
        let p = mul(self.rotation, p);
        std::array::from_fn(|i| self.scale * p[i] + self.translation[i])
    }
    pub fn matrix(self) -> String {
        let mut rows = Vec::new();
        for i in 0..3 {
            rows.push(format!(
                "{} {} {} {}",
                self.scale * self.rotation[i][0],
                self.scale * self.rotation[i][1],
                self.scale * self.rotation[i][2],
                self.translation[i]
            ));
        }
        rows.push("0 0 0 1".into());
        rows.join("\n") + "\n"
    }
    pub fn read(path: &Path) -> Option<Self> {
        let values = std::fs::read_to_string(path)
            .ok()?
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        if values.iter().any(|v| !v.is_finite()) {
            return None;
        }
        // COLMAP 4.2 writes Sim3 as scale, qw,qx,qy,qz, tx,ty,tz.
        if values.len() == 8 {
            let q: [f64; 4] = values[1..5].try_into().ok()?;
            if values[0] <= 1e-12 || (q.iter().map(|v| v * v).sum::<f64>() - 1.).abs() > 1e-6 {
                return None;
            }
            return Some(Self {
                scale: values[0],
                rotation: rotation(q),
                translation: values[5..8].try_into().ok()?,
            });
        }
        if values.len() != 16 || values[12..] != [0., 0., 0., 1.] {
            return None;
        }
        let scale = values[..3].iter().map(|v| v * v).sum::<f64>().sqrt();
        if scale <= 1e-12 {
            return None;
        }
        let rotation: [[f64; 3]; 3] =
            std::array::from_fn(|i| std::array::from_fn(|j| values[4 * i + j] / scale));
        let rows = rotation;
        let det = rows[0][0] * (rows[1][1] * rows[2][2] - rows[1][2] * rows[2][1])
            - rows[0][1] * (rows[1][0] * rows[2][2] - rows[1][2] * rows[2][0])
            + rows[0][2] * (rows[1][0] * rows[2][1] - rows[1][1] * rows[2][0]);
        for i in 0..3 {
            for j in 0..3 {
                let dot: f64 = rows[i].iter().zip(rows[j]).map(|(a, b)| a * b).sum();
                if (dot - f64::from(i == j)).abs() > 1e-6 {
                    return None;
                }
            }
        }
        if (det - 1.).abs() > 1e-6 {
            return None;
        }
        Some(Self {
            rotation,
            scale,
            translation: [values[3], values[7], values[11]],
        })
    }
    pub fn then(self, next: Self) -> Self {
        Self {
            rotation: std::array::from_fn(|i| {
                std::array::from_fn(|j| {
                    (0..3)
                        .map(|k| next.rotation[i][k] * self.rotation[k][j])
                        .sum()
                })
            }),
            scale: self.scale * next.scale,
            translation: next.point(self.translation),
        }
    }
    /// PlayCanvas maps PLY by Rz(180). After that mapping: east=+X, up=+Y,
    /// north=-Z. This proper rotation does not change metric units.
    pub fn enu_to_ply() -> Self {
        Self {
            rotation: [[-1., 0., 0.], [0., 0., -1.], [0., -1., 0.]],
            scale: 1.,
            translation: [0.; 3],
        }
    }
    pub fn engine_transform(self) -> crate::project::GaussianTransform {
        // Conjugate by the preview's existing Rz(180) coordinate mapping.
        let d = [-1., -1., 1.];
        let m: [[f64; 3]; 3] =
            std::array::from_fn(|i| std::array::from_fn(|j| d[i] * self.rotation[i][j] * d[j]));
        let y = (-m[2][0]).clamp(-1., 1.).asin();
        let (x, z) = if y.cos().abs() > 1e-7 {
            (m[2][1].atan2(m[2][2]), m[1][0].atan2(m[0][0]))
        } else {
            ((-m[1][2]).atan2(m[1][1]), 0.)
        };
        crate::project::GaussianTransform {
            position: std::array::from_fn(|i| d[i] * self.translation[i]),
            rotation: [x.to_degrees(), y.to_degrees(), z.to_degrees()],
            scale: self.scale,
        }
    }
}

fn error(e: impl std::fmt::Display) -> SplatError {
    SplatError::Process(format!("GPS local data: {e}"))
}
fn database_error(e: rusqlite::Error) -> SplatError {
    SplatError::Io(std::io::Error::other(format!("GPS database: {e}")))
}
pub fn file_identity(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let meta = std::fs::metadata(path)?;
    let mut hash = Sha256::new();
    hash.update(path.as_os_str().as_encoded_bytes());
    hash.update(meta.len().to_le_bytes());
    hash.update(
        meta.modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(error)?
            .as_nanos()
            .to_le_bytes(),
    );
    Ok(format!("{:x}", hash.finalize()))
}
pub fn collect(
    inputs: &[(String, PathBuf)],
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for (name, path) in inputs {
        if cancel.is_cancelled() {
            return Err(SplatError::Cancelled);
        }
        if let Some(fix) = crate::video::gps::read(path) {
            records.push(Record {
                name: name.clone(),
                fix,
                source: path.clone(),
                source_identity: file_identity(path)?,
            });
        }
    }
    Ok(records)
}
pub fn fingerprint(records: &[Record]) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(POLICY_VERSION, records))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
pub fn manifest_identity(manifest: &Manifest) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(manifest)?)
    ))
}
pub fn usable(records: &[Record], origin: &GpsFix) -> bool {
    let records: Vec<_> = records
        .iter()
        .filter(|r| r.fix.altitude.is_some())
        .collect();
    if records.len() < 5 {
        return false;
    }
    let mut accuracy: Vec<_> = records.iter().map(|r| r.fix.horizontal_std).collect();
    accuracy.sort_by(f64::total_cmp);
    let points: Vec<_> = records.iter().map(|r| r.fix.enu(origin)).collect();
    let distinct: std::collections::BTreeSet<_> = points
        .iter()
        .map(|p| ((p[0] * 100.).round() as i64, (p[1] * 100.).round() as i64))
        .collect();
    if distinct.len() < 5 {
        return false;
    }
    let n = points.len() as f64;
    let mean: [f64; 2] = std::array::from_fn(|i| points.iter().map(|p| p[i]).sum::<f64>() / n);
    let xx = points.iter().map(|p| (p[0] - mean[0]).powi(2)).sum::<f64>();
    let yy = points.iter().map(|p| (p[1] - mean[1]).powi(2)).sum::<f64>();
    let xy = points
        .iter()
        .map(|p| (p[0] - mean[0]) * (p[1] - mean[1]))
        .sum::<f64>();
    let large = ((xx + yy) + ((xx - yy).powi(2) + 4. * xy * xy).sqrt()) / 2.;
    let small = xx + yy - large;
    let span = (0..2)
        .map(|i| {
            points
                .iter()
                .map(|p| p[i])
                .fold(f64::NEG_INFINITY, f64::max)
                - points.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min)
        })
        .fold(0., f64::max);
    large > 0. && small / large >= 0.01 && span >= 5f64.max(3. * accuracy[accuracy.len() / 2])
}
pub fn horizontal_usable(records: &[Record], origin: &GpsFix) -> bool {
    if records.len() < 2 {
        return false;
    }
    let mut accuracy: Vec<_> = records.iter().map(|r| r.fix.horizontal_std).collect();
    accuracy.sort_by(f64::total_cmp);
    let points: Vec<_> = records.iter().map(|r| r.fix.enu(origin)).collect();
    let span = (0..2)
        .map(|i| {
            points
                .iter()
                .map(|p| p[i])
                .fold(f64::NEG_INFINITY, f64::max)
                - points.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min)
        })
        .fold(0., f64::max);
    span >= 5f64.max(3. * accuracy[accuracy.len() / 2])
}
pub fn install_priors(
    database: &Path,
    records: &[Record],
    origin: &GpsFix,
    horizontal_only: bool,
) -> Result<usize> {
    let error = database_error;
    let mut db = rusqlite::Connection::open(database).map_err(database_error)?;
    // Only the independent GPS attempt database is passed here. Validate its
    // schema before changing it; never reinterpret legacy prior columns.
    let columns: Vec<String> = db
        .prepare("PRAGMA table_info(pose_priors)")
        .map_err(error)?
        .query_map([], |r| r.get(1))
        .map_err(error)?
        .collect::<std::result::Result<_, _>>()
        .map_err(error)?;
    for name in [
        "corr_data_id",
        "corr_sensor_id",
        "corr_sensor_type",
        "position",
        "position_covariance",
        "coordinate_system",
    ] {
        if !columns.iter().any(|v| v == name) {
            return Err(SplatError::UnsupportedEngine(
                "GPS position-prior schema".into(),
            ));
        }
    }
    let images: BTreeMap<String, (u32, u32)> = db
        .prepare("SELECT name,image_id,camera_id FROM images")
        .map_err(error)?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))
        .map_err(error)?
        .collect::<std::result::Result<_, _>>()
        .map_err(error)?;
    let tx = db.transaction().map_err(error)?;
    tx.execute("DELETE FROM pose_priors", []).map_err(error)?;
    let mut count = 0;
    for record in records {
        if record.fix.altitude.is_none() && !horizontal_only {
            continue;
        }
        let Some((id, camera)) = images.get(&record.name) else {
            continue;
        };
        let p = record.fix.enu(origin);
        let h = record.fix.horizontal_std.powi(2);
        let v = record.fix.vertical_std.powi(2);
        let position: Vec<_> = p.into_iter().flat_map(f64::to_le_bytes).collect();
        let covariance: Vec<_> = [h, 0., 0., 0., h, 0., 0., 0., v]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect();
        tx.execute("INSERT INTO pose_priors(corr_data_id,corr_sensor_id,corr_sensor_type,position,position_covariance,coordinate_system) VALUES(?1,?2,0,?3,?4,1)",rusqlite::params![id,camera,position,covariance]).map_err(error)?;
        count += 1;
    }
    tx.commit().map_err(error)?;
    Ok(count)
}
pub fn verify_alignment(model: &Path, records: &[Record], origin: &GpsFix) -> Result<bool> {
    let poses = read_poses(model)?;
    let lookup: BTreeMap<_, _> = poses.iter().map(|p| (p.name.as_str(), center(p))).collect();
    let mut inliers = Vec::new();
    let mut total = 0;
    for r in records.iter().filter(|r| r.fix.altitude.is_some()) {
        total += 1;
        let Some(c) = lookup.get(r.name.as_str()) else {
            continue;
        };
        let p = r.fix.enu(origin);
        let residual = ((c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2))
            / r.fix.horizontal_std.powi(2)
            + (c[2] - p[2]).powi(2) / r.fix.vertical_std.powi(2);
        if residual.is_finite() && residual <= 9.0 {
            inliers.push(r.clone());
        }
    }
    Ok(inliers.len() as f64 >= 0.8 * total as f64 && usable(&inliers, origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_manifest_round_trip_preserves_coordinates_and_cache_identities() {
        fn fix_bits(fix: &GpsFix) -> [Option<u64>; 5] {
            [
                Some(fix.latitude.to_bits()),
                Some(fix.longitude.to_bits()),
                fix.altitude.map(f64::to_bits),
                Some(fix.horizontal_std.to_bits()),
                Some(fix.vertical_std.to_bits()),
            ]
        }

        // Synthetic values from serde_json's public f64 round-trip regressions,
        // not a user's GPS. Its default fast parser changes these by one ULP;
        // that is enough to invalidate a previously saved GPS cache identity.
        let known_regression = GpsFix {
            latitude: 51.248178375505404,
            longitude: -93.31137037688033,
            altitude: Some(52.314008204106244),
            horizontal_std: 97.45365320034685,
            vertical_std: 36.573994842753436,
        };
        let mut records = vec![Record {
            name: "synthetic-000.png".into(),
            fix: known_regression.clone(),
            source: PathBuf::from("synthetic/000.png"),
            source_identity: "synthetic-source-000".into(),
        }];
        // Reproducible finite values across legal geographic ranges, including
        // missing altitude. No image decoding, external files or GPS is needed.
        let mut random = 0x5e12_d9b4_8a73_c601_u64;
        let mut unit = || {
            random = random
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (random >> 11) as f64 / (1_u64 << 53) as f64
        };
        for index in 1..=128 {
            records.push(Record {
                name: format!("synthetic-{index:03}.png"),
                fix: GpsFix {
                    latitude: unit() * 178. - 89.,
                    longitude: unit() * 358. - 179.,
                    altitude: (index % 7 != 0).then(|| unit() * 10_000. - 1_000.),
                    horizontal_std: unit() * 50. + 0.1,
                    vertical_std: unit() * 100. + 0.1,
                },
                source: PathBuf::from(format!("synthetic/{index:03}.png")),
                source_identity: format!("synthetic-source-{index:03}"),
            });
        }
        let mut manifest = Manifest {
            version: POLICY_VERSION,
            records,
            origin: Some(known_regression),
        };
        let expected_bits: Vec<_> = manifest
            .records
            .iter()
            .map(|record| fix_bits(&record.fix))
            .collect();
        let expected_origin_bits = fix_bits(manifest.origin.as_ref().unwrap());
        let expected_fingerprint = fingerprint(&manifest.records).unwrap();
        let expected_manifest_identity = manifest_identity(&manifest).unwrap();
        let mut checkpoint = Checkpoint {
            version: POLICY_VERSION,
            status: "aligned".into(),
            fingerprint: expected_fingerprint.clone(),
            manifest_identity: Some(expected_manifest_identity.clone()),
            input_identity: Some("synthetic-input".into()),
            output_identity: Some("synthetic-output".into()),
            reference_valid: true,
            transform: Some(Similarity {
                rotation: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                scale: 1.2345678901234567,
                translation: [51.248178375505404, -93.31137037688033, 52.314008204106244],
            }),
        };
        let expected_manifest_json = serde_json::to_vec_pretty(&manifest).unwrap();
        let expected_checkpoint_json = serde_json::to_vec_pretty(&checkpoint).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let manifest_path = directory.path().join("gps-manifest.json");
        let checkpoint_path = directory.path().join("checkpoint.json");

        for round in 0..4 {
            std::fs::write(
                &manifest_path,
                serde_json::to_vec_pretty(&manifest).unwrap(),
            )
            .unwrap();
            std::fs::write(
                &checkpoint_path,
                serde_json::to_vec_pretty(&checkpoint).unwrap(),
            )
            .unwrap();
            manifest = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
            checkpoint = serde_json::from_slice(&std::fs::read(&checkpoint_path).unwrap()).unwrap();

            let actual_bits: Vec<_> = manifest
                .records
                .iter()
                .map(|record| fix_bits(&record.fix))
                .collect();
            assert_eq!(
                actual_bits, expected_bits,
                "GPS f64 bits changed in round {round}"
            );
            assert_eq!(
                fix_bits(manifest.origin.as_ref().unwrap()),
                expected_origin_bits,
                "origin f64 bits changed in round {round}"
            );
            assert_eq!(
                fingerprint(&manifest.records).unwrap(),
                expected_fingerprint
            );
            assert_eq!(
                manifest_identity(&manifest).unwrap(),
                expected_manifest_identity
            );
            assert_eq!(checkpoint.fingerprint, expected_fingerprint);
            assert_eq!(
                checkpoint.manifest_identity.as_ref(),
                Some(&expected_manifest_identity)
            );
            assert_eq!(
                serde_json::to_vec_pretty(&manifest).unwrap(),
                expected_manifest_json,
                "manifest JSON changed in round {round}"
            );
            assert_eq!(
                serde_json::to_vec_pretty(&checkpoint).unwrap(),
                expected_checkpoint_json,
                "checkpoint JSON changed in round {round}"
            );
        }
    }

    #[test]
    fn local_axes_match_existing_preview_mapping() {
        let t = Similarity::enu_to_ply();
        assert_eq!(t.point([1., 0., 0.]), [-1., 0., 0.]);
        assert_eq!(t.point([0., 1., 0.]), [0., 0., -1.]);
        assert_eq!(t.point([0., 0., 1.]), [0., -1., 0.]);
    }
    #[test]
    fn rejects_unobservable_gps() {
        let origin = GpsFix {
            latitude: 51.,
            longitude: 0.,
            altitude: Some(20.),
            horizontal_std: 10.,
            vertical_std: 20.,
        };
        let mut rows: Vec<_> = (0..6)
            .map(|i| Record {
                name: i.to_string(),
                fix: origin.clone(),
                source: PathBuf::new(),
                source_identity: String::new(),
            })
            .collect();
        assert!(!usable(&rows, &origin));
        for (i, r) in rows.iter_mut().enumerate() {
            r.fix.latitude += i as f64 * 0.001;
        }
        assert!(!usable(&rows, &origin));
        rows[0].fix.longitude += 0.001;
        assert!(usable(&rows, &origin));
        rows[1].fix.altitude = None;
        rows[2].fix.altitude = None;
        assert!(!usable(&rows, &origin));
    }
    #[test]
    fn similarity_matrix_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("matrix");
        let t = Similarity {
            rotation: rotation([1., 0., 0., 0.]),
            scale: 2.,
            translation: [1., 2., 3.],
        };
        std::fs::write(&path, t.matrix()).unwrap();
        assert_eq!(Similarity::read(&path), Some(t));
        std::fs::write(&path, "2 1 0 0 0 1 2 3").unwrap();
        assert_eq!(Similarity::read(&path), Some(t));
        std::fs::write(&path, "NaN").unwrap();
        assert_eq!(Similarity::read(&path), None);
    }

    #[test]
    fn gaussian_initialization_uses_the_same_similarity_once_without_losing_sh() {
        use crate::reconstruction::splat_transform::{
            export_transformed_ply_to, validate_training_ply,
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("original.ply");
        let output = dir.path().join("derived.ply");
        let mut names: Vec<String> = [
            "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
            "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        names.extend((0..45).map(|i| format!("f_rest_{i}")));
        let mut bytes = b"ply\nformat binary_little_endian 1.0\nelement vertex 1\n".to_vec();
        for name in names {
            bytes.extend(format!("property float {name}\n").bytes());
        }
        bytes.extend(b"end_header\n");
        let header = bytes.len();
        let mut data = vec![
            1_f32, 2., 3., 0.1, 0.2, 0.3, 0.7, -2., -3., -4., 1., 0., 0., 0.,
        ];
        data.extend((0..45).map(|i| i as f32 / 100.));
        for v in &data {
            bytes.extend(v.to_le_bytes());
        }
        std::fs::write(&source, &bytes).unwrap();
        let transform = Similarity {
            scale: 2.,
            translation: [4., 5., 6.],
            ..Similarity::enu_to_ply()
        };
        export_transformed_ply_to(
            &source,
            &output,
            transform.engine_transform(),
            Default::default(),
            |_, _| {},
            || Ok(()),
        )
        .unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        let (info, sh, _) = validate_training_ply(&output, &token).unwrap();
        assert_eq!((info.splat_count, sh), (1, 3));
        let result = std::fs::read(output).unwrap();
        let values: Vec<_> = result[header..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b) as f64)
            .collect();
        let point = transform.point([1., 2., 3.]);
        for i in 0..3 {
            assert!((values[i] - point[i]).abs() < 1e-5);
            assert!((values[7 + i] - data[7 + i] as f64 - 2f64.ln()).abs() < 1e-5);
        }
        assert_eq!(
            &values[3..7],
            &data[3..7].iter().map(|v| *v as f64).collect::<Vec<_>>()
        );
        let matrix = rotation(values[10..14].try_into().unwrap());
        for (a, b) in matrix
            .into_iter()
            .flatten()
            .zip(transform.rotation.into_iter().flatten())
        {
            assert!((a - b).abs() < 1e-5);
        }
        let input_norm = data[14..].iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
        let output_norm = values[14..].iter().map(|v| v * v).sum::<f64>();
        assert!((input_norm - output_norm).abs() < 1e-5);
        assert_eq!(std::fs::read(source).unwrap(), bytes);
    }

    #[test]
    fn priors_use_real_image_ids_and_never_invent_altitude_constraints() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gps.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE images(image_id INTEGER,camera_id INTEGER,name TEXT); INSERT INTO images VALUES(42,7,'photo.png'),(99,8,'horizontal.png'); CREATE TABLE pose_priors(corr_data_id INTEGER,corr_sensor_id INTEGER,corr_sensor_type INTEGER,position BLOB,position_covariance BLOB,coordinate_system INTEGER);").unwrap();
        let origin = GpsFix {
            latitude: 0.,
            longitude: 0.,
            altitude: Some(0.),
            horizontal_std: 10.,
            vertical_std: 20.,
        };
        let a = Record {
            name: "photo.png".into(),
            fix: origin.clone(),
            source: PathBuf::new(),
            source_identity: String::new(),
        };
        let mut b = a.clone();
        b.name = "horizontal.png".into();
        b.fix.altitude = None;
        let records = vec![a, b];
        assert_eq!(install_priors(&path, &records, &origin, true).unwrap(), 2);
        assert_eq!(install_priors(&path, &records, &origin, false).unwrap(), 1);
        let row:(u32,u32,u32,Vec<u8>)=db.query_row("SELECT corr_data_id,corr_sensor_id,coordinate_system,position_covariance FROM pose_priors",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        assert_eq!((row.0, row.1, row.2), (42, 7, 1));
        assert_eq!(f64::from_le_bytes(row.3[..8].try_into().unwrap()), 100.);
        assert_eq!(f64::from_le_bytes(row.3[64..72].try_into().unwrap()), 400.);
    }
}
