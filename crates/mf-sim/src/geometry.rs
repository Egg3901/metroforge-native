//! Continuous 2D geometry. Port of `sim/src/core/geometry.ts`.
//!
//! World units are METERS. No grid geometry here (scalar fields live in the
//! field grid); everything spatial is vectors and polylines. These are pure,
//! deterministic functions used across worldgen (P2) and the transit/economy
//! systems (P3). Polylines store their cumulative segment lengths (computed
//! once, never recomputed) as a determinism policy, mirroring the TS source.

use crate::rng::Rng;

/// A 2D point / vector in world meters. Mirrors `Vec2` in geometry.ts.
///
/// Distinct from `mf_protocol::Vec2` (the wire DTO); this is the sim-internal
/// type. A P4 bridge converts between them field-for-field (`x`, `y`).
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vec2 {
    /// World X coordinate (meters).
    pub x: f64,
    /// World Y coordinate (meters).
    pub y: f64,
}

/// Construct a [`Vec2`]. Mirrors `vec(x, y)`.
#[inline]
pub fn vec(x: f64, y: f64) -> Vec2 {
    Vec2 { x, y }
}

/// Euclidean distance between two points. Mirrors `dist`.
#[inline]
pub fn dist(a: Vec2, b: Vec2) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

/// Squared Euclidean distance (avoids the sqrt). Mirrors `distSq`.
#[inline]
pub fn dist_sq(a: Vec2, b: Vec2) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    dx * dx + dy * dy
}

/// Scalar linear interpolation. Mirrors `lerp`.
#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Vector linear interpolation. Mirrors `lerpVec`.
#[inline]
pub fn lerp_vec(a: Vec2, b: Vec2, t: f64) -> Vec2 {
    Vec2 {
        x: lerp(a.x, b.x, t),
        y: lerp(a.y, b.y, t),
    }
}

/// Clamp `v` into `[lo, hi]`. Mirrors `clamp`.
#[inline]
pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Polyline with precomputed cumulative lengths (stored, not recomputed — a
/// determinism policy). Mirrors the `Polyline` interface.
#[derive(Clone, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Polyline {
    /// Ordered points.
    pub points: Vec<Vec2>,
    /// `cumulative[i]` = distance from `points[0]` to `points[i]`;
    /// `cumulative[0] = 0`.
    pub cumulative: Vec<f64>,
    /// Total polyline length.
    pub length: f64,
}

/// Build a [`Polyline`] from its points, precomputing cumulative lengths.
/// Mirrors `makePolyline`.
pub fn make_polyline(points: Vec<Vec2>) -> Polyline {
    let mut cumulative = vec![0.0];
    let mut total = 0.0;
    for i in 1..points.len() {
        total += dist(points[i - 1], points[i]);
        cumulative.push(total);
    }
    Polyline {
        points,
        cumulative,
        length: total,
    }
}

