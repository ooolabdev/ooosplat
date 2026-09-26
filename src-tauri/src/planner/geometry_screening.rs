use std::{
    collections::{HashMap, HashSet},
    io::{BufReader, Read},
    path::Path,
};

use rusqlite::{Connection, OpenFlags};

use crate::{
    error::{Result, SplatError},
    presets::Quality,
    video::{FramePlan, PlannedFrame},
};

use super::{
    frame::candidate_score, FrameCandidate, GeometryImageMetric, GeometryProbeReason,
    GeometryProbeStatus, GeometryScreeningDecision, GeometryScreeningReport,
    ReconstructionCandidate, ReconstructionMetrics, ReconstructionViability, WeakGeometryInterval,
};

pub const GEOMETRY_SCREENING_THRESHOLD_PROFILE: &str = "provisional_video002_v1";

/// Benchmark-driven starting values. They are deliberately centralized so a
/// later benchmark can tune them without changing the screening algorithm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryScreeningThresholds {
    pub triangulation_median_low: f64,
    pub triangulation_p25_low: f64,
    pub point_diversity_low: f64,
    pub mean_track_redundancy_high: f64,
    pub weak_region_points_ratio: f64,
    pub weak_region_triangulation_ratio: f64,
    pub weak_region_min_images: usize,
    pub probe_frame_growth_ratio: f64,
    pub minimum_probe_frames: usize,
    pub minimum_probe_registration_retention: f64,
}

