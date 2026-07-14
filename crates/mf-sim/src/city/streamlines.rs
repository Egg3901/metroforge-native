//! Evenly-spaced streamline tracing over a tensor field. Port of
//! `sim/src/core/city/streamlines.ts`.
//!
//! Jobard-Lefer seeding + Chen et al. hyperstreamlines: streamlines become
//! streets; separation distance controls road-class density. Deterministic:
//! all randomness comes from the caller's [`Rng`].

use crate::city::tensor::{step_direction, TensorField};
use crate::geometry::Vec2;
use crate::rng::Rng;
use std::collections::HashMap;

const STEP: f64 = 40.0; // meters per integration step

/// Minimum spacing between parallel streamlines of a class. Mirrors the
/// `number | (p) => number` union.
pub enum Separation<'a> {
    /// Constant separation.
    Const(f64),
    /// Position-dependent separation.
    Varying(Box<dyn Fn(Vec2) -> f64 + 'a>),
}

impl Separation<'_> {
    fn at(&self, p: Vec2) -> f64 {
        match self {
            Separation::Const(v) => *v,
            Separation::Varying(f) => f(p),
        }
    }
}

/// Options controlling a trace pass. Mirrors `TraceOptions`.
pub struct TraceOptions<'a> {
    /// Minimum spacing between parallel streamlines.
    pub separation: Separation<'a>,
    /// Stop when leaving this predicate (e.g. populated land).
    pub in_domain: Box<dyn Fn(Vec2) -> bool + 'a>,
    /// Allow jumping short forbidden spans (bridges); 0 disables.
    pub bridge_max_steps: u32,
    /// Water/blocked test used for bridging.
    pub blocked: Box<dyn Fn(Vec2) -> bool + 'a>,
    /// Maximum streamline length.
    pub max_length: f64,
    /// Minimum accepted streamline length.
    pub min_length: f64,
    /// Seeds to start from, in priority order.
    pub seeds: Vec<Vec2>,
    /// Existing road sample points new lines may snap onto.
    pub snap_targets: Vec<Vec2>,
    /// Spawn extra seeds along each accepted streamline.
    pub spawn_seeds: bool,
    /// Eigen directions to trace (0 = major, 1 = minor).
    pub eigen_dirs: Vec<u8>,
}

/// Uniform grid of accepted sample points for separation tests. Mirrors
/// `SeparationGrid`.
struct SeparationGrid {
    cell: f64,
    map: HashMap<i64, Vec<Vec2>>,
}

impl SeparationGrid {
    fn new(cell: f64) -> Self {
        Self {
            cell,
            map: HashMap::new(),
        }
    }

    #[inline]
    fn key(&self, x: f64, y: f64) -> i64 {
        (x / self.cell).floor() as i64 * 73_856_093 + (y / self.cell).floor() as i64 * 19_349_663
    }

    fn add(&mut self, p: Vec2) {
        let k = self.key(p.x, p.y);
        self.map.entry(k).or_default().push(p);
    }

    fn nearest_point(&self, p: Vec2, radius: f64) -> Option<Vec2> {
        let mut best = radius * radius;
        let mut best_q: Option<Vec2> = None;
        let r = (radius / self.cell).ceil() as i64;
        let cx = (p.x / self.cell).floor() as i64;
        let cy = (p.y / self.cell).floor() as i64;
        for oy in -r..=r {
            for ox in -r..=r {
                let k = (cx + ox) * 73_856_093 + (cy + oy) * 19_349_663;
                if let Some(arr) = self.map.get(&k) {
                    for q in arr {
                        let d = (q.x - p.x) * (q.x - p.x) + (q.y - p.y) * (q.y - p.y);
                        if d < best {
                            best = d;
                            best_q = Some(*q);
                        }
                    }
                }
            }
        }
        best_q
    }

    fn nearest(&self, p: Vec2, radius: f64) -> f64 {
        let mut best = f64::INFINITY;
        let r = (radius / self.cell).ceil() as i64;
        let cx = (p.x / self.cell).floor() as i64;
        let cy = (p.y / self.cell).floor() as i64;
        for oy in -r..=r {
            for ox in -r..=r {
                let k = (cx + ox) * 73_856_093 + (cy + oy) * 19_349_663;
                if let Some(arr) = self.map.get(&k) {
                    for q in arr {
                        let d = (q.x - p.x) * (q.x - p.x) + (q.y - p.y) * (q.y - p.y);
                        if d < best {
                            best = d;
                        }
                    }
                }
            }
        }
        best.sqrt()
    }
}

