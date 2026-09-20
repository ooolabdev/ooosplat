use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
};

use rusqlite::{Connection, OpenFlags};

use crate::error::{Result, SplatError};

use super::{Bottleneck, ViewGraphReport, WeakRegion};

const MAX_IMAGE_ID: i64 = 2_147_483_647;

#[derive(Debug, Clone, Copy)]
struct Edge {
    a: u32,
    b: u32,
    inliers: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ViewGraphAnalyzer;

impl ViewGraphAnalyzer {
    pub fn analyze_database(&self, database: &Path) -> Result<ViewGraphReport> {
        let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| {
                SplatError::Process(format!("Unable to read COLMAP database: {error}"))
            })?;
        let mut images = Vec::new();
        {
            let mut statement = connection
                .prepare("SELECT image_id FROM images ORDER BY image_id")
                .map_err(sql_error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, u32>(0))
                .map_err(sql_error)?;
            for row in rows {
                images.push(row.map_err(sql_error)?);
            }
        }
        let image_set: HashSet<u32> = images.iter().copied().collect();
        let mut edges = Vec::new();
        {
            let mut statement = connection
                .prepare("SELECT pair_id, rows FROM two_view_geometries WHERE rows > 0")
                .map_err(sql_error)?;
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, u32>(1)?)))
                .map_err(sql_error)?;
            for row in rows {
                let (pair_id, inliers) = row.map_err(sql_error)?;
                let b = (pair_id % MAX_IMAGE_ID) as u32;
                let a = ((pair_id - b as i64) / MAX_IMAGE_ID) as u32;
                if a != b && image_set.contains(&a) && image_set.contains(&b) {
                    edges.push(Edge { a, b, inliers });
                }
            }
        }
        Ok(analyze(&images, &edges))
    }
}

fn sql_error(error: rusqlite::Error) -> SplatError {
    SplatError::Process(format!("Unable to query COLMAP view graph: {error}"))
}

fn analyze(images: &[u32], edges: &[Edge]) -> ViewGraphReport {
    let image_count = images.len();
    if image_count == 0 {
        return ViewGraphReport::default();
    }
    let mut adjacency: HashMap<u32, Vec<u32>> = images.iter().map(|id| (*id, Vec::new())).collect();
    for edge in edges {
        adjacency.entry(edge.a).or_default().push(edge.b);
        adjacency.entry(edge.b).or_default().push(edge.a);
    }
    let degrees: HashMap<u32, usize> = adjacency
        .iter()
        .map(|(id, values)| (*id, values.len()))
        .collect();
    let connected_images = degrees.values().filter(|degree| **degree > 0).count();
    let mut visited = HashSet::new();
    let mut component_sizes = Vec::new();
    for image in images {
        if !visited.insert(*image) {
            continue;
        }
        let mut queue = VecDeque::from([*image]);
        let mut size = 0;
        while let Some(current) = queue.pop_front() {
            size += 1;
            for neighbor in adjacency.get(&current).into_iter().flatten() {
                if visited.insert(*neighbor) {
                    queue.push_back(*neighbor);
                }
            }
        }
        component_sizes.push(size);
    }
    let largest_component_ratio =
        component_sizes.iter().copied().max().unwrap_or(0) as f32 / image_count as f32;
    let mut sorted_degrees: Vec<usize> = degrees.values().copied().collect();
    sorted_degrees.sort_unstable();
    let median_degree = median_usize(&sorted_degrees);
    let normalized_degree = if image_count > 1 {
        median_degree / (image_count - 1) as f32
    } else {
        0.0
    };
    let two_core_ratio = two_core_size(&adjacency) as f32 / image_count as f32;
    let bridges = find_bridges(images, &adjacency);
    let bridge_ratio = if edges.is_empty() {
        1.0
    } else {
        bridges.len() as f32 / edges.len() as f32
    };
    let temporal_limit = 3_u32;
    let long_range_limit = (image_count as u32 / 10).max(5);
    let temporal_edges = edges
        .iter()
        .filter(|edge| edge.a.abs_diff(edge.b) <= temporal_limit)
        .count();
    let long_range_edges = edges
        .iter()
        .filter(|edge| edge.a.abs_diff(edge.b) >= long_range_limit)
        .count();
    let edge_count = edges.len().max(1) as f32;
    let mut inliers: Vec<u32> = edges.iter().map(|edge| edge.inliers).collect();
    inliers.sort_unstable();
    let weak_runs = weak_runs(images, &degrees);
    let bottlenecks = bridges
        .iter()
        .take(64)
        .map(|(a, b)| Bottleneck {
            image_id_a: *a,
            image_id_b: *b,
            severity: 1.0,
        })
        .collect();
    ViewGraphReport {
        image_count,
        connected_images,
        connected_components: component_sizes.len(),
        largest_component_ratio,
        median_degree,
        normalized_degree,
        two_core_ratio,
        bridge_ratio,
        temporal_edge_ratio: temporal_edges as f32 / edge_count,
        long_range_edge_ratio: long_range_edges as f32 / edge_count,
        median_inliers: median_u32(&inliers),
        weak_runs,
        bottlenecks,
    }
}