impl Default for GeometryScreeningThresholds {
    fn default() -> Self {
        Self {
            triangulation_median_low: 0.30,
            triangulation_p25_low: 0.15,
            point_diversity_low: 0.06,
            mean_track_redundancy_high: 14.0,
            weak_region_points_ratio: 0.45,
            weak_region_triangulation_ratio: 0.60,
            weak_region_min_images: 4,
            probe_frame_growth_ratio: 0.12,
            minimum_probe_frames: 4,
            minimum_probe_registration_retention: 0.90,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GeometryProbeFrameBudget {
    pub requested: usize,
    pub available: usize,
}

#[derive(Debug, Clone)]
struct DatabaseImage {
    name: String,
    detected_features: u64,
}

pub fn screening_applicable(
    quality: Quality,
    planner_enabled: bool,
    viability: ReconstructionViability,
    remaining_candidates: usize,
) -> bool {
    quality == Quality::High
        && planner_enabled
        && viability != ReconstructionViability::NotViable
        && remaining_candidates > 0
}

pub fn can_start_new_probe(attempted: bool, status: GeometryProbeStatus) -> bool {
    !attempted && status == GeometryProbeStatus::NotEvaluated
}

pub fn probe_candidate_acceptable(
    baseline: &ReconstructionCandidate,
    probe: &ReconstructionCandidate,
    thresholds: GeometryScreeningThresholds,
) -> bool {
    probe.viability != ReconstructionViability::NotViable
        && probe.metrics.registered_images as f64
            >= baseline.metrics.registered_images as f64
                * thresholds.minimum_probe_registration_retention
}

pub fn geometry_probe_reasons(report: &GeometryScreeningReport) -> Vec<GeometryProbeReason> {
    let mut reasons = Vec::new();
    if report.triangulation_underfilled {
        reasons.push(GeometryProbeReason::TriangulationUnderfilled);
    }
    if report.track_redundancy_high {
        reasons.push(GeometryProbeReason::TrackRedundancy);
    }
    if report.continuous_weak_region {
        reasons.push(GeometryProbeReason::ContinuousWeakRegion);
    }
    reasons
}

pub fn geometry_probe_budget(
    plan: &FramePlan,
    duration_seconds: f64,
    max_fps: f64,
    remaining_candidates: usize,
    thresholds: GeometryScreeningThresholds,
) -> GeometryProbeFrameBudget {
    let requested = ((plan.selected_frames.len() as f64 * thresholds.probe_frame_growth_ratio)
        .ceil() as usize)
        .max(thresholds.minimum_probe_frames);
    // floor keeps the final average at or below the existing High 15 fps cap.
    let quality_ceiling = (duration_seconds.max(0.0) * max_fps.max(0.0)).floor() as usize;
    let headroom = quality_ceiling.saturating_sub(plan.selected_frames.len());
    GeometryProbeFrameBudget {
        requested,
        available: requested.min(headroom).min(remaining_candidates),
    }
}

pub fn analyze_sparse_geometry(
    model: &Path,
    database: &Path,
    frame_plan: &FramePlan,
    reconstruction: &ReconstructionMetrics,
    thresholds: GeometryScreeningThresholds,
) -> Result<GeometryScreeningReport> {
    let database_images = read_database_images(database)?;
    let database_by_name: HashMap<&str, &DatabaseImage> = database_images
        .values()
        .map(|image| (image.name.as_str(), image))
        .collect();
    let timestamps: HashMap<u64, f64> = frame_plan
        .candidate_frames
        .iter()
        .chain(frame_plan.selected_frames.iter())
        .map(|frame| (frame.source_frame_index, frame.timestamp_seconds))
        .collect();

    let mut reader = BufReader::new(std::fs::File::open(model.join("images.bin"))?);
    let registered_images = read_u64(&mut reader)? as usize;
    let mut per_image = Vec::with_capacity(registered_images);
    for _ in 0..registered_images {
        let image_id = read_u32(&mut reader)?;
        for _ in 0..7 {
            let _ = read_f64(&mut reader)?;
        }
        let _camera_id = read_u32(&mut reader)?;
        let name = read_c_string(&mut reader)?;
        let point_count = read_u64(&mut reader)?;
        let mut observed_points = 0_u64;
        for _ in 0..point_count {
            let _x = read_f64(&mut reader)?;
            let _y = read_f64(&mut reader)?;
            if read_u64(&mut reader)? != u64::MAX {
                observed_points += 1;
            }
        }
        let database_image = database_images
            .get(&image_id)
            .filter(|image| image.name == name)
            .or_else(|| database_by_name.get(name.as_str()).copied())
            .ok_or_else(|| {
                SplatError::Process(format!(
                    "COLMAP model image {image_id} ({name}) is missing from its database"
                ))
            })?;
        let source_frame_index = source_frame_index(&name).ok_or_else(|| {
            SplatError::Process(format!(
                "Unable to recover source frame order from COLMAP image name {name}"
            ))
        })?;
        let detected_features = database_image.detected_features;
        let triangulation_ratio = if detected_features == 0 {
            0.0
        } else {
            observed_points as f64 / detected_features as f64
        };
        per_image.push(GeometryImageMetric {
            image_id,
            source_frame_index,
            timestamp_seconds: timestamps.get(&source_frame_index).copied(),
            detected_features,
            observed_points,
            triangulation_ratio,
        });
    }
    per_image.sort_by(|left, right| {
        left.source_frame_index
            .cmp(&right.source_frame_index)
            .then_with(|| left.image_id.cmp(&right.image_id))
    });

    let triangulation_ratios: Vec<f64> = per_image
        .iter()
        .map(|image| image.triangulation_ratio)
        .collect();
    let observed_points: Vec<f64> = per_image
        .iter()
        .map(|image| image.observed_points as f64)
        .collect();
    let median_triangulation_ratio = percentile(&triangulation_ratios, 0.50);
    let p25_triangulation_ratio = percentile(&triangulation_ratios, 0.25);
    let minimum_triangulation_ratio = triangulation_ratios
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let minimum_triangulation_ratio = if minimum_triangulation_ratio.is_finite() {
        minimum_triangulation_ratio
    } else {
        0.0
    };
    let median_observed_points = percentile(&observed_points, 0.50);
    let minimum_weak_run = thresholds
        .weak_region_min_images
        .max(registered_images.div_ceil(100));
    let weak_flags: Vec<bool> = per_image
        .iter()
        .map(|image| {
            (image.observed_points as f64)
                < median_observed_points * thresholds.weak_region_points_ratio
                && image.triangulation_ratio
                    < median_triangulation_ratio * thresholds.weak_region_triangulation_ratio
        })
        .collect();
    let weak_geometry_intervals = weak_intervals(&per_image, &weak_flags, minimum_weak_run);
    let mean_track_length = ratio(reconstruction.observations, reconstruction.points_3d);
    let point_diversity_ratio = ratio(reconstruction.points_3d, reconstruction.observations);
    let triangulation_underfilled = median_triangulation_ratio
        < thresholds.triangulation_median_low
        && p25_triangulation_ratio < thresholds.triangulation_p25_low;
    let track_redundancy_high = point_diversity_ratio < thresholds.point_diversity_low
        && mean_track_length > thresholds.mean_track_redundancy_high;
    let continuous_weak_region = !weak_geometry_intervals.is_empty();
    let decision = if triangulation_underfilled || track_redundancy_high || continuous_weak_region {
        GeometryScreeningDecision::ProbeRecommended
    } else {
        GeometryScreeningDecision::NoProbe
    };
    Ok(GeometryScreeningReport {
        threshold_profile: GEOMETRY_SCREENING_THRESHOLD_PROFILE.into(),
        registered_images,
        points_3d: reconstruction.points_3d,
        observations: reconstruction.observations,
        mean_track_length,
        point_diversity_ratio,
        median_triangulation_ratio,
        p25_triangulation_ratio,
        minimum_triangulation_ratio,
        median_observed_points,
        weak_geometry_intervals,
        per_image,
        triangulation_underfilled,
        track_redundancy_high,
        continuous_weak_region,
        decision,
    })
}

pub fn plan_geometry_probe_backfill(
    frame_plan: &FramePlan,
    candidates: &[FrameCandidate],
    report: &GeometryScreeningReport,
    budget: usize,
    thresholds: GeometryScreeningThresholds,
) -> Vec<PlannedFrame> {
    if budget == 0 {
        return Vec::new();
    }
    let mut selected: Vec<PlannedFrame> = frame_plan.selected_frames.clone();
    selected.sort_by(|a, b| a.timestamp_seconds.total_cmp(&b.timestamp_seconds));
    let original: HashSet<u64> = selected
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect();
    let mut additions = Vec::new();
    let spacing = median_selected_gap(&selected).max(0.001);

    if report.continuous_weak_region {
        let mut ranges: Vec<_> = report
            .weak_geometry_intervals
            .iter()
            .filter_map(|interval| {
                Some((
                    interval.start_timestamp? - spacing,
                    interval.end_timestamp? + spacing,
                    interval.median_triangulation_ratio,
                ))
            })
            .collect();
        ranges.sort_by(|a, b| a.2.total_cmp(&b.2));
        select_from_ranges(
            candidates,
            &ranges,
            budget,
            spacing,
            &mut selected,
            &mut additions,
        );
    }

    if report.triangulation_underfilled && additions.len() < budget {
        let mut ranges = low_triangulation_ranges(report, thresholds.triangulation_median_low);
        ranges.sort_by(|a, b| a.2.total_cmp(&b.2));
        select_from_ranges(
            candidates,
            &ranges,
            budget,
            spacing,
            &mut selected,
            &mut additions,
        );
    }

    if report.track_redundancy_high && additions.len() < budget {
        select_largest_temporal_gaps(candidates, budget, &mut selected, &mut additions);
    }

    additions.retain(|frame| !original.contains(&frame.source_frame_index));
    additions.sort_by_key(|frame| frame.source_frame_index);
    additions.dedup_by_key(|frame| frame.source_frame_index);
    additions.truncate(budget);
    additions
}

fn read_database_images(database: &Path) -> Result<HashMap<u32, DatabaseImage>> {
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(sql_error)?;
    let mut statement = connection
        .prepare(
            "SELECT images.image_id, images.name, COALESCE(keypoints.rows, 0) \
             FROM images LEFT JOIN keypoints ON images.image_id = keypoints.image_id",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                DatabaseImage {
                    name: row.get(1)?,
                    detected_features: row.get(2)?,
                },
            ))
        })
        .map_err(sql_error)?;
    let mut output = HashMap::new();
    for row in rows {
        let (id, image) = row.map_err(sql_error)?;
        output.insert(id, image);
    }
    Ok(output)
}