/// Position + heading (radians) at distance `d` along the polyline (clamped to
/// `[0, length]`). Mirrors `pointAlong`.
pub fn point_along(pl: &Polyline, d: f64) -> (Vec2, f64) {
    let pts = &pl.points;
    if pts.is_empty() {
        return (vec(0.0, 0.0), 0.0);
    }
    if pts.len() == 1 || d <= 0.0 {
        let p0 = pts[0];
        let p1 = pts[pts.len().min(2) - 1];
        return (p0, (p1.y - p0.y).atan2(p1.x - p0.x));
    }
    if d >= pl.length {
        let pa = pts[pts.len() - 2];
        let pb = pts[pts.len() - 1];
        return (pb, (pb.y - pa.y).atan2(pb.x - pa.x));
    }
    // binary search the segment
    let mut lo = 0usize;
    let mut hi = pl.cumulative.len() - 1;
    while lo < hi - 1 {
        let mid = (lo + hi) >> 1;
        if pl.cumulative[mid] <= d {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let seg_start = pl.cumulative[lo];
    let seg_len = pl.cumulative[hi] - seg_start;
    let t = if seg_len > 0.0 {
        (d - seg_start) / seg_len
    } else {
        0.0
    };
    let a = pts[lo];
    let b = pts[hi];
    (lerp_vec(a, b, t), (b.y - a.y).atan2(b.x - a.x))
}

/// Closest point on segment `ab` to `p`. Returns `(t, dsq, pos)` where `t` is
/// in `[0, 1]` and `dsq` is the squared distance. Mirrors `closestOnSegment`.
pub fn closest_on_segment(p: Vec2, a: Vec2, b: Vec2) -> (f64, f64, Vec2) {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len_sq = abx * abx + aby * aby;
    let t = if len_sq > 0.0 {
        clamp(((p.x - a.x) * abx + (p.y - a.y) * aby) / len_sq, 0.0, 1.0)
    } else {
        0.0
    };
    let pos = Vec2 {
        x: a.x + abx * t,
        y: a.y + aby * t,
    };
    (t, dist_sq(p, pos), pos)
}

/// Closest point on a polyline to `p`. Returns `(along, dsq, pos)` where
/// `along` is the distance along the polyline. Mirrors `closestOnPolyline`.
pub fn closest_on_polyline(pl: &Polyline, p: Vec2) -> (f64, f64, Vec2) {
    let mut best_along = 0.0;
    let mut best_dsq = f64::INFINITY;
    let mut best_pos = pl.points.first().copied().unwrap_or(vec(0.0, 0.0));
    for i in 1..pl.points.len() {
        let a = pl.points[i - 1];
        let b = pl.points[i];
        let (t, dsq, pos) = closest_on_segment(p, a, b);
        if dsq < best_dsq {
            let seg_start = pl.cumulative[i - 1];
            let seg_len = pl.cumulative[i] - seg_start;
            best_along = seg_start + seg_len * t;
            best_dsq = dsq;
            best_pos = pos;
        }
    }
    (best_along, best_dsq, best_pos)
}

/// Uniform-bucket spatial hash for point lookups (stations, nodes). Port of the
/// `SpatialHash<T>` class. This is a transient query acceleration structure, not
/// hashed state; `query_radius` iterates a deterministic cell range so results
/// do not depend on internal bucket iteration order.
pub struct SpatialHash<T, F: Fn(&T) -> Vec2> {
    cell_size: f64,
    get_pos: F,
    buckets: std::collections::HashMap<i64, Vec<T>>,
}

impl<T, F: Fn(&T) -> Vec2> SpatialHash<T, F> {
    /// New spatial hash with the given cell size and position accessor.
    pub fn new(cell_size: f64, get_pos: F) -> Self {
        Self {
            cell_size,
            get_pos,
            buckets: std::collections::HashMap::new(),
        }
    }

    #[inline]
    fn key(&self, x: f64, y: f64) -> i64 {
        let cx = (x / self.cell_size).floor() as i64;
        let cy = (y / self.cell_size).floor() as i64;
        cx.wrapping_mul(73_856_093)
            .wrapping_add(cy.wrapping_mul(19_349_663))
    }

    /// Insert an item. Mirrors `insert`.
    pub fn insert(&mut self, item: T) {
        let p = (self.get_pos)(&item);
        let k = self.key(p.x, p.y);
        self.buckets.entry(k).or_default().push(item);
    }

    /// All items within radius `r` of `p` (exact, post-filtered). Mirrors
    /// `queryRadius`. `T: Clone` because Rust cannot hand out borrows spanning
    /// the nested bucket iteration; callers store cheap ids/handles.
    pub fn query_radius(&self, p: Vec2, r: f64) -> Vec<T>
    where
        T: Clone,
    {
        let mut out = Vec::new();
        let r_sq = r * r;
        let min_cx = ((p.x - r) / self.cell_size).floor() as i64;
        let max_cx = ((p.x + r) / self.cell_size).floor() as i64;
        let min_cy = ((p.y - r) / self.cell_size).floor() as i64;
        let max_cy = ((p.y + r) / self.cell_size).floor() as i64;
        for cx in min_cx..=max_cx {
            for cy in min_cy..=max_cy {
                let k = cx
                    .wrapping_mul(73_856_093)
                    .wrapping_add(cy.wrapping_mul(19_349_663));
                if let Some(arr) = self.buckets.get(&k) {
                    for item in arr {
                        if dist_sq((self.get_pos)(item), p) <= r_sq {
                            out.push(item.clone());
                        }
                    }
                }
            }
        }
        out
    }

    /// Rebuild from a set of items. Mirrors `rebuild`.
    pub fn rebuild(&mut self, items: impl IntoIterator<Item = T>) {
        self.buckets.clear();
        for item in items {
            self.insert(item);
        }
    }
}

/// Deterministic 2D value noise with fBm, used by the city generator (P2).
/// Port of the `Noise2D` class.
pub struct Noise2D {
    perm: [u8; 512],
}

impl Noise2D {
    /// Build a permutation table by shuffling `0..256` with the given RNG.
    /// Mirrors the TS constructor (which takes a `rngNextUint` closure).
    pub fn new(rng: &mut Rng) -> Self {
        let mut p = [0u8; 256];
        for (i, slot) in p.iter_mut().enumerate() {
            *slot = i as u8;
        }
        for i in (1..256usize).rev() {
            let j = (rng.next_uint() % (i as u32 + 1)) as usize;
            p.swap(i, j);
        }
        let mut perm = [0u8; 512];
        for (i, slot) in perm.iter_mut().enumerate() {
            *slot = p[i & 255];
        }
        Self { perm }
    }

    #[inline]
    fn hash(&self, x: i64, y: i64) -> f64 {
        let xi = (x & 255) as usize;
        let idx = (self.perm[xi] as i64 + y) & 255;
        self.perm[idx as usize] as f64 / 255.0
    }

    /// Smooth value noise in `[0, 1]`. Mirrors `at`.
    pub fn at(&self, x: f64, y: f64) -> f64 {
        let xi = x.floor() as i64;
        let yi = y.floor() as i64;
        let xf = x - xi as f64;
        let yf = y - yi as f64;
        let u = xf * xf * (3.0 - 2.0 * xf);
        let v = yf * yf * (3.0 - 2.0 * yf);
        let n00 = self.hash(xi, yi);
        let n10 = self.hash(xi + 1, yi);
        let n01 = self.hash(xi, yi + 1);
        let n11 = self.hash(xi + 1, yi + 1);
        lerp(lerp(n00, n10, u), lerp(n01, n11, u), v)
    }

    /// Fractal Brownian motion, output roughly `[0, 1]`. Mirrors `fbm`.
    pub fn fbm(&self, x: f64, y: f64, octaves: u32, lacunarity: f64, gain: f64) -> f64 {
        let mut amp = 0.5;
        let mut freq = 1.0;
        let mut sum = 0.0;
        let mut norm = 0.0;
        for _ in 0..octaves {
            sum += amp * self.at(x * freq, y * freq);
            norm += amp;
            amp *= gain;
            freq *= lacunarity;
        }
        sum / norm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dist_and_distsq() {
        let a = vec(0.0, 0.0);
        let b = vec(3.0, 4.0);
        assert_eq!(dist(a, b), 5.0);
        assert_eq!(dist_sq(a, b), 25.0);
    }

    #[test]
    fn lerp_endpoints_and_mid() {
        assert_eq!(lerp(0.0, 10.0, 0.0), 0.0);
        assert_eq!(lerp(0.0, 10.0, 1.0), 10.0);
        assert_eq!(lerp(0.0, 10.0, 0.5), 5.0);
    }

    #[test]
    fn clamp_bounds() {
        assert_eq!(clamp(-1.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp(2.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp(0.5, 0.0, 1.0), 0.5);
    }

    #[test]
    fn polyline_lengths() {
        let pl = make_polyline(vec![vec(0.0, 0.0), vec(3.0, 4.0), vec(3.0, 4.0 + 5.0)]);
        assert_eq!(pl.cumulative, vec![0.0, 5.0, 10.0]);
        assert_eq!(pl.length, 10.0);
    }

    #[test]
    fn point_along_midpoint() {
        let pl = make_polyline(vec![vec(0.0, 0.0), vec(10.0, 0.0)]);
        let (pos, heading) = point_along(&pl, 5.0);
        assert_eq!(pos, vec(5.0, 0.0));
        assert_eq!(heading, 0.0);
    }

    #[test]
    fn point_along_clamps_past_end() {
        let pl = make_polyline(vec![vec(0.0, 0.0), vec(10.0, 0.0)]);
        let (pos, _) = point_along(&pl, 999.0);
        assert_eq!(pos, vec(10.0, 0.0));
    }

    #[test]
    fn closest_on_segment_projects() {
        // point above the middle of a horizontal segment
        let (t, dsq, pos) = closest_on_segment(vec(5.0, 3.0), vec(0.0, 0.0), vec(10.0, 0.0));
        assert_eq!(t, 0.5);
        assert_eq!(dsq, 9.0);
        assert_eq!(pos, vec(5.0, 0.0));
    }

    #[test]
    fn closest_on_polyline_finds_nearest_segment() {
        let pl = make_polyline(vec![vec(0.0, 0.0), vec(10.0, 0.0), vec(10.0, 10.0)]);
        let (along, dsq, pos) = closest_on_polyline(&pl, vec(11.0, 5.0));
        assert_eq!(pos, vec(10.0, 5.0));
        assert_eq!(along, 15.0);
        assert_eq!(dsq, 1.0);
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let mut r1 = Rng::from_seed(7);
        let mut r2 = Rng::from_seed(7);
        let n1 = Noise2D::new(&mut r1);
        let n2 = Noise2D::new(&mut r2);
        for i in 0..10 {
            let x = i as f64 * 0.37;
            let y = i as f64 * 1.13;
            let a = n1.at(x, y);
            assert_eq!(a, n2.at(x, y));
            assert!((0.0..=1.0).contains(&a));
        }
    }

    #[test]
    fn spatial_hash_query_radius() {
        let mut sh = SpatialHash::new(10.0, |p: &Vec2| *p);
        sh.insert(vec(0.0, 0.0));
        sh.insert(vec(5.0, 0.0));
        sh.insert(vec(100.0, 100.0));
        let hits = sh.query_radius(vec(0.0, 0.0), 6.0);
        assert_eq!(hits.len(), 2);
    }
}