fn two_core_size(adjacency: &HashMap<u32, Vec<u32>>) -> usize {
    let mut degree: HashMap<u32, usize> = adjacency
        .iter()
        .map(|(id, values)| (*id, values.len()))
        .collect();
    let mut queue: VecDeque<u32> = degree
        .iter()
        .filter_map(|(id, value)| (*value < 2).then_some(*id))
        .collect();
    let mut removed = HashSet::new();
    while let Some(node) = queue.pop_front() {
        if !removed.insert(node) {
            continue;
        }
        for neighbor in adjacency.get(&node).into_iter().flatten() {
            if !removed.contains(neighbor) {
                let value = degree.entry(*neighbor).or_default();
                *value = value.saturating_sub(1);
                if *value < 2 {
                    queue.push_back(*neighbor);
                }
            }
        }
    }
    adjacency.len().saturating_sub(removed.len())
}

fn find_bridges(images: &[u32], adjacency: &HashMap<u32, Vec<u32>>) -> Vec<(u32, u32)> {
    struct Search<'a> {
        adjacency: &'a HashMap<u32, Vec<u32>>,
        time: u32,
        discovered: HashMap<u32, u32>,
        low: HashMap<u32, u32>,
        bridges: Vec<(u32, u32)>,
    }
    impl Search<'_> {
        fn visit(&mut self, node: u32, parent: Option<u32>) {
            self.time += 1;
            self.discovered.insert(node, self.time);
            self.low.insert(node, self.time);
            for neighbor in self.adjacency.get(&node).into_iter().flatten().copied() {
                if Some(neighbor) == parent {
                    continue;
                }
                if !self.discovered.contains_key(&neighbor) {
                    self.visit(neighbor, Some(node));
                    let low_neighbor = self.low[&neighbor];
                    let low_node = self.low[&node].min(low_neighbor);
                    self.low.insert(node, low_node);
                    if low_neighbor > self.discovered[&node] {
                        self.bridges.push((node.min(neighbor), node.max(neighbor)));
                    }
                } else {
                    let low_node = self.low[&node].min(self.discovered[&neighbor]);
                    self.low.insert(node, low_node);
                }
            }
        }
    }
    let mut search = Search {
        adjacency,
        time: 0,
        discovered: HashMap::new(),
        low: HashMap::new(),
        bridges: Vec::new(),
    };
    for image in images {
        if !search.discovered.contains_key(image) {
            search.visit(*image, None);
        }
    }
    search.bridges
}

fn weak_runs(images: &[u32], degrees: &HashMap<u32, usize>) -> Vec<WeakRegion> {
    let mut output = Vec::new();
    let mut start = None;
    let mut last = 0;
    for image in images {
        if degrees.get(image).copied().unwrap_or(0) < 2 {
            start.get_or_insert(*image);
            last = *image;
        } else if let Some(first) = start.take() {
            output.push(WeakRegion {
                start_image_id: first,
                end_image_id: last,
                severity: 1.0,
            });
        }
    }
    if let Some(first) = start {
        output.push(WeakRegion {
            start_image_id: first,
            end_image_id: last,
            severity: 1.0,
        });
    }
    output
}

fn median_usize(values: &[usize]) -> f32 {
    values.get(values.len() / 2).copied().unwrap_or(0) as f32
}

fn median_u32(values: &[u32]) -> f32 {
    values.get(values.len() / 2).copied().unwrap_or(0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_dense_chain_and_fragmented_graphs() {
        let images: Vec<u32> = (1..=6).collect();
        let chain: Vec<Edge> = (1..6)
            .map(|id| Edge {
                a: id,
                b: id + 1,
                inliers: 80,
            })
            .collect();
        let chain = analyze(&images, &chain);
        assert_eq!(chain.connected_components, 1);
        assert!(chain.bridge_ratio > 0.9);
        let fragmented = analyze(
            &images,
            &[Edge {
                a: 1,
                b: 2,
                inliers: 20,
            }],
        );
        assert!(fragmented.connected_components > 1);
        assert!(fragmented.largest_component_ratio < 0.5);
    }

    #[test]
    fn reads_geometrically_verified_pairs_from_colmap_schema() {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("database.db");
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch(
            "CREATE TABLE images(image_id INTEGER PRIMARY KEY, name TEXT, camera_id INTEGER);\
             CREATE TABLE two_view_geometries(pair_id INTEGER PRIMARY KEY, rows INTEGER, cols INTEGER, data BLOB, config INTEGER, F BLOB, E BLOB, H BLOB, qvec BLOB, tvec BLOB);\
             INSERT INTO images VALUES(1, 'a.jpg', 1);\
             INSERT INTO images VALUES(2, 'b.jpg', 1);\
             INSERT INTO images VALUES(3, 'c.jpg', 1);",
        ).unwrap();
        let pair_id = MAX_IMAGE_ID + 2;
        connection
            .execute(
                "INSERT INTO two_view_geometries(pair_id, rows, cols, config) VALUES(?1, 42, 2, 2)",
                [pair_id],
            )
            .unwrap();
        drop(connection);
        let report = ViewGraphAnalyzer.analyze_database(&database).unwrap();
        assert_eq!(report.image_count, 3);
        assert_eq!(report.connected_images, 2);
        assert_eq!(report.median_inliers, 42.0);
    }
}