fn weak_intervals(
    images: &[GeometryImageMetric],
    weak: &[bool],
    minimum_run: usize,
) -> Vec<WeakGeometryInterval> {
    let mut intervals = Vec::new();
    let mut start = None;
    for index in 0..=images.len() {
        let is_weak = weak.get(index).copied().unwrap_or(false);
        if is_weak {
            start.get_or_insert(index);
        } else if let Some(first) = start.take() {
            let run = &images[first..index];
            if run.len() >= minimum_run {
                intervals.push(WeakGeometryInterval {
                    start_source_frame_index: run[0].source_frame_index,
                    end_source_frame_index: run[run.len() - 1].source_frame_index,
                    start_timestamp: run[0].timestamp_seconds,
                    end_timestamp: run[run.len() - 1].timestamp_seconds,
                    image_count: run.len(),
                    median_observed_points: percentile(
                        &run.iter()
                            .map(|image| image.observed_points as f64)
                            .collect::<Vec<_>>(),
                        0.50,
                    ),
                    median_triangulation_ratio: percentile(
                        &run.iter()
                            .map(|image| image.triangulation_ratio)
                            .collect::<Vec<_>>(),
                        0.50,
                    ),
                });
            }
        }
    }
    intervals
}

fn low_triangulation_ranges(
    report: &GeometryScreeningReport,
    threshold: f64,
) -> Vec<(f64, f64, f64)> {
    let mut ranges = Vec::new();
    let mut start = None;
    for index in 0..=report.per_image.len() {
        let low = report
            .per_image
            .get(index)
            .is_some_and(|image| image.triangulation_ratio < threshold);
        if low {
            start.get_or_insert(index);
        } else if let Some(first) = start.take() {
            let run = &report.per_image[first..index];
            let Some(start_time) = run.first().and_then(|image| image.timestamp_seconds) else {
                continue;
            };
            let Some(end_time) = run.last().and_then(|image| image.timestamp_seconds) else {
                continue;
            };
            ranges.push((
                start_time,
                end_time,
                percentile(
                    &run.iter()
                        .map(|image| image.triangulation_ratio)
                        .collect::<Vec<_>>(),
                    0.50,
                ),
            ));
        }
    }
    ranges
}

