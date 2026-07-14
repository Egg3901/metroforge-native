//! Road graph over the generated street network. Port of
//! `sim/src/core/transit/roadGraph.ts`.
//!
//! Road-running modes (bus, tram) snap stations onto streets and route their
//! tracks along the actual street path. The TS side memoizes the graph in a
//! `WeakMap` keyed by the roads array; the Rust port rebuilds the graph per
//! call (the callers are build-command cold paths, not the per-tick hot loop),
//! which keeps the API pure and avoids interior-mutability caching. Ordering is
//! deterministic: cell buckets are a `BTreeMap` keyed by the integer cell hash
//! and node lists preserve insertion order, so A* returns a stable path.

use crate::geometry::{dist, Vec2};
use crate::types::RoadEdge;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};

/// Meters between graph nodes along a road. Mirrors `NODE_SPACING`.
const NODE_SPACING: f64 = 70.0;
/// Nodes of different roads this close are connected. Mirrors `JUNCTION_RADIUS`.
const JUNCTION_RADIUS: f64 = 40.0;

/// Routable graph over the road network. Mirrors the TS `RoadGraph`.
pub struct RoadGraph {
    /// Node positions.
    pub nodes: Vec<Vec2>,
    /// Adjacency: node index -> `[(neighbor index, cost meters)]`.
    pub adj: Vec<Vec<(usize, f64)>>,
    /// Spatial hash cell key -> node indices.
    cell_of: BTreeMap<i64, Vec<usize>>,
    /// Cell size in meters.
    cell_size: f64,
}

/// Integer spatial-hash cell key. Mirrors `hashKey` (JS number arithmetic in
/// i64 to avoid overflow surprises).
fn hash_key(x: f64, y: f64, cell: f64) -> i64 {
    (x / cell).floor() as i64 * 73_856_093 + (y / cell).floor() as i64 * 19_349_663
}

/// Cell key from integer cell coordinates.
fn cell_key(cx: i64, cy: i64) -> i64 {
    cx * 73_856_093 + cy * 19_349_663
}

/// Build the road graph. Mirrors `getRoadGraph` (sans the WeakMap cache).
pub fn build_road_graph(roads: &[RoadEdge]) -> RoadGraph {
    let mut nodes: Vec<Vec2> = Vec::new();
    let mut adj: Vec<Vec<(usize, f64)>> = Vec::new();
    let cell_size = JUNCTION_RADIUS * 2.0;
    let mut cell_of: BTreeMap<i64, Vec<usize>> = BTreeMap::new();

    // sample every road polyline at ~NODE_SPACING and chain the samples
    for road in roads {
        let pl = &road.polyline;
        if pl.points.len() < 20 {
            continue;
        }
        let mut prev_idx: i64 = -1;
        let steps = ((pl.points.len() as f64 / NODE_SPACING).round() as i64).max(1);
        for s in 0..=steps {
            let d = (s as f64 / steps as f64) * pl.points.len() as f64;
            // walk cumulative
            let mut i = 1usize;
            while i < pl.cumulative.len() - 1 && pl.cumulative[i] < d {
                i += 1;
            }
            let a = pl.points[i - 1];
            let b = pl.points[i];
            let seg_start = pl.cumulative[i - 1];
            let seg_len = {
                let l = pl.cumulative[i] - seg_start;
                if l == 0.0 {
                    1.0
                } else {
                    l
                }
            };
            let t = (d - seg_start) / seg_len;
            let p = Vec2 {
                x: a.x + (b.x - a.x) * t,
                y: a.y + (b.y - a.y) * t,
            };
            let idx = nodes.len();
            nodes.push(p);
            adj.push(Vec::new());
            cell_of
                .entry(hash_key(p.x, p.y, cell_size))
                .or_default()
                .push(idx);
            if prev_idx >= 0 {
                let c = dist(nodes[prev_idx as usize], nodes[idx]);
                adj[prev_idx as usize].push((idx, c));
                adj[idx].push((prev_idx as usize, c));
            }
            prev_idx = idx as i64;
        }
    }

    // junction links: connect close nodes (crossings + snapped joins)
    for i in 0..nodes.len() {
        let p = nodes[i];
        let cx = (p.x / cell_size).floor() as i64;
        let cy = (p.y / cell_size).floor() as i64;
        for oy in -1..=1 {
            for ox in -1..=1 {
                let Some(arr) = cell_of.get(&cell_key(cx + ox, cy + oy)) else {
                    continue;
                };
                for &j in arr {
                    if j <= i {
                        continue;
                    }
                    if dist(p, nodes[j]) <= JUNCTION_RADIUS && !adj[i].iter().any(|&(n, _)| n == j)
                    {
                        let c = dist(nodes[i], nodes[j]);
                        adj[i].push((j, c));
                        adj[j].push((i, c));
                    }
                }
            }
        }
    }

    RoadGraph {
        nodes,
        adj,
        cell_of,
        cell_size,
    }
}

