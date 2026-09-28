use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashSet},
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, SplatError},
    video::{FramePlan, PlannedFrame},
};

pub const BRIDGE_TRIGGER_RATIO: f64 = 0.80;
const LOCAL_NEIGHBORS_PER_SIDE: usize = 10;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BridgeBackfillStatus {
    #[default]
    NotEvaluated,
    NotNeeded,
    NoBudget,
    Running,
    Completed,
    FailedRolledBack,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeBackfillSelection {
    pub gap_start_frame_index: u64,
    pub gap_end_frame_index: u64,
    pub selected_frame_index: u64,
    pub edge_extension: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeBackfillPlan {
    pub initial_registered_images: u64,
    pub initial_input_images: u64,
    pub initial_registration_ratio: f64,
    pub available_budget: u64,
    pub selected_frame_indices: Vec<u64>,
    pub internal_gap_count: u64,
    pub internal_bridge_count: u64,
    pub edge_extension_count: u64,
    #[serde(default)]
    pub selection_trace: Vec<BridgeBackfillSelection>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeBackfillCheckpoint {
    #[serde(default)]
    pub status: BridgeBackfillStatus,
    #[serde(default)]
    pub plan: Option<BridgeBackfillPlan>,
    #[serde(default)]
    pub initial_model: Option<String>,
    #[serde(default)]
    pub selected_model: Option<String>,
    #[serde(default)]
    pub final_registered_images: Option<u64>,
    #[serde(default)]
    pub final_points_3d: Option<u64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GapKind {
    Internal,
    Edge,
}

#[derive(Debug, Clone, Copy)]
struct Gap {
    start: u64,
    end: u64,
    kind: GapKind,
}

impl Gap {
    fn span(self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

pub fn plan_bridge_backfill(
    frame_plan: &FramePlan,
    registered_source_indices: &BTreeSet<u64>,
    initial_registered_images: u64,
) -> BridgeBackfillPlan {
    let initial_input_images = frame_plan.selected_frames.len() as u64;
    let initial_registration_ratio = if initial_input_images == 0 {
        0.0
    } else {
        initial_registered_images as f64 / initial_input_images as f64
    };
    let available_budget = frame_plan
        .rescue_max_frames
        .saturating_sub(initial_input_images);
    let mut output = BridgeBackfillPlan {
        initial_registered_images,
        initial_input_images,
        initial_registration_ratio,
        available_budget,
        ..BridgeBackfillPlan::default()
    };
    if initial_registration_ratio >= BRIDGE_TRIGGER_RATIO || available_budget == 0 {
        return output;
    }

    let mut selected = frame_plan.selected_frames.clone();
    selected.sort_by_key(|frame| frame.source_frame_index);
    let already_selected = selected
        .iter()
        .map(|frame| frame.source_frame_index)
        .collect::<HashSet<_>>();
    let candidates = frame_plan
        .candidate_frames
        .iter()
        .filter(|frame| !already_selected.contains(&frame.source_frame_index))
        .cloned()
        .collect::<Vec<_>>();
    if candidates.is_empty() || registered_source_indices.is_empty() {
        return output;
    }

    let mut gaps = initial_gaps(&selected, registered_source_indices);
    output.internal_gap_count = gaps
        .iter()
        .filter(|gap| gap.kind == GapKind::Internal)
        .count() as u64;
    let mut chosen = HashSet::new();
    while output.selected_frame_indices.len() < available_budget as usize {
        let Some((gap_index, candidate)) = largest_actionable_gap(&gaps, &candidates, &chosen)
        else {
            break;
        };
        let gap = gaps.swap_remove(gap_index);
        chosen.insert(candidate.source_frame_index);
        output
            .selected_frame_indices
            .push(candidate.source_frame_index);
        if gap.kind == GapKind::Edge {
            output.edge_extension_count += 1;
        } else {
            output.internal_bridge_count += 1;
        }
        output.selection_trace.push(BridgeBackfillSelection {
            gap_start_frame_index: gap.start,
            gap_end_frame_index: gap.end,
            selected_frame_index: candidate.source_frame_index,
            edge_extension: gap.kind == GapKind::Edge,
        });
        for child in [
            Gap {
                start: gap.start,
                end: candidate.source_frame_index,
                kind: gap.kind,
            },
            Gap {
                start: candidate.source_frame_index,
                end: gap.end,
                kind: gap.kind,
            },
        ] {
            if child.span() > 1 && has_candidate(child, &candidates, &chosen) {
                gaps.push(child);
            }
        }
    }
    output
}

fn initial_gaps(selected: &[PlannedFrame], registered: &BTreeSet<u64>) -> Vec<Gap> {
    if selected.is_empty() {
        return Vec::new();
    }
    let mut gaps = Vec::new();
    let mut run_start = None;
    for (position, frame) in selected.iter().enumerate() {
        let is_registered = registered.contains(&frame.source_frame_index);
        if !is_registered && run_start.is_none() {
            run_start = Some(position);
        }
        let run_ended = run_start.is_some() && (is_registered || position + 1 == selected.len());
        if !run_ended {
            continue;
        }
        let start_position = run_start.take().expect("run exists");
        let end_position = if is_registered {
            position - 1
        } else {
            position
        };
        let left = start_position
            .checked_sub(1)
            .and_then(|index| selected.get(index))
            .filter(|frame| registered.contains(&frame.source_frame_index));
        let right = selected
            .get(end_position + 1)
            .filter(|frame| registered.contains(&frame.source_frame_index));
        let (start, end, kind) = match (left, right) {
            (Some(left), Some(right)) => (
                left.source_frame_index,
                right.source_frame_index,
                GapKind::Internal,
            ),
            (None, Some(right)) => (
                selected[0].source_frame_index,
                right.source_frame_index,
                GapKind::Edge,
            ),
            (Some(left), None) => (
                left.source_frame_index,
                selected
                    .last()
                    .expect("selected is not empty")
                    .source_frame_index,
                GapKind::Edge,
            ),
            (None, None) => continue,
        };
        if start < end {
            gaps.push(Gap { start, end, kind });
        }
    }
    gaps
}

fn largest_actionable_gap(
    gaps: &[Gap],
    candidates: &[PlannedFrame],
    chosen: &HashSet<u64>,
) -> Option<(usize, PlannedFrame)> {
    gaps.iter()
        .enumerate()
        .filter_map(|(index, gap)| {
            midpoint_candidate(*gap, candidates, chosen).map(|c| (index, *gap, c))
        })
        .max_by(
            |(_, left_gap, left_candidate), (_, right_gap, right_candidate)| {
                gap_priority(*left_gap)
                    .cmp(&gap_priority(*right_gap))
                    .then_with(|| left_gap.span().cmp(&right_gap.span()))
                    .then_with(|| right_gap.start.cmp(&left_gap.start))
                    .then_with(|| {
                        right_candidate
                            .source_frame_index
                            .cmp(&left_candidate.source_frame_index)
                    })
            },
        )
        .map(|(index, _, candidate)| (index, candidate))
}

fn gap_priority(gap: Gap) -> u8 {
    match gap.kind {
        GapKind::Internal => 1,
        GapKind::Edge => 0,
    }
}

fn midpoint_candidate(
    gap: Gap,
    candidates: &[PlannedFrame],
    chosen: &HashSet<u64>,
) -> Option<PlannedFrame> {
    let midpoint = gap.start as f64 + gap.span() as f64 / 2.0;
    candidates
        .iter()
        .filter(|frame| {
            frame.source_frame_index > gap.start
                && frame.source_frame_index < gap.end
                && !chosen.contains(&frame.source_frame_index)
        })
        .min_by(|left, right| {
            let left_distance = (left.source_frame_index as f64 - midpoint).abs();
            let right_distance = (right.source_frame_index as f64 - midpoint).abs();
            left_distance
                .partial_cmp(&right_distance)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.source_frame_index.cmp(&right.source_frame_index))
        })
        .cloned()
}

fn has_candidate(gap: Gap, candidates: &[PlannedFrame], chosen: &HashSet<u64>) -> bool {
    candidates.iter().any(|frame| {
        frame.source_frame_index > gap.start
            && frame.source_frame_index < gap.end
            && !chosen.contains(&frame.source_frame_index)
    })
}

pub fn read_registered_source_indices(model: &Path) -> Result<BTreeSet<u64>> {
    let mut reader = BufReader::new(File::open(model.join("images.bin"))?);
    let image_count = read_u64(&mut reader)?;
    let mut indices = BTreeSet::new();
    for _ in 0..image_count {
        skip(&mut reader, 4 + 7 * 8 + 4)?;
        let name = read_c_string(&mut reader)?;
        let points = read_u64(&mut reader)?;
        skip(&mut reader, points.saturating_mul(24))?;
        if let Some(index) = source_index_from_name(&name) {
            indices.insert(index);
        }
    }
    Ok(indices)
}

fn source_index_from_name(name: &str) -> Option<u64> {
    let stem = Path::new(name).file_stem()?.to_str()?;
    stem.strip_prefix("frame_")?.parse().ok()
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
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
        .map_err(|error| SplatError::Process(format!("COLMAP image name is not UTF-8: {error}")))
}

fn skip(reader: &mut impl Read, bytes: u64) -> Result<()> {
    std::io::copy(&mut reader.take(bytes), &mut std::io::sink())?;
    Ok(())
}

pub fn write_bridge_pair_list(
    path: &Path,
    all_frame_indices: &[u64],
    added_frame_indices: &[u64],
    has_alpha: bool,
) -> Result<usize> {
    let extension = if has_alpha { "png" } else { "jpg" };
    let mut timeline = all_frame_indices.to_vec();
    timeline.sort_unstable();
    timeline.dedup();
    let added = added_frame_indices.iter().copied().collect::<HashSet<_>>();
    let mut pairs = BTreeSet::new();
    for (position, frame) in timeline.iter().enumerate() {
        if !added.contains(frame) {
            continue;
        }
        let start = position.saturating_sub(LOCAL_NEIGHBORS_PER_SIDE);
        let end = (position + LOCAL_NEIGHBORS_PER_SIDE + 1).min(timeline.len());
        for neighbor in &timeline[start..end] {
            if neighbor == frame {
                continue;
            }
            let ordered = if frame < neighbor {
                (*frame, *neighbor)
            } else {
                (*neighbor, *frame)
            };
            pairs.insert(ordered);
        }
    }
    let text = pairs
        .iter()
        .map(|(left, right)| format!("frame_{left:010}.{extension} frame_{right:010}.{extension}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, if text.is_empty() { text } else { text + "\n" })?;
    Ok(pairs.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(index: u64) -> PlannedFrame {
        PlannedFrame {
            source_frame_index: index,
            timestamp_seconds: index as f64,
        }
    }

    fn plan(selected: &[u64], candidates: &[u64], max: u64) -> FramePlan {
        FramePlan {
            selected_frames: selected.iter().copied().map(frame).collect(),
            candidate_frames: candidates.iter().copied().map(frame).collect(),
            rescue_max_frames: max,
            estimated_frames: selected.len() as u64,
            ..FramePlan::default()
        }
    }

    #[test]
    fn eighty_percent_does_not_need_bridge() {
        let plan = plan(&[0, 10, 20, 30, 40], &[0, 5, 10, 15, 20, 25, 30, 35, 40], 9);
        let report = plan_bridge_backfill(&plan, &[0, 10, 20, 30].into_iter().collect(), 4);
        assert!(report.selected_frame_indices.is_empty());
    }

    #[test]
    fn exhausted_quality_budget_cannot_add_frames() {
        let plan = plan(&[0, 10, 20, 30], &[0, 10, 20, 30], 4);
        let report = plan_bridge_backfill(&plan, &[0, 30].into_iter().collect(), 2);
        assert_eq!(report.available_budget, 0);
        assert!(report.selected_frame_indices.is_empty());
    }

    #[test]
    fn bridge_never_selects_an_existing_frame_or_exceeds_budget() {
        let plan = plan(
            &[0, 20, 40, 60, 80, 100],
            &(0..=100).step_by(5).collect::<Vec<_>>(),
            9,
        );
        let report = plan_bridge_backfill(&plan, &[0, 100].into_iter().collect(), 2);
        assert_eq!(report.selected_frame_indices.len(), 3);
        let existing = [0, 20, 40, 60, 80, 100].into_iter().collect::<HashSet<_>>();
        assert!(report
            .selected_frame_indices
            .iter()
            .all(|index| !existing.contains(index)));
        assert_eq!(
            report
                .selected_frame_indices
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .len(),
            report.selected_frame_indices.len()
        );
    }

    #[test]
    fn longest_internal_gap_is_bisected_before_smaller_gap() {
        let plan = plan(
            &[0, 20, 40, 60, 80, 100],
            &(0..=100).step_by(10).collect::<Vec<_>>(),
            8,
        );
        let registered = [0, 20, 80, 100].into_iter().collect();
        let report = plan_bridge_backfill(&plan, &registered, 4);
        assert_eq!(report.selected_frame_indices, vec![50, 30]);
        assert_eq!(report.internal_gap_count, 1);
        assert_eq!(report.internal_bridge_count, 2);
        assert_eq!(report.selection_trace[0].gap_start_frame_index, 20);
        assert_eq!(report.selection_trace[0].gap_end_frame_index, 80);
        assert_eq!(report.selection_trace[0].selected_frame_index, 50);
    }

    #[test]
    fn internal_gaps_have_priority_over_edge_extension() {
        let plan = plan(
            &[0, 20, 40, 60, 80, 100],
            &(0..=100).step_by(10).collect::<Vec<_>>(),
            7,
        );
        let registered = [20, 40, 80].into_iter().collect();
        let report = plan_bridge_backfill(&plan, &registered, 3);
        assert_eq!(report.selected_frame_indices, vec![50]);
        assert_eq!(report.edge_extension_count, 0);
    }

    #[test]
    fn edge_extension_uses_midpoint_when_no_internal_gap_remains() {
        let plan = plan(
            &[0, 20, 40, 60, 80, 100],
            &(0..=100).step_by(10).collect::<Vec<_>>(),
            8,
        );
        let registered = [40, 60].into_iter().collect();
        let report = plan_bridge_backfill(&plan, &registered, 2);
        assert_eq!(report.selected_frame_indices, vec![10, 70]);
        assert_eq!(report.edge_extension_count, 2);
    }

    #[test]
    fn pair_list_connects_added_frame_on_both_sides() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("pairs.txt");
        let count = write_bridge_pair_list(&path, &[0, 10, 20, 30, 40], &[20], false).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(count >= 2);
        assert!(text.contains("frame_0000000010.jpg frame_0000000020.jpg"));
        assert!(text.contains("frame_0000000020.jpg frame_0000000030.jpg"));
    }
}
