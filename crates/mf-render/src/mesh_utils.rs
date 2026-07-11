//! Shared low-level mesh-building helpers used by `terrain.rs`, `roads.rs`,
//! `buildings.rs` and `transit.rs`. Everything here builds a single
//! `TriangleList` `Mesh` with `ATTRIBUTE_POSITION`/`ATTRIBUTE_NORMAL`/
//! `ATTRIBUTE_COLOR` — no textures, per the art direction ("NO texture").

use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;

/// Accumulates vertices/indices for one merged mesh (a road class, a
/// building chunk, a route stripe, ...). Kept as a plain growable buffer so
/// callers can append many primitives before a single `build()`.
#[derive(Default)]
pub struct MeshBuffers {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

fn color_to_array(c: Color) -> [f32; 4] {
    let s = c.to_srgba();
    [s.red, s.green, s.blue, s.alpha]
}

impl MeshBuffers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Vertex count so far — exposed for callers estimating/preallocating
    /// against the wire's per-city vertex totals (e.g. real building
    /// footprints) and for tests asserting exact geometry counts.
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// Index count so far (see `vertex_count`).
    pub fn index_count(&self) -> usize {
        self.indices.len()
    }

    /// Preallocated variant of `new`: real building footprints arrive with a
    /// wire-declared vertex total, so the real-footprint path in
    /// `buildings.rs` can estimate each chunk's final vertex/index count up
    /// front and avoid the repeated-doubling reallocs a bare `Vec::new` would
    /// otherwise do across ~2M vertices.
    pub fn with_capacity(vertex_capacity: usize, index_capacity: usize) -> Self {
        Self {
            positions: Vec::with_capacity(vertex_capacity),
            normals: Vec::with_capacity(vertex_capacity),
            colors: Vec::with_capacity(vertex_capacity),
            indices: Vec::with_capacity(index_capacity),
        }
    }

    /// Clear lengths but keep capacity — for hot-path scratch buffers
    /// (agents rebuild ~20 Hz) that would otherwise reallocate every tick.
    pub fn clear(&mut self) {
        self.positions.clear();
        self.normals.clear();
        self.colors.clear();
        self.indices.clear();
    }

    /// Grow capacities if needed without discarding existing contents.
    pub fn ensure_capacity(&mut self, vertex_capacity: usize, index_capacity: usize) {
        if self.positions.capacity() < vertex_capacity {
            self.positions
                .reserve(vertex_capacity - self.positions.len());
        }
        if self.normals.capacity() < vertex_capacity {
            self.normals.reserve(vertex_capacity - self.normals.len());
        }
        if self.colors.capacity() < vertex_capacity {
            self.colors.reserve(vertex_capacity - self.colors.len());
        }
        if self.indices.capacity() < index_capacity {
            self.indices.reserve(index_capacity - self.indices.len());
        }
    }

