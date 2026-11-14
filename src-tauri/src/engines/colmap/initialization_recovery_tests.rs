//! Synthetic process/control-flow regression; no bundled engine or dataset.
use super::*;

fn geometry_failure() -> SplatError {
    let output = crate::process::ProcessOutput {
        success: false,
        exit_code: Some(1),
        termination: crate::process::ProcessTermination {
            exit_code: Some(1),
            ..Default::default()
        },
        stdout: "Registering initial image pair #8 and #7\nDiscarding reconstruction due to bad initial pair".into(),
        stderr: "Failed to create any sparse model".into(),
    };
    SplatError::EngineProcess(Box::new(crate::failure::EngineProcessFailure::from_output(
        crate::pipeline::PipelineEngine::Colmap,
        "mapper",
        &output,
    )))
}

#[test]
fn prior_geometric_attempts_do_not_replace_io_or_cancellation() {
    let SplatError::EngineProcess(previous) = geometry_failure() else {
        unreachable!()
    };
    let error = mapper_outcome::with_previous_geometry_attempts(
        SplatError::Io(std::io::Error::from_raw_os_error(5)),
        vec![*previous.clone()],
    );
    assert_eq!(
        error.io_error().and_then(std::io::Error::raw_os_error),
        Some(5)
    );
    assert!(error
        .to_string()
        .contains("Previous Mapper geometry attempt"));
    assert!(!is_recoverable_mapper_geometry_failure(&error));
    assert!(matches!(
        mapper_outcome::with_previous_geometry_attempts(SplatError::Cancelled, vec![*previous],),
        SplatError::Cancelled
    ));
}

#[test]
fn fallback_history_does_not_decide_final_geometry_outcome() {
    let SplatError::EngineProcess(previous) = geometry_failure() else {
        unreachable!()
    };
    let mut current = *previous.clone();
    current.operation = "matches_importer".into();
    current.mapper_geometry_failure = false;
    let error = mapper_outcome::with_previous_geometry_attempts(
        SplatError::EngineProcess(Box::new(current)),
        vec![*previous],
    );
    assert!(!is_recoverable_mapper_geometry_failure(&error));
    let SplatError::EngineProcess(failure) = error else {
        unreachable!()
    };
    assert_eq!(failure.previous_attempts.len(), 1);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::reconstruction::{
        colmap_model::ColmapCamera,
        direct_input::{Model, ModelImage, Observation, Point},
    };
    use crate::video::panorama::PosedImage;
    use std::os::unix::fs::PermissionsExt;

    fn model(path: &Path, image_count: u32, k: f64) {
        Model {
            cameras: vec![ColmapCamera {
                id: 1,
                model_id: 2,
                model: "SIMPLE_RADIAL".into(),
                width: 2560,
                height: 3840,
                params: vec![27380., 1280., 1920., k],
            }],
            images: (1..=image_count)
                .map(|id| ModelImage {
                    pose: PosedImage {
                        id,
                        camera_id: 1,
                        name: format!("{id}.png"),
                        qvec: [1., 0., 0., 0.],
                        tvec: [id as f64, 0., 0.],
                    },
                    observations: vec![Observation {
                        x: 1280.,
                        y: 1920.,
                        point: Some(1),
                    }],
                })
                .collect(),
            points: vec![Point {
                id: 1,
                xyz: [0., 0., 1.],
                rgb: [128; 3],
                error: 0.,
                track: (1..=image_count).map(|id| (id, 0)).collect(),
            }],
        }
        .write(path)
        .unwrap();
    }

    fn database(path: &Path) {
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch("CREATE TABLE cameras(camera_id INTEGER PRIMARY KEY,model INTEGER,prior_focal_length INTEGER);
            CREATE TABLE images(image_id INTEGER PRIMARY KEY,name TEXT,camera_id INTEGER);
            CREATE TABLE two_view_geometries(pair_id INTEGER PRIMARY KEY,rows INTEGER,config INTEGER);
            INSERT INTO cameras VALUES(1,2,0),(2,1,1);
            INSERT INTO images VALUES(1,'a.png',1),(2,'b.png',1),(3,'c.png',2),(4,'d.png',1);
            INSERT INTO two_view_geometries VALUES(2147483649,100,3),(2147483650,20,3),(4294967297,20,3),(6442450945,20,3);").unwrap();
    }

    async fn run_case(mode: &str) -> (tempfile::TempDir, Result<()>) {
        let root = tempfile::tempdir().unwrap();
        let sparse = root.path().join("sparse");
        model(&sparse.join("0"), 1, 1.93);
        model(&root.path().join("candidate"), 2, 0.55);
        let db = root.path().join("database.db");
        database(&db);
        let images = root.path().join("images");
        std::fs::create_dir(&images).unwrap();
        let script = root.path().join("mapper");
        let dir = root.path().to_string_lossy().replace('\'', "'\\''");
        let body = format!(
            r##"#!/bin/sh
ulimit -c 0
out=''
seed=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output_path) shift; out="$1" ;;
    --Mapper.init_image_id1) shift; seed="$1" ;;
  esac
  shift
done
printf '%s\n' "$seed" >> '{dir}/seeds'
if [ "$seed" = 3 ]; then
  mkdir -p "$out/0"
  cp '{dir}/candidate/cameras.bin' '{dir}/candidate/images.bin' '{dir}/candidate/points3D.bin' "$out/0/"
  exit 0
fi
case '{mode}' in
  crash) kill -SEGV $$ ;;
  io) echo 'Failed to open database: Permission denied' >&2 ;;
esac
echo 'No good initial image pair found.'
echo 'Failed to create any sparse model' >&2
exit 1
"##
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let args = mapper_args(&db, &images, None, &sparse, true);
        let result = recover_mapper_initialization(
            &script,
            args.clone(),
            args,
            &sparse,
            root.path(),
            root.path().join("colmap.log"),
            &ProcessManager::new(),
            None,
            super::super::super::colmap_ba::ceres_selection("synthetic regression"),
        )
        .await;
        (root, result)
    }

    #[tokio::test]
    async fn later_geometric_seed_failures_keep_verified_candidate() {
        let (root, result) = run_case("geometry").await;
        result.unwrap();
        let best = initialization::assess_output(
            &root.path().join("sparse"),
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap()
        .unwrap();
        assert!(best.valid_intrinsics);
        assert_eq!(best.registered_images, 2);
        let seeds = std::fs::read_to_string(root.path().join("seeds")).unwrap();
        assert_eq!(seeds.lines().collect::<Vec<_>>(), ["3", "1", "2"]);
        assert!(root.path().join("sparse/0/cameras.bin").is_file());
    }

    #[tokio::test]
    async fn later_operational_failure_is_not_hidden_by_verified_candidate() {
        for mode in ["io", "crash"] {
            let (root, result) = run_case(mode).await;
            let error = result.unwrap_err();
            assert!(!is_recoverable_mapper_geometry_failure(&error));
            let seeds = std::fs::read_to_string(root.path().join("seeds")).unwrap();
            assert_eq!(seeds.lines().collect::<Vec<_>>(), ["3", "1"]);
            assert!(root.path().join("sparse/1/cameras.bin").is_file());
        }
    }
}
