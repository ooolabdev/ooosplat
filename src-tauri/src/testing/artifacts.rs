use super::*;
use crate::reconstruction::{
    colmap_model::ColmapCamera,
    direct_input::{Model, ModelImage, Observation, Point},
};
use crate::video::panorama::PosedImage;
use rusqlite::{params, Connection};
use std::{collections::BTreeMap, io::Write};

const POINTS: usize = 32;
pub(super) fn prepare_inputs(h: &Harness) -> Result<()> {
    std::fs::create_dir_all(h.root.join("fixtures"))?;
    let paths = crate::engines::EnginePaths::from_root(h.root.join("engines"));
    for path in [&paths.ffmpeg, &paths.ffprobe, &paths.colmap, &paths.brush] {
        std::fs::create_dir_all(path.parent().unwrap())?;
        if !path.exists() {
            std::fs::write(path, b"E2E placeholder, never executable")?;
        }
    }
    std::fs::write(
        h.root.join("engines/vocabulary.bin"),
        b"E2E vocabulary fixture",
    )?;
    for i in &h.scenario.inputs {
        if i.count < 2
            || i.count > 200
            || i.width < 8
            || i.height < 8
            || i.width > 2048
            || i.height > 2048
            || !i.fps.is_finite()
            || i.fps <= 0.
            || !i.duration.is_finite()
            || i.duration <= 0.
            || i.duration * i.fps > 500.
        {
            return Err(invalid(
                "fixture size or timing exceeds the small-test budget",
            ));
        }
        if !i.timestamps.is_empty()
            && (i.timestamps.len() != (i.fps * i.duration).round() as usize
                || i.timestamps.iter().any(|v| !v.is_finite())
                || i.timestamps.windows(2).any(|p| p[1] <= p[0]))
        {
            return Err(invalid("invalid fixture timestamps"));
        }
        let path = safe_path(&h.root, &h.root.join("fixtures").join(&i.name))?;
        std::fs::create_dir_all(path.parent().ok_or_else(|| invalid("fixture parent"))?)?;
        if i.kind == "video" {
            if !path.exists() {
                std::fs::write(
                    path,
                    b"OOOSplat E2E video identity; metadata supplied by process adapter",
                )?;
            }
        } else if i.kind == "images" {
            std::fs::create_dir_all(&path)?;
            for n in 0..i.count {
                let (mut w, h) = if i.portrait && n % 2 == 1 {
                    (i.height, i.width)
                } else {
                    (i.width, i.height)
                };
                if i.mismatched_dimensions && n == i.count - 1 {
                    w += 1;
                }
                let p = path.join(format!(
                    "image-{n:04}.{}",
                    if i.jpeg { "jpg" } else { "png" }
                ));
                if !p.exists() {
                    write_image_tagged(
                        &p,
                        w,
                        h,
                        i.alpha,
                        i.transparent,
                        i.name.bytes().fold(n as u8, u8::wrapping_add),
                    )?;
                }
            }
        } else {
            return Err(invalid("unknown fixture kind"));
        }
    }
    Ok(())
}
pub(super) fn write_image(
    path: &Path,
    width: u32,
    height: u32,
    alpha: bool,
    transparent: bool,
) -> Result<()> {
    write_image_tagged(path, width, height, alpha, transparent, 100)
}
pub(super) fn write_image_tagged(
    path: &Path,
    width: u32,
    height: u32,
    alpha: bool,
    transparent: bool,
    tag: u8,
) -> Result<()> {
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| invalid("image without parent"))?,
    )?;
    let img = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([
            (x % 255) as u8,
            (y % 255) as u8,
            tag,
            if transparent && x < width / 4 { 0 } else { 255 },
        ])
    });
    let dynamic = if alpha {
        image::DynamicImage::ImageRgba8(img)
    } else {
        image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(img).to_rgb8())
    };
    dynamic.save(path).map_err(invalid)
}
fn sql<T>(value: rusqlite::Result<T>) -> Result<T> {
    value.map_err(invalid)
}
fn schema(db: &Connection) -> Result<()> {
    sql(db.execute_batch("CREATE TABLE IF NOT EXISTS rigs(rig_id INTEGER PRIMARY KEY,ref_sensor_type INTEGER NOT NULL,ref_sensor_id INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS rig_sensors(rig_id INTEGER,sensor_type INTEGER,sensor_id INTEGER); CREATE TABLE IF NOT EXISTS frames(frame_id INTEGER PRIMARY KEY,rig_id INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS frame_data(frame_id INTEGER NOT NULL,sensor_id INTEGER NOT NULL,sensor_type INTEGER NOT NULL,data_id INTEGER PRIMARY KEY);"))?;
    sql(db.execute_batch("CREATE TABLE IF NOT EXISTS cameras(camera_id INTEGER PRIMARY KEY,model INTEGER NOT NULL,width INTEGER NOT NULL,height INTEGER NOT NULL,params BLOB NOT NULL,prior_focal_length INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS images(image_id INTEGER PRIMARY KEY,name TEXT NOT NULL UNIQUE,camera_id INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS keypoints(image_id INTEGER PRIMARY KEY,rows INTEGER NOT NULL,cols INTEGER NOT NULL,data BLOB); CREATE TABLE IF NOT EXISTS descriptors(image_id INTEGER PRIMARY KEY,type INTEGER DEFAULT 0,rows INTEGER NOT NULL,cols INTEGER NOT NULL,data BLOB); CREATE TABLE IF NOT EXISTS matches(pair_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB); CREATE TABLE IF NOT EXISTS two_view_geometries(pair_id INTEGER PRIMARY KEY,rows INTEGER,cols INTEGER,data BLOB,config INTEGER DEFAULT 2);"))?;
    Ok(())
}
fn xy(n: usize) -> (f64, f64) {
    (8. + (n % 8) as f64, 8. + (n / 8) as f64)
}
pub(super) fn features(args: &[String]) -> Result<String> {
    let database = engine::argument(args, "--database_path")?;
    let images = PathBuf::from(engine::argument(args, "--image_path")?);
    let db = sql(Connection::open(database))?;
    schema(&db)?;
    let names = if let Ok(list) = engine::argument(args, "--image_list_path") {
        std::fs::read_to_string(list)?
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        crate::video::list_images(&images)?
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    };
    let mut camera = engine::argument(args, "--ImageReader.existing_camera_id")
        .ok()
        .and_then(|n| n.parse::<u32>().ok());
    for name in &names {
        if sql(
            db.query_row("SELECT COUNT(*) FROM images WHERE name=?1", [name], |r| {
                r.get::<_, u32>(0)
            }),
        )? > 0
        {
            continue;
        }
        let (w, h) = image::image_dimensions(images.join(name)).map_err(invalid)?;
        let model = engine::argument(args, "--ImageReader.camera_model").unwrap_or("SIMPLE_RADIAL");
        if !matches!(model, "SPHERICAL" | "EQUIRECTANGULAR" | "SIMPLE_RADIAL") {
            return Err(invalid(
                "fixture adapter does not support this camera model",
            ));
        }
        let params = if let Ok(parameters) = engine::argument(args, "--ImageReader.camera_params") {
            parameters
                .split(',')
                .map(str::parse::<f64>)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(invalid)?
        } else if model == "SPHERICAL" || model == "EQUIRECTANGULAR" {
            vec![w as f64, h as f64]
        } else {
            vec![w.max(h) as f64 * 0.8, w as f64 / 2., h as f64 / 2., 0.]
        };
        if camera.is_none() {
            let id = sql(db.query_row(
                "SELECT COALESCE(MAX(camera_id),0)+1 FROM cameras",
                [],
                |r| r.get::<_, u32>(0),
            ))?;
            let bytes = params
                .into_iter()
                .flat_map(f64::to_le_bytes)
                .collect::<Vec<_>>();
            sql(db.execute(
                "INSERT INTO cameras VALUES(?1,?2,?3,?4,?5,1)",
                params![
                    id,
                    if model == "SPHERICAL" || model == "EQUIRECTANGULAR" {
                        17
                    } else {
                        2
                    },
                    w,
                    h,
                    bytes
                ],
            ))?;
            camera = Some(id);
        }
        let id = sql(
            db.query_row("SELECT COALESCE(MAX(image_id),0)+1 FROM images", [], |r| {
                r.get::<_, u32>(0)
            }),
        )?;
        sql(db.execute(
            "INSERT INTO images VALUES(?1,?2,?3)",
            params![id, name, camera],
        ))?;
        sql(db.execute("INSERT OR IGNORE INTO rigs VALUES(?1,0,?1)", [camera]))?;
        sql(db.execute("INSERT INTO frames VALUES(?1,?2)", params![id, camera]))?;
        sql(db.execute(
            "INSERT INTO frame_data VALUES(?1,?2,0,?1)",
            params![id, camera],
        ))?;
        let keypoints = (0..POINTS)
            .flat_map(|n| {
                let (x, y) = xy(n);
                [x as f32, y as f32]
            })
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        sql(db.execute(
            "INSERT INTO keypoints VALUES(?1,?2,2,?3)",
            params![id, POINTS, keypoints],
        ))?;
        sql(db.execute(
            "INSERT INTO descriptors(image_id,rows,cols,data) VALUES(?1,?2,128,?3)",
            params![id, POINTS, vec![0_u8; POINTS * 128]],
        ))?;
        lifecycle::partial(
            Path::new(database),
            "features",
            names.iter().position(|n| n == name).unwrap() + 1,
            names.len(),
        )?;
    }
    Ok(names
        .iter()
        .enumerate()
        .map(|(n, _)| format!("Processed file [{}/{}]\n", n + 1, names.len()))
        .collect())
}
pub(super) fn matches(args: &[String]) -> Result<String> {
    let database = PathBuf::from(engine::argument(args, "--database_path")?);
    let db = sql(Connection::open(&database))?;
    let mut q = sql(db.prepare("SELECT image_id,name FROM images ORDER BY image_id"))?;
    let rows = sql(q.query_map([], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?))))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(invalid)?;
    let lookup = rows
        .iter()
        .map(|(id, name)| (name.as_str(), *id))
        .collect::<BTreeMap<_, _>>();
    let mut pairs = std::collections::BTreeSet::new();
    if args.first().is_some_and(|op| op == "matches_importer") {
        let text = std::fs::read_to_string(engine::argument(args, "--match_list_path")?)?;
        for line in text.lines().filter(|l| !l.is_empty()) {
            let names = line.split_whitespace().collect::<Vec<_>>();
            if names.len() != 2 || names[0] == names[1] {
                return Err(invalid("invalid requested match pair"));
            }
            let a = *lookup
                .get(names[0])
                .ok_or_else(|| invalid("unknown pair image"))?;
            let b = *lookup
                .get(names[1])
                .ok_or_else(|| invalid("unknown pair image"))?;
            pairs.insert((a.min(b), a.max(b)));
        }
    } else if args.first().is_some_and(|op| op == "sequential_matcher") {
        let mut ordered = rows.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|(_, name)| name);
        for (index, (a, _)) in ordered.iter().enumerate() {
            for exponent in 0..10 {
                if let Some((b, _)) = ordered.get(index + (1 << exponent)) {
                    pairs.insert(((*a).min(*b), (*a).max(*b)));
                }
            }
        }
    } else {
        for (index, (a, _)) in rows.iter().enumerate() {
            for (b, _) in rows.iter().skip(index + 1) {
                pairs.insert(((*a).min(*b), (*a).max(*b)));
            }
        }
    }
    let total = pairs.len();
    let mut transaction = false;
    let mut observed = std::collections::BTreeSet::new();
    for (index, (a, b)) in pairs.into_iter().enumerate() {
        if !transaction {
            sql(db.execute_batch("BEGIN"))?;
            transaction = true;
        }
        let pair_id = a as u64 * 2_147_483_647 + b as u64;
        let bytes = (0..POINTS as u32)
            .flat_map(|n| [n, n])
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        sql(db.execute(
            "INSERT OR REPLACE INTO matches VALUES(?1,?2,2,?3)",
            params![pair_id, POINTS, bytes],
        ))?;
        sql(db.execute("INSERT OR REPLACE INTO two_view_geometries(pair_id,rows,cols,data,config) VALUES(?1,?2,2,?3,2)",params![pair_id,POINTS,bytes]))?;
        let first = rows.iter().find(|r| r.0 == a).unwrap().1.as_str();
        let second = rows.iter().find(|r| r.0 == b).unwrap().1.as_str();
        let batch = |name: &str| {
            name.find("_s")
                .map(|n| name[..(n + 7).min(name.len())].to_owned())
        };
        let op = if batch(first) == batch(second) {
            "matchingIntra"
        } else {
            "matchingCross"
        };
        if observed.insert(op) || index % 64 == 63 || index + 1 == total {
            sql(db.execute_batch("COMMIT"))?;
            transaction = false;
            lifecycle::partial(&database, op, index + 1, total)?;
        }
    }
    Ok(format!("Matching block [1/1] ({total} requested pairs)\n"))
}
#[cfg(test)]
pub(super) fn mapper(args: &[String], registered: Option<usize>) -> Result<String> {
    mapper_impl(
        args,
        registered,
        &std::collections::BTreeMap::new(),
        &lifecycle::Scope::default(),
    )
}
pub(super) fn mapper_scoped(
    args: &[String],
    fault: Option<&Fault>,
    scope: &lifecycle::Scope,
) -> Result<String> {
    mapper_impl(
        args,
        fault.and_then(|f| f.registered),
        &fault
            .map(|f| f.registered_by_source.clone())
            .unwrap_or_default(),
        scope,
    )
}
fn mapper_impl(
    args: &[String],
    registered: Option<usize>,
    limits: &BTreeMap<String, usize>,
    scope: &lifecycle::Scope,
) -> Result<String> {
    let db = sql(Connection::open(engine::argument(args, "--database_path")?))?;
    let mut q = sql(db.prepare("SELECT image_id,name,camera_id FROM images ORDER BY image_id"))?;
    let mut rows = sql(q.query_map([], |r| {
        Ok((
            r.get::<_, u32>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u32>(2)?,
        ))
    }))?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(invalid)?;
    if let Some(count) = registered {
        rows.truncate(count);
    }
    for (name, members) in lifecycle::source_members(scope) {
        if let Some(&limit) = limits.get(&name) {
            let mut seen = 0;
            let prefix = members
                .first()
                .and_then(|n| n.find("_s").map(|i| &n[..(i + 7).min(n.len())]));
            rows.retain(|(_, n, _)| {
                if members.contains(n) || prefix.is_some_and(|p| n.starts_with(p)) {
                    seen += 1;
                    seen <= limit
                } else {
                    true
                }
            });
        }
    }
    let mut cameras = BTreeMap::new();
    for (_, _, id) in &rows {
        let (model, w, h, bytes) = sql(db.query_row(
            "SELECT model,width,height,params FROM cameras WHERE camera_id=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                ))
            },
        ))?;
        cameras.insert(
            *id,
            ColmapCamera {
                id: *id,
                model_id: model,
                model: if model == 17 {
                    "EQUIRECTANGULAR"
                } else {
                    "SIMPLE_RADIAL"
                }
                .into(),
                width: w,
                height: h,
                params: bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| f64::from_le_bytes(*c))
                    .collect(),
            },
        );
    }
    let mut model = Model {
        cameras: cameras.into_values().collect(),
        images: rows
            .iter()
            .enumerate()
            .map(|(n, (id, name, camera))| ModelImage {
                pose: PosedImage {
                    id: *id,
                    camera_id: *camera,
                    name: name.clone(),
                    qvec: [1., 0., 0., 0.],
                    tvec: [n as f64 * 0.1, 0., 0.],
                },
                observations: (0..POINTS)
                    .map(|i| {
                        let (x, y) = xy(i);
                        Observation {
                            x,
                            y,
                            point: Some(i as u64 + 1),
                        }
                    })
                    .collect(),
            })
            .collect(),
        points: (0..POINTS)
            .map(|i| Point {
                id: i as u64 + 1,
                xyz: [i as f64 * 0.01, 0.2, 3.],
                rgb: [128; 3],
                error: 0.,
                track: rows.iter().map(|(id, _, _)| (*id, i as u32)).collect(),
            })
            .collect(),
    };
    if let Ok(input) = engine::argument(args, "--input_path") {
        let fixed = crate::reconstruction::direct_input::read_model(Path::new(input))?;
        for image in &mut model.images {
            if let Some(saved) = fixed
                .images
                .iter()
                .find(|i| i.pose.id == image.pose.id && i.pose.name == image.pose.name)
            {
                image.pose = saved.pose.clone();
            }
        }
        for camera in &mut model.cameras {
            if let Some(saved) = fixed.cameras.iter().find(|c| c.id == camera.id) {
                *camera = saved.clone();
            }
        }
    }
    model.validate()?;
    let out = PathBuf::from(engine::argument(args, "--output_path")?);
    model.write(&out.join("0"))?;
    Ok(format!(
        "Registering initial image pair #1 and #2\nRegistering image #{} (num_reg_frames={})\n",
        rows.len(),
        rows.len()
    ))
}
pub(super) fn gaussian(path: &Path, count: usize) -> Result<()> {
    gaussian_tagged(path, count, 0)
}
pub(super) fn gaussian_tagged(path: &Path, count: usize, tag: u32) -> Result<()> {
    std::fs::create_dir_all(path.parent().ok_or_else(|| invalid("PLY parent"))?)?;
    let mut file = std::fs::File::create(path)?;
    write!(
        file,
        "ply\nformat binary_little_endian 1.0\nelement vertex {count}\n"
    )?;
    for name in [
        "x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2",
        "rot_0", "rot_1", "rot_2", "rot_3",
    ] {
        writeln!(file, "property float {name}")?;
    }
    writeln!(file, "end_header")?;
    for n in 0..count {
        for v in [
            n as f32 * 0.01 + tag as f32 * 0.00001,
            0.,
            1.,
            0.5,
            0.5,
            0.5,
            0.,
            -2.,
            -2.,
            -2.,
            1.,
            0.,
            0.,
            0.,
        ] {
            file.write_all(&v.to_le_bytes())?;
        }
        if n == count / 2 {
            file.flush()?;
            file.sync_all()?;
            lifecycle::partial(path, "training", n + 1, count)?;
        }
    }
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_specific_gaussians_are_deterministic_distinct_and_valid() {
        let directory = tempfile::tempdir().unwrap();
        let a = directory.path().join("a.ply");
        let b = directory.path().join("b.ply");
        let again = directory.path().join("again.ply");
        gaussian_tagged(&a, 64, 1001).unwrap();
        gaussian_tagged(&b, 64, 2001).unwrap();
        gaussian_tagged(&again, 64, 1001).unwrap();
        assert_ne!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&again).unwrap());
        for file in [a, b] {
            assert_eq!(
                crate::reconstruction::ply::inspect_gaussian_ply(&file)
                    .unwrap()
                    .splat_count,
                64
            );
        }
    }
    #[test]
    fn generated_results_pass_production_model_and_ply_validation() {
        let dir = tempfile::tempdir().unwrap();
        let frames = dir.path().join("frames");
        std::fs::create_dir(&frames).unwrap();
        for n in 0..3 {
            write_image(&frames.join(format!("{n}.png")), 96, 64, true, true).unwrap();
        }
        let db = dir.path().join("database.db");
        let args = vec![
            "--database_path".into(),
            db.display().to_string(),
            "--image_path".into(),
            frames.display().to_string(),
            "--ImageReader.camera_model".into(),
            "SIMPLE_RADIAL".into(),
        ];
        features(&args).unwrap();
        matches(&args).unwrap();
        let output = dir.path().join("sparse");
        let mut mapping = args;
        mapping.extend(["--output_path".into(), output.display().to_string()]);
        mapper(&mapping, None).unwrap();
        let model = crate::reconstruction::direct_input::read_model(&output.join("0")).unwrap();
        assert_eq!(model.images.len(), 3);
        assert_eq!(model.points.len(), POINTS);
        assert_eq!(model.cameras.len(), 1);
        let inspection =
            crate::reconstruction::direct_input::inspect(&crate::pipeline::inputs::ColmapInput {
                model: output.join("0"),
                images: frames,
                database: Some(db),
                identity: None,
            })
            .unwrap();
        assert!(
            inspection.reshoot_eligible,
            "Simulated COLMAP output must pass the real rig/feature/model binding validator"
        );
        let ply = dir.path().join("final.ply");
        gaussian(&ply, 64).unwrap();
        assert_eq!(
            crate::reconstruction::ply::inspect_gaussian_ply(&ply)
                .unwrap()
                .splat_count,
            64
        );
    }
}