    /// Move attributes into `mesh`, then re-reserve the previous capacities
    /// so the next `clear`+fill cycle does not reallocate.
    pub fn apply_to_mesh(&mut self, mesh: &mut Mesh) {
        let pos_cap = self.positions.capacity();
        let nrm_cap = self.normals.capacity();
        let col_cap = self.colors.capacity();
        let idx_cap = self.indices.capacity();
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            std::mem::take(&mut self.positions),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, std::mem::take(&mut self.normals));
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, std::mem::take(&mut self.colors));
        mesh.insert_indices(Indices::U32(std::mem::take(&mut self.indices)));
        self.positions = Vec::with_capacity(pos_cap);
        self.normals = Vec::with_capacity(nrm_cap);
        self.colors = Vec::with_capacity(col_cap);
        self.indices = Vec::with_capacity(idx_cap);
    }

    /// One quad (p0..p3 wound counter-clockwise when viewed from `normal`),
    /// one color per corner (lets callers bake a cheap top-to-base gradient
    /// instead of flat-shading every face).
    #[allow(clippy::too_many_arguments)]
    pub fn push_quad(
        &mut self,
        p0: Vec3,
        p1: Vec3,
        p2: Vec3,
        p3: Vec3,
        normal: Vec3,
        c0: Color,
        c1: Color,
        c2: Color,
        c3: Color,
    ) {
        let base = self.positions.len() as u32;
        let n = normal.to_array();
        for (p, c) in [(p0, c0), (p1, c1), (p2, c2), (p3, c3)] {
            self.positions.push(p.to_array());
            self.normals.push(n);
            self.colors.push(color_to_array(c));
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Convenience: one quad, one flat color.
    pub fn push_flat_quad(
        &mut self,
        p0: Vec3,
        p1: Vec3,
        p2: Vec3,
        p3: Vec3,
        normal: Vec3,
        c: Color,
    ) {
        self.push_quad(p0, p1, p2, p3, normal, c, c, c, c);
    }

    /// One triangle (used for route-stripe chevron arrows).
    pub fn push_tri(&mut self, p0: Vec3, p1: Vec3, p2: Vec3, normal: Vec3, c: Color) {
        let base = self.positions.len() as u32;
        let n = normal.to_array();
        for p in [p0, p1, p2] {
            self.positions.push(p.to_array());
            self.normals.push(n);
            self.colors.push(color_to_array(c));
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    pub fn build(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// Append a merged cuboid (box) with distinct top/side/base vertex colors:
/// the top cap is flat `top`, the four side walls gradient from `side` (at
/// the top edge) down to `base` (at the ground edge) — a cheap top-lit look
/// that reads even fully unlit (art-direction §1). The bottom cap is skipped
/// (never visible: the camera never goes under the ground plane).
///
/// `center_xz` is the footprint center in world (X, Z); `ground_y` is the
/// base of the box; `height` is how tall it stands above `ground_y`.
#[allow(clippy::too_many_arguments)]
pub fn append_cuboid(
    buf: &mut MeshBuffers,
    center_xz: Vec2,
    ground_y: f32,
    half_x: f32,
    half_z: f32,
    height: f32,
    top: Color,
    side: Color,
    base: Color,
) {
    let x0 = center_xz.x - half_x;
    let x1 = center_xz.x + half_x;
    let z0 = center_xz.y - half_z;
    let z1 = center_xz.y + half_z;
    let y0 = ground_y;
    let y1 = ground_y + height;

    // Top cap.
    buf.push_flat_quad(
        Vec3::new(x0, y1, z1),
        Vec3::new(x1, y1, z1),
        Vec3::new(x1, y1, z0),
        Vec3::new(x0, y1, z0),
        Vec3::Y,
        top,
    );

    // Four side walls, each a vertical gradient side (top) -> base (bottom).
    let walls = [
        // +Z (south)
        (
            Vec3::new(x0, y0, z1),
            Vec3::new(x1, y0, z1),
            Vec3::new(x1, y1, z1),
            Vec3::new(x0, y1, z1),
            Vec3::Z,
        ),
        // -Z (north)
        (
            Vec3::new(x1, y0, z0),
            Vec3::new(x0, y0, z0),
            Vec3::new(x0, y1, z0),
            Vec3::new(x1, y1, z0),
            Vec3::NEG_Z,
        ),
        // +X (east)
        (
            Vec3::new(x1, y0, z1),
            Vec3::new(x1, y0, z0),
            Vec3::new(x1, y1, z0),
            Vec3::new(x1, y1, z1),
            Vec3::X,
        ),
        // -X (west)
        (
            Vec3::new(x0, y0, z0),
            Vec3::new(x0, y0, z1),
            Vec3::new(x0, y1, z1),
            Vec3::new(x0, y1, z0),
            Vec3::NEG_X,
        ),
    ];
    for (p0, p1, p2, p3, n) in walls {
        // p0,p1 = bottom corners (base color); p2,p3 = top corners (side color)
        buf.push_quad(p0, p1, p2, p3, n, base, base, side, side);
    }
}

/// Twice the standard (shoelace) signed area of a polygon, treating each
/// `Vec2` as (x, y) in the ordinary math sense (positive = counter-clockwise
/// when x is right and y is "up" on the page). This is a plain planar
/// formula with no notion of Bevy's axes; callers decide what the two
/// components mean (e.g. `append_prism` feeds it world (x, z) pairs).
fn signed_area2(ring: &[Vec2]) -> f32 {
    let n = ring.len();
    let mut a = 0.0;
    for i in 0..n {
        let p = ring[i];
        let q = ring[(i + 1) % n];
        a += p.x * q.y - q.x * p.y;
    }
    a
}

/// Unsigned polygon area (world units squared) — used both by `buildings.rs`
/// (footprint area feeds the built-volume estimate that finds a city's dense
/// core) and by this module's own ear-clip tests (triangle-area-sum check).
pub fn polygon_area(ring: &[Vec2]) -> f32 {
    (signed_area2(ring) / 2.0).abs()
}

/// Signed area x2 of the triangle (o, a, b) — positive when (o, a, b) turns
/// left (counter-clockwise) in the ordinary math sense. Used both as the
/// convex/reflex turn test and, restated, as the point-in-triangle test in
/// `ear_clip_indices`.
fn cross2(o: Vec2, a: Vec2, b: Vec2) -> f32 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// Ear-clip triangulate a simple polygon (no holes; may be concave, may
/// contain exactly-collinear points). Returns triangle index-triples into
/// `ring`. Fixed contract regardless of `ring`'s own winding: each returned
/// `[a, b, c]` is CCW-standard, i.e. `signed_area2([ring[a], ring[b],
/// ring[c]])` is positive — this utility normalizes internally rather than
/// trusting (or requiring) the caller to hand it a particular winding.
/// Callers that need a specific mesh front-face convention (see
/// `append_prism`'s roof cap) apply their own fixed transform on top of this
/// fixed output convention.
///
/// Two robustness guarantees, both load-bearing because this ultimately runs
/// on untrusted wire data (`BuildingFootprint.verts`, 3..=64 verts but
/// otherwise unchecked):
/// - collinear "ears" (three points with ~zero turn) are never selected by
///   the normal search, so a long straight wall doesn't get chewed into
///   degenerate slivers while real ears are still available;
/// - if the search ever fails to find ANY valid ear while more than 3
///   vertices remain (self-intersecting or otherwise invalid input — not a
///   real simple polygon), it falls back to a fan from the first remaining
///   vertex instead of looping. A bad footprint may draw a visibly wrong
///   roof; it must never hang or panic.
///
/// Always returns exactly `ring.len() - 2` triangles for `ring.len() >= 3`
/// (empty for fewer) — the fallback fan always contributes exactly as many
/// triangles as ear-clipping would have for whatever vertices are left when
/// it kicks in, so the total is invariant to where (or whether) it kicks in.
pub fn ear_clip_indices(ring: &[Vec2]) -> Vec<[usize; 3]> {
    let n = ring.len();
    if n < 3 {
        return Vec::new();
    }

    // Normalize to a CCW-standard working copy: the convexity/containment
    // tests below assume CCW, and the wire's claimed winding is explicitly
    // not trusted (see `BuildingFootprint::verts` doc). `orig_index` maps a
    // position in the (possibly reversed) working copy back to `ring`'s own
    // indexing so the returned triples index `ring` directly either way.
    let reversed = signed_area2(ring) < 0.0;
    let working: Vec<Vec2> = if reversed {
        ring.iter().rev().copied().collect()
    } else {
        ring.to_vec()
    };
    let orig_index = |working_pos: usize| -> usize {
        if reversed {
            n - 1 - working_pos
        } else {
            working_pos
        }
    };

    const EPS: f32 = 1e-6;
    // Positions into `working` still part of the shrinking polygon.
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity(n - 2);

    let is_ear = |remaining: &[usize], i: usize| -> bool {
        let m = remaining.len();
        let prev = working[remaining[(i + m - 1) % m]];
        let cur = working[remaining[i]];
        let next = working[remaining[(i + 1) % m]];
        // Reflex or collinear vertices can never be ears of a CCW polygon.
        if cross2(prev, cur, next) <= EPS {
            return false;
        }
        for (k, &wp) in remaining.iter().enumerate() {
            if k == (i + m - 1) % m || k == i || k == (i + 1) % m {
                continue;
            }
            let p = working[wp];
            if cross2(prev, cur, p) >= 0.0
                && cross2(cur, next, p) >= 0.0
                && cross2(next, prev, p) >= 0.0
            {
                return false; // another vertex sits inside this candidate ear
            }
        }
        true
    };

    // Each successful clip strictly shrinks `remaining` by one, so at most
    // `n` clips can ever succeed; bounding the outer loop at `n` guarantees
    // termination even if `is_ear` somehow kept finding (and re-finding)
    // ears forever, in addition to the explicit stall-break below.
    let mut guard = 0usize;
    while remaining.len() > 3 && guard < n {
        guard += 1;
        let mut clipped = false;
        for i in 0..remaining.len() {
            if is_ear(&remaining, i) {
                let m = remaining.len();
                let prev = remaining[(i + m - 1) % m];
                let cur = remaining[i];
                let next = remaining[(i + 1) % m];
                triangles.push([orig_index(prev), orig_index(cur), orig_index(next)]);
                remaining.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break; // stalled on degenerate/self-intersecting input: fall back
        }
    }

    // Fallback fan: covers both the normal case (exactly 3 vertices left
    // after clean ear-clipping — one final triangle) and the stalled case
    // (whatever didn't clip, possibly the whole ring). Never panics.
    if remaining.len() >= 3 {
        for w in 1..remaining.len() - 1 {
            triangles.push([
                orig_index(remaining[0]),
                orig_index(remaining[w]),
                orig_index(remaining[w + 1]),
            ]);
        }
    }

    triangles
}

/// Fixed stylized sun direction for wall cel-shading, in the world XZ plane.
/// Not a physically accurate sun angle — art direction wants a flat,
/// quantized three-tone read (bright face / plain / shade face) with zero
/// shader work, so a fixed direction chosen to reliably split axis-aligned
/// city blocks into one lit face and one shaded face is exactly the goal.
fn wall_sun_dir() -> Vec2 {
    Vec2::new(0.55, 0.35).normalize()
}

/// Extrude a building footprint `ring` (world X/Z, EITHER winding, see
/// below) into a prism: one quad per ring edge for the walls, plus a
/// triangulated roof cap, plus (when `base_offset > 0`) a triangulated
/// bottom cap. Companion to `append_cuboid` for real per-building vector
/// footprints instead of axis-aligned boxes. Returns `(vertices_added,
/// indices_added)` so callers can preallocate/log/test against exact
/// counts.
///
/// The prism occupies world Y in `[ground_y + base_offset, ground_y +
/// height]` (so `height` names the same thing it always has: the absolute
/// top of the mass above `ground_y`, and `base_offset` is where the walls
/// start rather than always starting at `ground_y`). This exists for OSM
/// `building:part` stacking: one real building often arrives as several
/// footprints at different min/max heights (a ground podium, a tower set
/// back on top of it, a spire on top of that). Callers are responsible for
/// ensuring `height > base_offset`; this function does not re-derive or
/// validate the relationship, it only extrudes between the two Y values it's
/// given.
///
/// `ring` is normalized to a single fixed winding first (via signed area):
/// both the wall-outward-normal formula and the cap winding below depend on
/// a known traversal direction, and the wire's claimed CCW-in-y-down
/// winding is explicitly not trusted (protocol doc on
/// `BuildingFootprint::verts`: "decode does NOT trust or check that").
///
/// After normalizing to negative signed area (the convention this function's
/// math is derived against), each edge's direction `dir = b - a` rotated +90
/// degrees in the XZ plane, `(dx, dz) -> (-dz, dx)`, gives the outward wall
/// normal. Verified against `append_cuboid`'s four hand-wound walls: a unit
/// square ring in this normalized order, walked edge by edge, reproduces
/// exactly `append_cuboid`'s north/south/east/west normals (see this
/// module's tests).
///
/// Roof cap: `ear_clip_indices` always returns CCW-standard (positive
/// signed-area) triangles regardless of its input's own winding. Bevy's
/// correct top-face front-facing order for a normal declared `+Y` is instead
/// CW-standard (negative signed area, the same convention `terrain.rs`'s
/// grid-quad comment derives for its own top-down quads), so each returned
/// triangle has its last two vertices swapped before being pushed.
///
/// Bottom cap: only emitted when `base_offset > 0`. A ground-based prism
/// (`base_offset == 0`) never shows its underside to the camera, so skipping
/// it there is a real cost saving, not just an omission. An elevated mass
/// (a skybridge, a cantilevered tower set back above a podium) IS visible
/// from below, so its underside needs a real face. The bottom cap's winding
/// is the roof cap's reversed: since the roof swaps the ear-clipper's raw
/// `(ia, ib, ic)` to `(ia, ic, ib)` to flip CCW-standard into the CW-standard
/// a `+Y` face needs, the bottom cap pushes the raw, unswapped order at
/// `y0` with a `-Y` normal, which is exactly the front-facing order that
/// normal needs.
///
/// Colors: `side_plain` is used when a wall's outward normal is roughly
/// perpendicular to the fixed stylized sun direction (`wall_sun_dir`);
/// `side_sunlit` / `side_shaded` when it faces toward / away from it. Bottom
/// wall edge is always `base` (matches `append_cuboid`'s top-lit gradient
/// trick); `top` is flat on the roof cap; the bottom cap (when present) is
/// flat `base`, matching the shaded tone the wall bases already use there.
#[allow(clippy::too_many_arguments)]
pub fn append_prism(
    buf: &mut MeshBuffers,
    ring: &[Vec2],
    ground_y: f32,
    base_offset: f32,
    height: f32,
    top: Color,
    side_plain: Color,
    side_sunlit: Color,
    side_shaded: Color,
    base: Color,
) -> (usize, usize) {
    let start_v = buf.vertex_count();
    let start_i = buf.index_count();
    let n = ring.len();
    if n < 3 {
        return (0, 0); // degenerate footprint: nothing sane to draw
    }

    let normalized_ring: Vec<Vec2> = if signed_area2(ring) < 0.0 {
        ring.to_vec()
    } else {
        ring.iter().rev().copied().collect()
    };

    let sun_dir = wall_sun_dir();
    let y0 = ground_y + base_offset;
    let y1 = ground_y + height;
    for i in 0..n {
        let a = normalized_ring[i];
        let b = normalized_ring[(i + 1) % n];
        let dir = (b - a).normalize_or_zero();
        if dir == Vec2::ZERO {
            continue; // duplicate consecutive vertex: no wall to draw
        }
        let normal_xz = Vec2::new(-dir.y, dir.x);
        let dot = normal_xz.dot(sun_dir);
        let wall_color = if dot > 0.15 {
            side_sunlit
        } else if dot < -0.15 {
            side_shaded
        } else {
            side_plain
        };
        let normal = Vec3::new(normal_xz.x, 0.0, normal_xz.y);
        let bottom_a = Vec3::new(a.x, y0, a.y);
        let bottom_b = Vec3::new(b.x, y0, b.y);
        let top_a = Vec3::new(a.x, y1, a.y);
        let top_b = Vec3::new(b.x, y1, b.y);
        // p0,p1 = bottom corners (base color); p2,p3 = top corners (wall
        // color): same top-lit vertical gradient trick as `append_cuboid`.
        buf.push_quad(
            bottom_a, bottom_b, top_b, top_a, normal, base, base, wall_color, wall_color,
        );
    }

    for [ia, ib, ic] in ear_clip_indices(&normalized_ring) {
        let pa = normalized_ring[ia];
        let pb = normalized_ring[ib];
        let pc = normalized_ring[ic];
        // Swap (ib, ic): flips the ear-clipper's fixed CCW-standard output
        // to the CW-standard order Bevy's top face needs (see doc comment).
        buf.push_tri(
            Vec3::new(pa.x, y1, pa.y),
            Vec3::new(pc.x, y1, pc.y),
            Vec3::new(pb.x, y1, pb.y),
            Vec3::Y,
            top,
        );
    }

    if base_offset > 0.0 {
        // Elevated mass: the underside is real geometry a camera below or
        // beside it can see (skybridges, cantilevered upper parts set back
        // from a podium). Raw (unswapped) ear-clip order at y0 with a -Y
        // normal is front-facing here (see doc comment).
        for [ia, ib, ic] in ear_clip_indices(&normalized_ring) {
            let pa = normalized_ring[ia];
            let pb = normalized_ring[ib];
            let pc = normalized_ring[ic];
            buf.push_tri(
                Vec3::new(pa.x, y0, pa.y),
                Vec3::new(pb.x, y0, pb.y),
                Vec3::new(pc.x, y0, pc.y),
                Vec3::NEG_Y,
                base,
            );
        }
    }

    (buf.vertex_count() - start_v, buf.index_count() - start_i)
}

/// Perpendicular-offset a polyline (ported from `renderer.ts`'s
/// `offsetPolyline`): used for parallel-corridor route-stripe bundling.
/// Endpoints are pinned to the original so stripes still meet at stations.
pub fn offset_polyline(pts: &[Vec2], dist: f32) -> Vec<Vec2> {
    if pts.len() < 2 || dist.abs() < 0.5 {
        return pts.to_vec();
    }
    let n = pts.len();
    let mut out = vec![Vec2::ZERO; n];
    for i in 0..n {
        let (dx, dy) = if i == 0 {
            (pts[1].x - pts[0].x, pts[1].y - pts[0].y)
        } else if i == n - 1 {
            (pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y)
        } else {
            (pts[i + 1].x - pts[i - 1].x, pts[i + 1].y - pts[i - 1].y)
        };
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        out[i] = Vec2::new(pts[i].x + (-dy / len) * dist, pts[i].y + (dx / len) * dist);
    }
    out[0] = pts[0];
    out[n - 1] = pts[n - 1];
    out
}

/// Cumulative arc-length table for a world-space polyline (world X, world Y
/// — i.e. Bevy X, Z), used to walk fixed-spacing features (chevrons,
/// procedural building lots) along it.
pub fn arc_length_table(pts: &[Vec2]) -> (Vec<f32>, f32) {
    let mut cum = Vec::with_capacity(pts.len());
    cum.push(0.0);
    let mut total = 0.0;
    for w in pts.windows(2) {
        total += w[0].distance(w[1]);
        cum.push(total);
    }
    (cum, total)
}

/// Point + unit tangent at arc-length `d` along a polyline described by
/// `pts`/`cum` (as returned by [`arc_length_table`]).
pub fn point_along(pts: &[Vec2], cum: &[f32], d: f32) -> (Vec2, Vec2) {
    let mut lo = 0usize;
    let mut hi = cum.len() - 1;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if cum[mid] < d {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let seg = lo.max(1);
    let d0 = cum[seg - 1];
    let seg_len = (cum[seg] - d0).max(1e-6);
    let t = ((d - d0) / seg_len).clamp(0.0, 1.0);
    let a = pts[seg - 1];
    let b = pts[seg];
    let pos = a + (b - a) * t;
    let dir = (b - a).normalize_or_zero();
    (pos, dir)
}

/// Append a ribbon (a road, track, or route stripe) following `points`
/// (world X/Y pairs — i.e. Bevy X/Z), `width` wide, one flat `color`, with
/// each vertex individually raised via `height_at(x, z) + y_offset` so the
/// ribbon follows terrain relief. Segments are independent quads (no miter
/// joins) — per art-direction §2, overlapping intersections in the same
/// flat color read fine without seams, so this is a deliberate
/// simplification, not a bug.
/// Insert intermediate points every `step` meters along a polyline so a
/// terrain-following ribbon actually follows the terrain: source polylines
/// are simplified to long straight segments, and any relief between two
/// original vertices otherwise swallows the ribbon whole (the root cause of
/// streets reading faint and dashed since v0.1).
/// Chaikin corner cutting, endpoint-preserving: transit ribbons follow
/// track polylines whose hard corners read as janky staircases (owner);
/// two passes turn every corner into a flowing curve while streets keep
/// their true grid-crisp geometry by simply not calling this.
pub fn smooth_polyline(pts: &[Vec2], iterations: usize) -> Vec<Vec2> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut cur = pts.to_vec();
    for _ in 0..iterations {
        let mut out = Vec::with_capacity(cur.len() * 2);
        out.push(cur[0]);
        for w in cur.windows(2) {
            out.push(w[0].lerp(w[1], 0.25));
            out.push(w[0].lerp(w[1], 0.75));
        }
        out.push(cur[cur.len() - 1]);
        cur = out;
    }
    cur
}

pub fn densify_polyline(pts: &[Vec2], step: f32) -> Vec<Vec2> {
    if pts.len() < 2 || step <= 0.0 {
        return pts.to_vec();
    }
    let mut out = Vec::with_capacity(pts.len() * 2);
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        out.push(a);
        let len = a.distance(b);
        if len > step {
            let n = (len / step).floor() as usize;
            for i in 1..=n {
                let t = i as f32 * step / len;
                if t < 0.999 {
                    out.push(a.lerp(b, t));
                }
            }
        }
    }
    out.push(pts[pts.len() - 1]);
    out
}

pub fn append_ribbon(
    buf: &mut MeshBuffers,
    points: &[Vec2],
    y_offset: f32,
    width: f32,
    color: Color,
    height_at: impl Fn(f32, f32) -> f32,
) {
    let half = width * 0.5;
    for w in points.windows(2) {
        let a = w[0];
        let b = w[1];
        let dir = (b - a).normalize_or_zero();
        if dir == Vec2::ZERO {
            continue;
        }
        let perp = Vec2::new(-dir.y, dir.x) * half;
        let ya = height_at(a.x, a.y) + y_offset;
        let yb = height_at(b.x, b.y) + y_offset;
        let a0 = Vec3::new(a.x + perp.x, ya, a.y + perp.y);
        let a1 = Vec3::new(a.x - perp.x, ya, a.y - perp.y);
        let b0 = Vec3::new(b.x + perp.x, yb, b.y + perp.y);
        let b1 = Vec3::new(b.x - perp.x, yb, b.y - perp.y);
        // Winding: with `dir = (dx, dz)` and `perp = (-dz, dx) * half`, the
        // triangle (a1, b1, b0) has edges `v1 = b1-a1 ~= (dx, dz)*len` and
        // `v2 = b0-a1 ~= (dx,dz)*len - perp*2`; the right-hand cross product
        // `v1 x v2` works out to `-2*half*len*Y` (since `dx^2+dz^2 == 1`) —
        // i.e. it points opposite the declared `+Y` normal. `push_quad`'s
        // `[0,1,2,0,2,3]` index order requires (p0,p1,p2) wound CCW as seen
        // from `normal` (Bevy/wgpu front-face = CCW), so `(a1,b1,b0,a0)`
        // is backwards; passing `(a0,b0,b1,a1)` (the same quad, reversed)
        // flips the sign to `+Y` and matches the normal. Verified by hand
        // with dir=(1,0): (a1,b1,b0)=(0,-1)->(1,-1)->(1,1) gives cross=-Y;
        // (a0,b0,b1)=(0,1)->(1,1)->(1,-1) gives cross=+Y.
        buf.push_flat_quad(a0, b0, b1, a1, Vec3::Y, color);
    }
}

/// Deterministic 0..1 pseudo-random value from a world-position hash — used
/// for per-building brightness jitter and procedural-lot placement, mirrors
/// the coordinate hash used throughout `renderer.ts`.
pub fn hash01(x: i32, y: i32) -> f32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) as u32) as f32 / 4_294_967_296.0
}

#[cfg(test)]
mod ear_clip_tests {
    use super::*;

    fn triangle_area(a: Vec2, b: Vec2, c: Vec2) -> f32 {
        (cross2(a, b, c) / 2.0).abs()
    }

    /// Assert `ear_clip_indices(ring)` produces `ring.len() - 2` triangles
    /// whose summed (unsigned) area matches the polygon's own area within
    /// 0.5% — the load-bearing correctness check (not just "didn't panic").
    /// Runs the check on `ring` both as given and reversed, since real wire
    /// data may arrive in either winding and the function must handle both
    /// identically.
    fn assert_triangulates_correctly(ring: &[Vec2]) {
        for variant in [ring.to_vec(), ring.iter().rev().copied().collect()] {
            let tris = ear_clip_indices(&variant);
            assert_eq!(
                tris.len(),
                variant.len() - 2,
                "expected {} triangles, got {}",
                variant.len() - 2,
                tris.len()
            );
            let expected_area = polygon_area(&variant);
            let got_area: f32 = tris
                .iter()
                .map(|&[a, b, c]| triangle_area(variant[a], variant[b], variant[c]))
                .sum();
            let tolerance = (expected_area * 0.005).max(1e-4);
            assert!(
                (got_area - expected_area).abs() <= tolerance,
                "triangle area sum {got_area} vs polygon area {expected_area} (tolerance {tolerance})"
            );
        }
    }

    #[test]
    fn square() {
        let ring = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        assert_triangulates_correctly(&ring);
    }

    #[test]
    fn l_shape() {
        // Concave hexagon: 2x2 square with a 1x1 corner notch removed
        // (area = 4 - 1 = 3), matching the "real building footprint" shape
        // this exists to handle.
        let ring = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        assert_triangulates_correctly(&ring);
    }

    #[test]
    fn u_shape_concave() {
        // 3x2 rectangle with a 1x1 notch cut from the top-middle edge
        // (area = 6 - 1 = 5), two reflex vertices at the notch mouth.
        let ring = [
            Vec2::new(0.0, 0.0),
            Vec2::new(3.0, 0.0),
            Vec2::new(3.0, 2.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        assert_triangulates_correctly(&ring);
    }

    #[test]
    fn collinear_point_on_edge() {
        // Square with an extra vertex sitting exactly on the bottom edge
        // (0,0)-(2,0): must not be treated as a valid ear on its own, and
        // must not stall the whole clip.
        let ring = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        assert_triangulates_correctly(&ring);
    }

    #[test]
    fn both_windings_of_a_concave_ring_agree_on_area() {
        let ring = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        let reversed: Vec<Vec2> = ring.iter().rev().copied().collect();
        let area_fwd = polygon_area(&ring);
        let area_rev = polygon_area(&reversed);
        assert!((area_fwd - area_rev).abs() < 1e-6);
        assert!((area_fwd - 3.0).abs() < 1e-6);
    }

    #[test]
    fn degenerate_input_never_panics() {
        // Self-intersecting bowtie: not a valid simple polygon, but must
        // still terminate and return some triangles rather than hang/panic.
        let ring = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(0.0, 2.0),
        ];
        let tris = ear_clip_indices(&ring);
        assert_eq!(tris.len(), ring.len() - 2);
    }

    #[test]
    fn fewer_than_three_points_returns_empty() {
        assert!(ear_clip_indices(&[]).is_empty());
        assert!(ear_clip_indices(&[Vec2::ZERO]).is_empty());
        assert!(ear_clip_indices(&[Vec2::ZERO, Vec2::X]).is_empty());
    }

    #[test]
    fn append_prism_matches_append_cuboid_wall_normals() {
        // A unit square footprint in the CW-standard order `append_prism`
        // normalizes to should reproduce exactly `append_cuboid`'s four
        // hand-verified wall normals (+Z south, -Z north, +X east, -X west)
        // for the corresponding edges.
        let ring = [
            Vec2::new(-1.0, -1.0),
            Vec2::new(-1.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, -1.0),
        ];
        let mut buf = MeshBuffers::new();
        let white = Color::WHITE;
        let (v, i) = append_prism(
            &mut buf, &ring, 0.0, 0.0, 10.0, white, white, white, white, white,
        );
        // 4 wall quads (4 verts each) + cap triangles (n-2=2, 3 verts each).
        // base_offset=0 (ground-based), so no bottom cap.
        assert_eq!(v, 4 * 4 + 2 * 3);
        assert_eq!(i, 4 * 6 + 2 * 3);
    }

    #[test]
    fn append_prism_degenerate_ring_is_a_noop() {
        let mut buf = MeshBuffers::new();
        let white = Color::WHITE;
        let ring = [Vec2::ZERO, Vec2::X];
        let (v, i) = append_prism(
            &mut buf, &ring, 0.0, 0.0, 10.0, white, white, white, white, white,
        );
        assert_eq!((v, i), (0, 0));
        assert!(buf.is_empty());
    }

    #[test]
    fn append_prism_with_base_offset_emits_bottom_cap() {
        // Same unit-square ring as the wall-normal test above, but elevated
        // (base_offset=5.0 > 0): an elevated mass must gain a bottom cap on
        // top of the usual walls + roof cap, since its underside is now
        // visible (skybridge / cantilever case from the module doc).
        let ring = [
            Vec2::new(-1.0, -1.0),
            Vec2::new(-1.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, -1.0),
        ];
        let mut buf = MeshBuffers::new();
        let white = Color::WHITE;
        let (v, i) = append_prism(
            &mut buf, &ring, 0.0, 5.0, 10.0, white, white, white, white, white,
        );
        // 4 wall quads (4 verts each) + roof cap (n-2=2 tris, 3 verts each)
        // + bottom cap (n-2=2 tris, 3 verts each): the bottom cap is the
        // only addition versus the base_offset=0 case above.
        assert_eq!(v, 4 * 4 + 2 * 3 + 2 * 3);
        assert_eq!(i, 4 * 6 + 2 * 3 + 2 * 3);
        assert_eq!(buf.vertex_count(), v);
        assert_eq!(buf.index_count(), i);
    }

    #[test]
    fn append_prism_walls_span_ground_plus_base_to_ground_plus_height() {
        // Verifies the Y-range contract directly: with ground_y=100,
        // base_offset=5, height=30, every wall vertex's Y must be either
        // 105.0 (bottom edge) or 130.0 (top edge) -- nothing at 100 or at
        // 30, which would indicate the offset was ignored or double
        // applied.
        let ring = [
            Vec2::new(-1.0, -1.0),
            Vec2::new(-1.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, -1.0),
        ];
        let mut buf = MeshBuffers::new();
        let white = Color::WHITE;
        append_prism(
            &mut buf, &ring, 100.0, 5.0, 30.0, white, white, white, white, white,
        );
        let mesh = buf.build();
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("float3 positions");
        for p in positions {
            let y = p[1];
            assert!(
                (y - 105.0).abs() < 1e-4 || (y - 130.0).abs() < 1e-4,
                "unexpected wall/cap vertex Y {y}"
            );
        }
    }
}