/// Trace evenly-spaced streamlines over `field`. Mirrors `traceStreamlines`.
pub fn trace_streamlines(field: &TensorField, mut rng: Rng, opts: TraceOptions) -> Vec<Vec<Vec2>> {
    let mut min_sep = f64::INFINITY;
    match &opts.separation {
        Separation::Const(v) => min_sep = *v,
        Separation::Varying(_) => {
            for s in &opts.seeds {
                min_sep = min_sep.min(opts.separation.at(*s));
            }
            if !min_sep.is_finite() {
                min_sep = 100.0;
            }
        }
    }

    // independent separation grids per eigen family (indices 0 and 1)
    let cell = (min_sep / 2.0).max(40.0);
    let mut grids: [SeparationGrid; 2] = [SeparationGrid::new(cell), SeparationGrid::new(cell)];
    let mut results: Vec<Vec<Vec2>> = Vec::new();
    let mut snap_grid = SeparationGrid::new(cell);
    for t in &opts.snap_targets {
        snap_grid.add(*t);
    }
    let mut queue: Vec<Vec2> = opts.seeds.clone();

    let mut guard = 0u32;
    while !queue.is_empty() && {
        guard += 1;
        guard < 4000
    } {
        // random-ish pop keeps growth spatially balanced
        let idx = if queue.len() > 4 {
            rng.int(0, queue.len() as i64 - 1) as usize
        } else {
            0
        };
        let seed = queue.remove(idx);
        for &eigen in &opts.eigen_dirs {
            let ei = eigen as usize;
            let line = trace_one(field, &opts, &grids[ei], &snap_grid, seed, eigen);
            let Some(line) = line else { continue };
            let sep = opts.separation.at(seed);
            for p in &line {
                grids[ei].add(*p);
                snap_grid.add(*p);
            }
            if opts.spawn_seeds && line.len() >= 9 {
                // Jobard-Lefer: candidate seeds offset perpendicular to the line
                let mut i = 4usize;
                while i < line.len() - 4 {
                    let a = line[i - 1];
                    let b = line[i + 1];
                    let tx = b.x - a.x;
                    let ty = b.y - a.y;
                    let tl = (tx * tx + ty * ty).sqrt().max(1e-12);
                    let tl = if tl == 0.0 { 1.0 } else { tl };
                    for side in [1.0, -1.0] {
                        let cand = Vec2 {
                            x: line[i].x + (-ty / tl) * sep * side,
                            y: line[i].y + (tx / tl) * sep * side,
                        };
                        if (opts.in_domain)(cand) && !(opts.blocked)(cand) {
                            queue.push(cand);
                        }
                    }
                    i += 4;
                }
            }
            results.push(line);
        }
    }
    results
}

#[allow(clippy::too_many_arguments)]
fn trace_one(
    field: &TensorField,
    opts: &TraceOptions,
    grid: &SeparationGrid,
    snap_grid: &SeparationGrid,
    seed: Vec2,
    eigen: u8,
) -> Option<Vec<Vec2>> {
    let sep = opts.separation.at(seed);
    if grid.nearest(seed, sep) < sep * 0.9 {
        return None;
    }
    if !(opts.in_domain)(seed) {
        return None;
    }

    // trace both ways from the seed and stitch
    let mut halves: Vec<Vec<Vec2>> = Vec::new();
    for flip in [1.0, -1.0] {
        let mut pts: Vec<Vec2> = Vec::new();
        let mut p = seed;
        let mut prev: Option<Vec2> = None;
        let mut bridging = 0u32;
        let mut len = 0.0;
        while len < opts.max_length / 2.0 {
            let mut dir = step_direction(field, p, eigen, prev);
            if prev.is_none() {
                dir = Vec2 {
                    x: dir.x * flip,
                    y: dir.y * flip,
                };
            }
            let next = Vec2 {
                x: p.x + dir.x * STEP,
                y: p.y + dir.y * STEP,
            };
            if (opts.blocked)(next) {
                if let (true, Some(pv)) = (
                    opts.bridge_max_steps > 0 && bridging < opts.bridge_max_steps,
                    prev,
                ) {
                    bridging += 1;
                    p = Vec2 {
                        x: p.x + pv.x * STEP,
                        y: p.y + pv.y * STEP,
                    };
                    pts.push(p);
                    len += STEP;
                    continue;
                }
                break;
            }
            if bridging > 0 && !(opts.blocked)(next) {
                bridging = 0;
            }
            if !(opts.in_domain)(next) {
                break;
            }
            // stop when crowding an existing streamline of the SAME family
            let d_near = grid.nearest(next, sep);
            if pts.len() > 2 && d_near < sep * 0.55 {
                pts.push(next);
                break;
            }
            pts.push(next);
            prev = Some(dir);
            p = next;
            len += STEP;
        }
        // trim a failed bridge: never end a street in the water
        while pts.last().is_some_and(|q| (opts.blocked)(*q)) {
            pts.pop();
        }
        halves.push(pts);
    }
    let back = &mut halves[1];
    back.reverse();
    let mut line: Vec<Vec2> = Vec::with_capacity(halves[0].len() + halves[1].len() + 1);
    line.extend_from_slice(&halves[1]);
    line.push(seed);
    line.extend_from_slice(&halves[0]);
    if (line.len() as f64) * STEP < opts.min_length {
        return None;
    }
    // connect: snap both ends onto the nearest existing road point
    let sep_end = opts.separation.at(seed);
    let last = line.len() - 1;
    for end_idx in [0usize, last] {
        let end = line[end_idx];
        if let Some(q) = snap_grid.nearest_point(end, sep_end * 1.3) {
            if ((q.x - end.x).powi(2) + (q.y - end.y).powi(2)).sqrt() > 8.0 && !(opts.blocked)(q) {
                if end_idx == 0 {
                    line.insert(0, q);
                } else {
                    line.push(q);
                }
            }
        }
    }
    Some(line)
}