fn select_from_ranges(
    candidates: &[FrameCandidate],
    ranges: &[(f64, f64, f64)],
    budget: usize,
    spacing: f64,
    selected: &mut Vec<PlannedFrame>,
    additions: &mut Vec<PlannedFrame>,
) {
    for (start, end, _) in ranges {
        while additions.len() < budget {
            let existing: HashSet<u64> = selected
                .iter()
                .map(|frame| frame.source_frame_index)
                .collect();
            let next = candidates
                .iter()
                .filter(|candidate| {
                    candidate.frame_index >= 0
                        && candidate.timestamp >= *start
                        && candidate.timestamp <= *end
                        && !existing.contains(&(candidate.frame_index as u64))
                })
                .max_by(|left, right| {
                    regional_candidate_score(left, selected, spacing)
                        .total_cmp(&regional_candidate_score(right, selected, spacing))
                });
            let Some(next) = next else {
                break;
            };
            let frame = PlannedFrame {
                source_frame_index: next.frame_index as u64,
                timestamp_seconds: next.timestamp,
            };
            selected.push(frame.clone());
            selected.sort_by(|a, b| a.timestamp_seconds.total_cmp(&b.timestamp_seconds));
            additions.push(frame);
        }
        if additions.len() >= budget {
            break;
        }
    }
}

fn regional_candidate_score(
    candidate: &FrameCandidate,
    selected: &[PlannedFrame],
    spacing: f64,
) -> f64 {
    let distance = selected
        .iter()
        .map(|frame| (frame.timestamp_seconds - candidate.timestamp).abs())
        .fold(f64::INFINITY, f64::min);
    let spacing_score = (distance / spacing.max(0.001)).clamp(0.0, 1.0);
    candidate_score(candidate) as f64 * 0.70 + spacing_score * 0.30
}

