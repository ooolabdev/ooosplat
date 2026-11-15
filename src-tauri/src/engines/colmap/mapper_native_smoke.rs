//! Explicit native smoke with synthetic geometry. This does not reproduce the
//! unavailable 67-photo report or establish photographic reconstruction quality.
use super::*;
use crate::{
    reconstruction::{
        colmap_model::ColmapCamera,
        direct_input::{Model, ModelImage, Observation, Point},
    },
    video::panorama::PosedImage,
};
use sha2::{Digest, Sha256};
use std::{future::Future, time::Duration};

async fn bounded<T>(
    manager: &ProcessManager,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => result,
        _ = tokio::time::sleep(Duration::from_secs(120)) => {
            // Do not just drop a timed-out child. Let ProcessManager terminate
            // and reap its process group / Windows Job Object first.
            manager.cancel();
            let _ = future.await;
            Err(SplatError::Process("Native Mapper smoke exceeded its 120 second operation limit".into()))
        }
    }
}

fn digest(path: &Path) -> String {
    let data = std::fs::read(path).unwrap();
    format!("{:x}", Sha256::digest(data))
}

fn doubles(values: impl IntoIterator<Item = f64>) -> Vec<u8> {
    values.into_iter().flat_map(f64::to_le_bytes).collect()
}

fn prepare_geometry(database: &Path, seed: &Path) -> usize {
    let db = rusqlite::Connection::open(database).unwrap();
    let images = db
        .prepare("SELECT image_id,name,camera_id FROM images ORDER BY name")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(images.len(), 6);
    let camera = images[0].2;
    assert!(images.iter().all(|image| image.2 == camera));
    db.execute("UPDATE cameras SET model=2,width=128,height=128,params=?1,prior_focal_length=1 WHERE camera_id=?2",
        rusqlite::params![doubles([64.,64.,64.,0.]), camera]).unwrap();
    let descriptor_type: i32 = db
        .query_row("SELECT type FROM descriptors LIMIT 1", [], |row| row.get(0))
        .unwrap();
    db.execute_batch("DELETE FROM keypoints; DELETE FROM descriptors; DELETE FROM pose_priors; DELETE FROM matches; DELETE FROM two_view_geometries;").unwrap();
    let centers = [
        [-2., -1., 0.1],
        [-1., 1., -0.2],
        [1., 2., 0.3],
        [2., 0., -0.1],
        [1., -2., 0.2],
        [-1., -2., -0.3],
    ];
    let points = (0..180)
        .map(|index| {
            [
                (index % 15) as f64 * 0.24 - 1.68,
                (index / 15) as f64 * 0.22 - 1.1,
                5. + (index % 7) as f64 * 0.3,
            ]
        })
        .collect::<Vec<_>>();
    let project = |point: &[f64; 3], center: &[f64; 3]| {
        [
            64. * (point[0] - center[0]) / (point[2] - center[2]) + 64.,
            64. * (point[1] - center[1]) / (point[2] - center[2]) + 64.,
        ]
    };
    for (index, image) in images.iter().enumerate() {
        let keypoints = points
            .iter()
            .flat_map(|point| project(point, &centers[index]))
            .flat_map(|value| (value as f32).to_le_bytes())
            .collect::<Vec<_>>();
        db.execute(
            "INSERT INTO keypoints(image_id,rows,cols,data) VALUES(?1,?2,2,?3)",
            rusqlite::params![image.0, points.len(), keypoints],
        )
        .unwrap();
        db.execute(
            "INSERT INTO descriptors(image_id,type,rows,cols,data) VALUES(?1,?2,?3,128,?4)",
            rusqlite::params![
                image.0,
                descriptor_type,
                points.len(),
                vec![0_u8; points.len() * 128]
            ],
        )
        .unwrap();
    }
    for first in 0..images.len() {
        for second in first + 1..images.len() {
            let matches = (0..points.len() as u32)
                .flat_map(|index| [index, index])
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>();
            let a = images[first].0.min(images[second].0) as u64;
            let b = images[first].0.max(images[second].0) as u64;
            db.execute("INSERT INTO two_view_geometries(pair_id,rows,cols,data,config) VALUES(?1,?2,2,?3,2)", rusqlite::params![a*2_147_483_647+b,points.len(),matches]).unwrap();
        }
    }
    let model = Model {
        cameras: vec![ColmapCamera {
            id: camera,
            model_id: 2,
            model: "SIMPLE_RADIAL".into(),
            width: 128,
            height: 128,
            params: vec![64., 64., 64., 0.],
        }],
        images: images
            .iter()
            .enumerate()
            .map(|(index, image)| ModelImage {
                pose: PosedImage {
                    id: image.0,
                    camera_id: camera,
                    name: image.1.clone(),
                    qvec: [1., 0., 0., 0.],
                    tvec: centers[index].map(|value| -value),
                },
                observations: points
                    .iter()
                    .enumerate()
                    .map(|(point_index, point)| {
                        let xy = project(point, &centers[index]);
                        Observation {
                            x: xy[0],
                            y: xy[1],
                            point: Some(point_index as u64 + 1),
                        }
                    })
                    .collect(),
            })
            .collect(),
        points: points
            .iter()
            .enumerate()
            .map(|(index, xyz)| Point {
                id: index as u64 + 1,
                xyz: *xyz,
                rgb: [128; 3],
                error: 0.,
                track: images.iter().map(|image| (image.0, index as u32)).collect(),
            })
            .collect(),
    };
    model.validate().unwrap();
    model.write(seed).unwrap();
    // Only this isolated test DB is changed. Ensure the copied rejection case
    // is a complete SQLite file rather than copying pending sidecar writes.
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
        .unwrap();
    images.len()
}