impl RoadGraph {
    /// Nearest graph node to `p` within `max_dist`, or `-1`. Mirrors
    /// `nearestNode`.
    fn nearest_node(&self, p: Vec2, max_dist: f64) -> i64 {
        let mut best = max_dist * max_dist;
        let mut best_i: i64 = -1;
        let r = (max_dist / self.cell_size).ceil() as i64;
        let cx = (p.x / self.cell_size).floor() as i64;
        let cy = (p.y / self.cell_size).floor() as i64;
        for oy in -r..=r {
            for ox in -r..=r {
                let Some(arr) = self.cell_of.get(&cell_key(cx + ox, cy + oy)) else {
                    continue;
                };
                for &i in arr {
                    let q = self.nodes[i];
                    let d = (q.x - p.x) * (q.x - p.x) + (q.y - p.y) * (q.y - p.y);
                    if d < best {
                        best = d;
                        best_i = i as i64;
                    }
                }
            }
        }
        best_i
    }
}

/// Nearest point ON the road network to `p` within `max_dist`. Mirrors
/// `nearestRoadPoint`.
pub fn nearest_road_point(roads: &[RoadEdge], p: Vec2, max_dist: f64) -> Option<Vec2> {
    let g = build_road_graph(roads);
    let mut best = max_dist * max_dist;
    let mut best_p: Option<Vec2> = None;
    let r = (max_dist / g.cell_size).ceil() as i64;
    let cx = (p.x / g.cell_size).floor() as i64;
    let cy = (p.y / g.cell_size).floor() as i64;
    for oy in -r..=r {
        for ox in -r..=r {
            let Some(arr) = g.cell_of.get(&cell_key(cx + ox, cy + oy)) else {
                continue;
            };
            for &i in arr {
                let q = g.nodes[i];
                let d = (q.x - p.x) * (q.x - p.x) + (q.y - p.y) * (q.y - p.y);
                if d < best {
                    best = d;
                    best_p = Some(q);
                }
            }
        }
    }
    best_p
}

/// A* open-set entry ordered by `(f, seq)`; `seq` (push order) breaks ties FIFO.
#[derive(Clone, Copy)]
struct OpenNode {
    i: usize,
    f: f64,
    seq: u64,
}
impl PartialEq for OpenNode {
    fn eq(&self, o: &Self) -> bool {
        self.f == o.f && self.seq == o.seq
    }
}
impl Eq for OpenNode {}
impl Ord for OpenNode {
    // Reversed so the `BinaryHeap` (max-heap) pops the LOWEST (f, seq).
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f).then_with(|| o.seq.cmp(&self.seq))
    }
}
impl PartialOrd for OpenNode {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// A* street path between two points; `None` if either end is off-network or
/// unreachable. Mirrors `findRoadPath`.
pub fn find_road_path(roads: &[RoadEdge], from: Vec2, to: Vec2) -> Option<Vec<Vec2>> {
    let g = build_road_graph(roads);
    let start = g.nearest_node(from, 300.0);
    let goal = g.nearest_node(to, 300.0);
    if start < 0 || goal < 0 {
        return None;
    }
    let (start, goal) = (start as usize, goal as usize);
    let goal_p = g.nodes[goal];

    let n = g.nodes.len();
    let mut d_score: Vec<f64> = vec![f64::INFINITY; n];
    let mut prev: Vec<i64> = vec![-1; n];
    let mut open: BinaryHeap<OpenNode> = BinaryHeap::new();
    let mut seq = 0u64;
    let mut closed = vec![false; n];

    d_score[start] = 0.0;
    open.push(OpenNode {
        i: start,
        f: 0.0,
        seq,
    });
    seq += 1;

    let mut guard = 0u32;
    let mut reached = false;
    while let Some(top) = open.pop() {
        guard += 1;
        if guard >= 60_000 {
            break;
        }
        let i = top.i;
        if i == goal {
            reached = true;
            break;
        }
        if closed[i] {
            continue;
        }
        closed[i] = true;
        let di = d_score[i];
        for &(j, c) in &g.adj[i] {
            let nd = di + c;
            if nd < d_score[j] {
                d_score[j] = nd;
                prev[j] = i as i64;
                open.push(OpenNode {
                    i: j,
                    f: nd + dist(g.nodes[j], goal_p),
                    seq,
                });
                seq += 1;
            }
        }
    }
    if !reached && d_score[goal].is_infinite() {
        return None;
    }
    if d_score[goal].is_infinite() {
        return None;
    }

    let mut path: Vec<Vec2> = Vec::new();
    let mut cur = goal;
    let mut guard2 = 0u32;
    while cur != start && guard2 < 100_000 {
        guard2 += 1;
        path.push(g.nodes[cur]);
        let p = prev[cur];
        if p < 0 {
            return None;
        }
        cur = p as usize;
    }
    path.push(g.nodes[start]);
    path.reverse();
    Some(path)
}