fn select_largest_temporal_gaps(
    candidates: &[FrameCandidate],
    budget: usize,
    selected: &mut Vec<PlannedFrame>,
    additions: &mut Vec<PlannedFrame>,
) {
    while additions.len() < budget {
        selected.sort_by(|a, b| a.timestamp_seconds.total_cmp(&b.timestamp_seconds));
        let existing: HashSet<u64> = selected
            .iter()
            .map(|frame| frame.source_frame_index)
            .collect();
        let mut best: Option<(&FrameCandidate, f64, f64)> = None;
        for candidate in candidates.iter().filter(|candidate| {
            candidate.frame_index >= 0 && !existing.contains(&(candidate.frame_index as u64))
        }) {
            let Some(window) = selected.windows(2).find(|window| {
                candidate.timestamp > window[0].timestamp_seconds
                    && candidate.timestamp < window[1].timestamp_seconds
            }) else {
                continue;
            };
            let gap = window[1].timestamp_seconds - window[0].timestamp_seconds;
            let midpoint = (window[0].timestamp_seconds + window[1].timestamp_seconds) * 0.5;
            let centrality = if gap > 0.0 {
                (1.0 - (candidate.timestamp - midpoint).abs() / (gap * 0.5)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let tie_break = centrality * 0.60 + candidate_score(candidate) as f64 * 0.40;
            if best.is_none_or(|(_, best_gap, best_tie)| {
                gap > best_gap || (gap == best_gap && tie_break > best_tie)
            }) {
                best = Some((candidate, gap, tie_break));
            }
        }
        let Some((next, _, _)) = best else {
            break;
        };
        let frame = PlannedFrame {
            source_frame_index: next.frame_index as u64,
            timestamp_seconds: next.timestamp,
        };
        selected.push(frame.clone());
        additions.push(frame);
    }
}

fn median_selected_gap(selected: &[PlannedFrame]) -> f64 {
    let gaps: Vec<f64> = selected
        .windows(2)
        .map(|window| window[1].timestamp_seconds - window[0].timestamp_seconds)
        .filter(|gap| *gap > 0.0)
        .collect();
    percentile(&gaps, 0.50)
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = quantile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let weight = position - lower as f64;
        sorted[lower] * (1.0 - weight) + sorted[upper] * weight
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn source_frame_index(name: &str) -> Option<u64> {
    Path::new(name)
        .file_stem()?
        .to_str()?
        .strip_prefix("frame_")?
        .parse()
        .ok()
}

fn read_c_string(reader: &mut impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        reader.read_exact(&mut byte)?;
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes)
        .map_err(|error| SplatError::Process(format!("Invalid COLMAP image name: {error}")))
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_f64(reader: &mut impl Read) -> Result<f64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(f64::from_le_bytes(bytes))
}

fn sql_error(error: rusqlite::Error) -> SplatError {
    SplatError::Process(format!("Unable to query COLMAP geometry metrics: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::{ReconstructionDecision, ReconstructionViability};
    use std::io::Write;

    fn frame(index: u64, timestamp: f64) -> PlannedFrame {
        PlannedFrame {
            source_frame_index: index,
            timestamp_seconds: timestamp,
        }
    }

    fn candidate(index: i64, timestamp: f64, quality: f32) -> FrameCandidate {
        FrameCandidate {
            frame_index: index,
            timestamp,
            sharpness_score: quality,
            exposure_score: quality,
            view_change_score: quality,
            motion_score: quality,
        }
    }

    fn report() -> GeometryScreeningReport {
        GeometryScreeningReport {
            registered_images: 100,
            points_3d: 1_000,
            observations: 10_000,
            mean_track_length: 10.0,
            point_diversity_ratio: 0.1,
            median_triangulation_ratio: 0.5,
            p25_triangulation_ratio: 0.4,
            minimum_triangulation_ratio: 0.1,
            median_observed_points: 100.0,
            ..GeometryScreeningReport::default()
        }
    }

    fn plan(selected: Vec<PlannedFrame>, candidates: Vec<PlannedFrame>) -> FramePlan {
        FramePlan {
            selected_frames: selected,
            candidate_frames: candidates,
            ..FramePlan::default()
        }
    }

    fn geometry_fixture(
        images: &[(u32, u64, u64)],
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        FramePlan,
    ) {
        let temporary = tempfile::tempdir().unwrap();
        let model = temporary.path().join("model");
        std::fs::create_dir_all(&model).unwrap();
        let database = temporary.path().join("database.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE images(image_id INTEGER PRIMARY KEY, name TEXT);\
                 CREATE TABLE keypoints(image_id INTEGER PRIMARY KEY, rows INTEGER);",
            )
            .unwrap();
        for (image_id, source_index, _) in images {
            let name = format!("frame_{source_index:010}.jpg");
            connection
                .execute(
                    "INSERT INTO images(image_id, name) VALUES(?1, ?2)",
                    rusqlite::params![image_id, name],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO keypoints(image_id, rows) VALUES(?1, 100)",
                    [image_id],
                )
                .unwrap();
        }
        drop(connection);

        let mut file = std::fs::File::create(model.join("images.bin")).unwrap();
        file.write_all(&(images.len() as u64).to_le_bytes())
            .unwrap();
        for (image_id, source_index, observed) in images {
            file.write_all(&image_id.to_le_bytes()).unwrap();
            for _ in 0..7 {
                file.write_all(&0.0_f64.to_le_bytes()).unwrap();
            }
            file.write_all(&1_u32.to_le_bytes()).unwrap();
            file.write_all(format!("frame_{source_index:010}.jpg\0").as_bytes())
                .unwrap();
            file.write_all(&100_u64.to_le_bytes()).unwrap();
            for point in 0..100_u64 {
                file.write_all(&(point as f64).to_le_bytes()).unwrap();
                file.write_all(&0.0_f64.to_le_bytes()).unwrap();
                file.write_all(
                    &(if point < *observed {
                        point + 1
                    } else {
                        u64::MAX
                    })
                    .to_le_bytes(),
                )
                .unwrap();
            }
        }
        drop(file);
        let frame_plan = plan(
            images
                .iter()
                .map(|(_, source, _)| frame(*source, *source as f64 / 10.0))
                .collect(),
            Vec::new(),
        );
        (temporary, model, database, frame_plan)
    }

    #[test]
    fn screening_is_high_planner_usable_and_candidate_only() {
        assert!(!screening_applicable(
            Quality::Fast,
            true,
            ReconstructionViability::Viable,
            10
        ));
        assert!(!screening_applicable(
            Quality::Balanced,
            true,
            ReconstructionViability::Viable,
            10
        ));
        assert!(!screening_applicable(
            Quality::High,
            true,
            ReconstructionViability::NotViable,
            10
        ));
        assert!(screening_applicable(
            Quality::High,
            true,
            ReconstructionViability::Viable,
            10
        ));
    }

    #[test]
    fn percentile_calculation_handles_median_and_p25() {
        let values = [0.1, 0.2, 0.3, 0.4, 0.5];
        assert_eq!(percentile(&values, 0.50), 0.3);
        assert_eq!(percentile(&values, 0.25), 0.2);
    }

    #[test]
    fn sparse_model_and_database_produce_real_per_image_quantiles_in_source_order() {
        let (_temporary, model, database, frame_plan) = geometry_fixture(&[
            (91, 40, 50),
            (7, 0, 10),
            (44, 30, 40),
            (2, 10, 20),
            (80, 20, 30),
        ]);
        let report = analyze_sparse_geometry(
            &model,
            &database,
            &frame_plan,
            &ReconstructionMetrics {
                points_3d: 100,
                observations: 300,
                ..ReconstructionMetrics::default()
            },
            GeometryScreeningThresholds::default(),
        )
        .unwrap();
        assert_eq!(report.median_triangulation_ratio, 0.30);
        assert_eq!(report.p25_triangulation_ratio, 0.20);
        assert_eq!(report.minimum_triangulation_ratio, 0.10);
        assert_eq!(report.decision, GeometryScreeningDecision::NoProbe);
        assert_eq!(
            report
                .per_image
                .iter()
                .map(|image| image.source_frame_index)
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40]
        );
    }

    #[test]
    fn one_bad_image_does_not_trigger_but_low_median_and_p25_do() {
        let (_temporary, model, database, frame_plan) = geometry_fixture(&[
            (1, 0, 1),
            (2, 10, 50),
            (3, 20, 50),
            (4, 30, 50),
            (5, 40, 50),
        ]);
        let healthy = analyze_sparse_geometry(
            &model,
            &database,
            &frame_plan,
            &ReconstructionMetrics {
                points_3d: 500,
                observations: 2_000,
                ..ReconstructionMetrics::default()
            },
            GeometryScreeningThresholds::default(),
        )
        .unwrap();
        assert!(!healthy.triangulation_underfilled);

        let (_temporary, model, database, frame_plan) = geometry_fixture(&[
            (1, 0, 10),
            (2, 10, 10),
            (3, 20, 10),
            (4, 30, 10),
            (5, 40, 10),
        ]);
        let underfilled = analyze_sparse_geometry(
            &model,
            &database,
            &frame_plan,
            &ReconstructionMetrics {
                points_3d: 500,
                observations: 2_000,
                ..ReconstructionMetrics::default()
            },
            GeometryScreeningThresholds::default(),
        )
        .unwrap();
        assert!(underfilled.triangulation_underfilled);
        assert_eq!(
            underfilled.decision,
            GeometryScreeningDecision::ProbeRecommended
        );
    }

    #[test]
    fn single_low_image_does_not_create_a_weak_interval() {
        let images: Vec<_> = (0..10)
            .map(|index| GeometryImageMetric {
                source_frame_index: index,
                observed_points: if index == 5 { 1 } else { 100 },
                triangulation_ratio: if index == 5 { 0.01 } else { 0.5 },
                ..GeometryImageMetric::default()
            })
            .collect();
        let weak: Vec<_> = images
            .iter()
            .map(|image| image.observed_points < 10)
            .collect();
        assert!(weak_intervals(&images, &weak, 4).is_empty());
    }

    #[test]
    fn continuous_weak_run_uses_source_order_and_minimum_length() {
        let ids = [90, 4, 70, 2, 60, 1];
        let images: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| GeometryImageMetric {
                image_id: *id,
                source_frame_index: index as u64 * 10,
                timestamp_seconds: Some(index as f64),
                observed_points: 1,
                triangulation_ratio: 0.01,
                ..GeometryImageMetric::default()
            })
            .collect();
        let intervals = weak_intervals(&images, &[true; 6], 4);
        assert_eq!(intervals.len(), 1);
        assert_eq!(intervals[0].start_source_frame_index, 0);
        assert_eq!(intervals[0].end_source_frame_index, 50);
    }

    #[test]
    fn track_redundancy_threshold_matches_video_002_but_not_historical() {
        let thresholds = GeometryScreeningThresholds::default();
        let video002_diversity = ratio(48_689, 980_482);
        let video002_track = ratio(980_482, 48_689);
        assert!(video002_diversity < thresholds.point_diversity_low);
        assert!(video002_track > thresholds.mean_track_redundancy_high);

        let historical_diversity = ratio(378_744, 4_382_417);
        let historical_track = ratio(4_382_417, 378_744);
        assert!(historical_diversity >= thresholds.point_diversity_low);
        assert!(historical_track <= thresholds.mean_track_redundancy_high);
    }

    #[test]
    fn global_underfill_requires_both_provisional_thresholds() {
        let thresholds = GeometryScreeningThresholds::default();
        let triggers = |median: f64, p25: f64| {
            median < thresholds.triangulation_median_low && p25 < thresholds.triangulation_p25_low
        };
        assert!(!triggers(0.29, 0.20));
        assert!(!triggers(0.40, 0.01));
        assert!(triggers(0.29, 0.14));
    }

    #[test]
    fn weak_region_backfill_prefers_its_time_range_and_deduplicates() {
        let mut screening = report();
        screening.continuous_weak_region = true;
        screening.weak_geometry_intervals = vec![WeakGeometryInterval {
            start_timestamp: Some(4.0),
            end_timestamp: Some(6.0),
            image_count: 4,
            ..WeakGeometryInterval::default()
        }];
        let frame_plan = plan(
            vec![
                frame(0, 0.0),
                frame(20, 2.0),
                frame(40, 4.0),
                frame(60, 6.0),
                frame(80, 8.0),
                frame(100, 10.0),
            ],
            (0..=10).map(|i| frame(i * 10, i as f64)).collect(),
        );
        let candidates: Vec<_> = (0..=10).map(|i| candidate(i * 10, i as f64, 0.5)).collect();
        let additions = plan_geometry_probe_backfill(
            &frame_plan,
            &candidates,
            &screening,
            3,
            GeometryScreeningThresholds::default(),
        );
        assert_eq!(additions.len(), 3);
        assert!(additions
            .iter()
            .all(|item| (2.0..=8.0).contains(&item.timestamp_seconds)));
        assert_eq!(
            additions
                .iter()
                .map(|item| item.source_frame_index)
                .collect::<HashSet<_>>()
                .len(),
            additions.len()
        );
    }

    #[test]
    fn low_triangulation_backfill_prefers_low_ratio_region() {
        let mut screening = report();
        screening.triangulation_underfilled = true;
        screening.per_image = (0..=10)
            .map(|index| GeometryImageMetric {
                source_frame_index: index * 10,
                timestamp_seconds: Some(index as f64),
                triangulation_ratio: if (4..=6).contains(&index) { 0.05 } else { 0.5 },
                ..GeometryImageMetric::default()
            })
            .collect();
        let frame_plan = plan(
            vec![frame(0, 0.0), frame(100, 10.0)],
            (0..=10).map(|i| frame(i * 10, i as f64)).collect(),
        );
        let candidates: Vec<_> = (0..=10).map(|i| candidate(i * 10, i as f64, 0.5)).collect();
        let additions = plan_geometry_probe_backfill(
            &frame_plan,
            &candidates,
            &screening,
            2,
            GeometryScreeningThresholds::default(),
        );
        assert!(additions
            .iter()
            .all(|item| (4.0..=6.0).contains(&item.timestamp_seconds)));
    }

    #[test]
    fn redundancy_backfill_splits_the_largest_temporal_gap() {
        let mut screening = report();
        screening.track_redundancy_high = true;
        let frame_plan = plan(
            vec![frame(0, 0.0), frame(20, 2.0), frame(100, 10.0)],
            Vec::new(),
        );
        let candidates = vec![
            candidate(10, 1.0, 1.0),
            candidate(40, 4.0, 0.5),
            candidate(60, 6.0, 0.5),
            candidate(80, 8.0, 0.5),
        ];
        let additions = plan_geometry_probe_backfill(
            &frame_plan,
            &candidates,
            &screening,
            2,
            GeometryScreeningThresholds::default(),
        );
        assert_eq!(additions[0].source_frame_index, 40);
        assert_eq!(additions[1].source_frame_index, 60);
    }

    #[test]
    fn probe_budget_is_twelve_percent_and_never_exceeds_high_fps() {
        let selected: Vec<_> = (0..422).map(|i| frame(i, i as f64 / 7.5)).collect();
        let frame_plan = plan(selected, Vec::new());
        let budget = geometry_probe_budget(
            &frame_plan,
            56.2,
            15.0,
            500,
            GeometryScreeningThresholds::default(),
        );
        assert_eq!(budget.requested, 51);
        assert_eq!(budget.available, 51);

        let capped = geometry_probe_budget(
            &plan(
                (0..45).map(|i| frame(i, i as f64 / 15.0)).collect(),
                Vec::new(),
            ),
            3.0,
            15.0,
            100,
            GeometryScreeningThresholds::default(),
        );
        assert_eq!(capped.available, 0);
    }

    #[test]
    fn probe_can_only_start_once_and_rejects_unusable_or_registration_drop() {
        assert!(can_start_new_probe(
            false,
            GeometryProbeStatus::NotEvaluated
        ));
        assert!(!can_start_new_probe(true, GeometryProbeStatus::Running));
        assert!(!can_start_new_probe(true, GeometryProbeStatus::Completed));

        let make_candidate = |registered, viability| ReconstructionCandidate {
            id: "candidate".into(),
            mapper: super::super::MapperBackend::Incremental,
            model_path: "model".into(),
            metrics: ReconstructionMetrics {
                registered_images: registered,
                ..ReconstructionMetrics::default()
            },
            decision: ReconstructionDecision::Warning,
            viability,
            rescue_round: 0,
        };
        let baseline = make_candidate(100, ReconstructionViability::Viable);
        assert!(!probe_candidate_acceptable(
            &baseline,
            &make_candidate(100, ReconstructionViability::NotViable),
            GeometryScreeningThresholds::default()
        ));
        assert!(!probe_candidate_acceptable(
            &baseline,
            &make_candidate(80, ReconstructionViability::Viable),
            GeometryScreeningThresholds::default()
        ));
        assert!(probe_candidate_acceptable(
            &baseline,
            &make_candidate(95, ReconstructionViability::Viable),
            GeometryScreeningThresholds::default()
        ));
    }
}