#[tokio::test]
#[ignore = "Explicit pinned native COLMAP smoke; synthetic geometry, CPU only, isolated new directory"]
async fn native_mapper_geometry_rejection_keeps_verified_baseline() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let root = PathBuf::from(
        std::env::var_os("OOOSPLAT_NATIVE_MAPPER_SMOKE_ROOT")
            .expect("Set a fresh workspace .tmp output path"),
    );
    let executable = PathBuf::from(
        std::env::var_os("OOOSPLAT_NATIVE_MAPPER_EXECUTABLE")
            .expect("Set the pinned COLMAP executable"),
    );
    assert!(root.is_absolute() && !root.exists());
    assert!(std::fs::canonicalize(root.parent().unwrap())
        .unwrap()
        .starts_with(std::fs::canonicalize(workspace.join(".tmp")).unwrap()));
    std::fs::create_dir(&root).unwrap();
    println!("Native smoke evidence: {}", root.display());
    let manager = ProcessManager::new();
    let help = bounded(
        &manager,
        manager.run(ProcessSpec {
            executable: executable.clone(),
            args: vec!["-h".into()],
            working_directory: Some(root.clone()),
            log_path: Some(root.join("engine-help.log")),
            observer: None,
        }),
    )
    .await
    .unwrap();
    assert!(help.success && is_locked_build(&format!("{}\n{}", help.stdout, help.stderr)));
    let images = root.join("images");
    std::fs::create_dir(&images).unwrap();
    let fixture = image::open(workspace.join("src-tauri/icons/128x128.png"))
        .unwrap()
        .into_rgb8();
    assert_eq!(fixture.dimensions(), (128, 128));
    for index in 1..=6 {
        fixture
            .save(images.join(format!("frame_{index}.jpg")))
            .unwrap();
    }
    let original_image_hashes = (1..=6)
        .map(|index| digest(&images.join(format!("frame_{index}.jpg"))))
        .collect::<Vec<_>>();
    let database = root.join("database.db");
    bounded(
        &manager,
        extract_features(
            &executable,
            &database,
            &images,
            None,
            Some(128),
            Some(256),
            root.join("feature-extraction.log"),
            &manager,
            None,
            None,
            Some("64,64,64,0"),
        ),
    )
    .await
    .unwrap();
    let seed = root.join("seed");
    let count = prepare_geometry(&database, &seed) as u64;
    let original_seed_hashes =
        ["cameras.bin", "images.bin", "points3D.bin"].map(|name| digest(&seed.join(name)));
    let output = root.join("accepted");
    bounded(
        &manager,
        map_from_existing(
            &executable,
            &database,
            &images,
            &seed,
            &output,
            false,
            false,
            root.join("accepted-mapper.log"),
            &manager,
            None,
            None,
        ),
    )
    .await
    .unwrap();
    let (baseline, report) = select_reconstruction_model(&output, count)
        .unwrap()
        .unwrap();
    assert_eq!(report.registered_images, count);
    let baseline_hashes =
        ["cameras.bin", "images.bin", "points3D.bin"].map(|name| digest(&baseline.join(name)));
    let rejected_database = root.join("disconnected.db");
    std::fs::copy(&database, &rejected_database).unwrap();
    let rejected = rusqlite::Connection::open(&rejected_database).unwrap();
    rejected.execute_batch("DELETE FROM matches; DELETE FROM two_view_geometries; PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;").unwrap();
    drop(rejected);
    let rejected_output = root.join("rejected");
    let failure = bounded(
        &manager,
        map(
            &executable,
            &rejected_database,
            &images,
            &rejected_output,
            false,
            false,
            root.join("rejected-mapper.log"),
            &manager,
            None,
            None,
        ),
    )
    .await
    .unwrap_err();
    assert!(
        is_recoverable_mapper_geometry_failure(&failure),
        "{failure}"
    );
    assert_eq!(
        original_seed_hashes,
        ["cameras.bin", "images.bin", "points3D.bin"].map(|name| digest(&seed.join(name)))
    );
    assert_eq!(
        baseline_hashes,
        ["cameras.bin", "images.bin", "points3D.bin"].map(|name| digest(&baseline.join(name)))
    );
    assert_eq!(
        original_image_hashes,
        (1..=6)
            .map(|index| digest(&images.join(format!("frame_{index}.jpg"))))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        select_reconstruction_model(&output, count)
            .unwrap()
            .unwrap()
            .1,
        report
    );
    assert!(!manager.has_active_processes());
    let receipt = serde_json::json!({
        "scope":"synthetic geometry; not the historical 67-photo dataset",
        "registeredImages":report.registered_images,"points3d":report.points_3d,
        "geometryRejectionRecognized":true,"baselineUnchanged":true,"seedUnchanged":true,
        "imagesUnchanged":true,"brushExecutions":0,"gpuRequested":false,
        "failure":failure.to_string(),
    });
    std::fs::write(
        root.join("result.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!("{receipt}");
}
