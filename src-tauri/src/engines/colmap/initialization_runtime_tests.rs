//! Explicit, opt-in native regression. The manifest names read-only source
//! assets and a fresh output directory; normal CI never starts a real engine.
use super::*;
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeCase {
    executable: PathBuf,
    executable_sha256: String,
    database: PathBuf,
    images: PathBuf,
    bad_model: PathBuf,
    output: PathBuf,
    #[serde(default)]
    masks: Option<PathBuf>,
}

fn hash(path: &Path) -> Vec<u8> {
    use std::io::Read;
    let mut reader = std::fs::File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = reader.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    hash.finalize().to_vec()
}

#[tokio::test]
#[ignore = "Requires explicit read-only native regression manifest and bundled COLMAP"]
async fn native_invalid_initialization_recovers_without_reextracting_or_changing_masks() {
    let manifest = std::env::var_os("OOOSPLAT_NATIVE_INITIALIZATION_CASE")
        .expect("Set the test-only native case manifest path");
    let case: NativeCase = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    assert_eq!(
        hash(&case.executable)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        case.executable_sha256
    );
    assert!(case.output.is_absolute() && !case.output.exists());
    let source_database_hash = hash(&case.database);
    let mut source_models = Vec::new();
    std::fs::create_dir(&case.output).unwrap();
    let sparse = case.output.join("sparse");
    let initial = sparse.join("0");
    std::fs::create_dir_all(&initial).unwrap();
    for entry in std::fs::read_dir(&case.bad_model).unwrap() {
        let entry = entry.unwrap();
        assert!(!entry.file_type().unwrap().is_symlink());
        if entry.file_type().unwrap().is_file() {
            source_models.push((entry.path(), hash(&entry.path())));
            std::fs::copy(entry.path(), initial.join(entry.file_name())).unwrap();
        }
    }
    let database = case.output.join("database.db");
    std::fs::copy(&case.database, &database).unwrap();
    let started = Arc::new(AtomicUsize::new(0));
    let seen = started.clone();
    let observer: ProcessObserver = Arc::new(move |event| {
        if matches!(event, crate::process::ProcessUpdate::Started { .. }) {
            seen.fetch_add(1, Ordering::Relaxed);
        }
    });
    let manager = ProcessManager::new();
    let args = mapper_args(&database, &case.images, None, &sparse, true);
    let request = initialization::eligible_request(&args, &case.output)
        .unwrap()
        .unwrap();
    let seeds = initialization::select_seed_images(&request, &manager.child_token()).unwrap();
    let before = initialization::assess_output(&sparse, &manager.child_token())
        .unwrap()
        .unwrap();
    assert!(initialization::should_recover(&before, seeds.input_images));
    let begin = std::time::Instant::now();
    recover_mapper_initialization(
        &case.executable,
        args.clone(),
        args,
        &sparse,
        &case.output,
        case.output.join("colmap.log"),
        &manager,
        Some(observer),
        super::super::colmap_ba::ceres_selection("Explicit native regression"),
    )
    .await
    .unwrap();
    let after = initialization::assess_output(&sparse, &manager.child_token())
        .unwrap()
        .unwrap();
    let receipt = serde_json::json!({
        "beforeRegistered": before.registered_images,
        "afterRegistered": after.registered_images,
        "inputImages": seeds.input_images,
        "seedImages": seeds.seeds,
        "validIntrinsics": after.valid_intrinsics,
        "elapsedSeconds": begin.elapsed().as_secs_f64(),
        "engineExecutions": started.load(Ordering::Relaxed),
        "model": after.model,
        "sourceDatabaseUnchanged": source_database_hash == hash(&case.database),
        "sourceModelUnchanged": source_models.iter().all(|(path, digest)| &hash(path) == digest),
        "originalFailedModelPreserved": initial.join("cameras.bin").is_file(),
        "featureExtractionExecutions": 0,
        "matchingExecutions": 0,
        "brushExecutions": 0,
    });
    std::fs::write(
        case.output.join("result.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!("{receipt}");
    assert!(after.valid_intrinsics);
    assert_eq!(after.registered_images, seeds.input_images);
    assert!((1..=initialization::MAX_RECOVERY_SEEDS).contains(&started.load(Ordering::Relaxed)));
    assert_eq!(hash(&case.database), source_database_hash);
    assert!(source_models
        .iter()
        .all(|(path, digest)| &hash(path) == digest));
}

#[tokio::test]
#[ignore = "Requires explicit prepared RGB/grayscale-Mask native regression manifest"]
async fn native_071_matching_and_mapper_keep_premultiplied_rgb_and_grayscale_masks() {
    let manifest = std::env::var_os("OOOSPLAT_NATIVE_INITIALIZATION_CASE")
        .expect("Set the test-only native case manifest path");
    let case: NativeCase = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    let engine_digest = hash(&case.executable)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(engine_digest, case.executable_sha256);
    assert!(!case.output.exists());
    let mut images = std::fs::read_dir(&case.images)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    images.sort();
    let sample = &images[0];
    assert!(
        !crate::video::image_projection::inspect(sample)
            .unwrap()
            .alpha
    );
    let mask = case.masks.as_ref().unwrap().join(format!(
        "{}.png",
        sample.file_name().unwrap().to_str().unwrap()
    ));
    let pixels = image::open(&mask).unwrap().into_luma8();
    assert!(pixels
        .pixels()
        .any(|pixel| pixel.0[0] != 0 && pixel.0[0] != 255));
    let original_hashes = [hash(&case.database), hash(sample), hash(&mask)];
    std::fs::create_dir(&case.output).unwrap();
    let database = case.output.join("database.db");
    std::fs::copy(&case.database, &database).unwrap();
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute_batch("DELETE FROM matches; DELETE FROM two_view_geometries;")
        .unwrap();
    let features: u64 = db
        .query_row("SELECT SUM(rows) FROM keypoints", [], |row| row.get(0))
        .unwrap();
    drop(db);
    let manager = ProcessManager::new();
    let log = case.output.join("colmap.log");
    let begin = std::time::Instant::now();
    match_exhaustive(
        &case.executable,
        &database,
        log.clone(),
        &manager,
        None,
        Some(0),
    )
    .await
    .unwrap();
    let sparse = case.output.join("sparse");
    map(
        &case.executable,
        &database,
        &case.images,
        &sparse,
        true,
        false,
        log,
        &manager,
        None,
        None,
    )
    .await
    .unwrap();
    let result = initialization::assess_output(&sparse, &manager.child_token())
        .unwrap()
        .unwrap();
    let unchanged = original_hashes == [hash(&case.database), hash(sample), hash(&mask)];
    let receipt = serde_json::json!({
        "inputImages": images.len(), "registeredImages": result.registered_images,
        "validIntrinsics": result.valid_intrinsics, "keypoints": features,
        "elapsedSeconds": begin.elapsed().as_secs_f64(), "model": result.model,
        "matching": "native exhaustive_matcher", "alphaPremultipliedRgb": true,
        "grayscaleMask": true, "sourceHashesUnchanged": unchanged, "brushExecutions": 0,
    });
    std::fs::write(
        case.output.join("result.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!("{receipt}");
    assert!(unchanged && result.valid_intrinsics);
    assert_eq!(result.registered_images, images.len() as u64);
}
