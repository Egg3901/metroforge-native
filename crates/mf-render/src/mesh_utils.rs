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
