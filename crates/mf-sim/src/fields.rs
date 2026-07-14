//! Scalar field grid helpers. Port of `sim/src/core/fields.ts`.
//!
//! A [`FieldGrid`] is a coarse row-major grid of per-cell scalar channels
//! (terrain, water, parks, population, jobs, land value, NIMBY). Worldgen (P2)
//! writes these; the transit/economy systems (P3) sample them. Cell size is
//! fixed ([`FIELD_CELL`]); bigger worlds simply have more cells.

use crate::constants::{FIELD_CELL, FIELD_H, FIELD_W, WORLD_SIZE};
use crate::geometry::{clamp, Vec2};
use crate::types::FieldGrid;

/// Build an empty [`FieldGrid`] for a world of `world_size` meters (square,
/// centered on the origin). Mirrors `createFieldGrid`. `None` = default world.
pub fn create_field_grid(world_size: Option<f64>) -> FieldGrid {
    let world_size = world_size.unwrap_or(WORLD_SIZE);
    // fixed cell size -> bigger worlds simply have more cells
    let dim = ((world_size / FIELD_CELL).round() as i64).max(1) as u32;
    let (w, h) = if world_size == WORLD_SIZE {
        (FIELD_W, FIELD_H)
    } else {
        (dim, dim)
    };
    let n = (w * h) as usize;
    FieldGrid {
        w,
        h,
        cell_size: FIELD_CELL,
        origin_x: -(w as f64 * FIELD_CELL) / 2.0,
        origin_y: -(h as f64 * FIELD_CELL) / 2.0,
        terrain: vec![0.0; n],
        water: vec![0; n],
        parks: vec![0; n],
        population: vec![0.0; n],
        jobs: vec![0.0; n],
        land_value: vec![0.0; n],
        nimby: vec![0.0; n],
    }
}

/// Row-major cell index containing world point `p` (clamped to the grid).
/// Mirrors `cellIndexAt`.
#[inline]
pub fn cell_index_at(g: &FieldGrid, p: Vec2) -> usize {
    let cx = clamp(
        ((p.x - g.origin_x) / g.cell_size).floor(),
        0.0,
        (g.w - 1) as f64,
    ) as usize;
    let cy = clamp(
        ((p.y - g.origin_y) / g.cell_size).floor(),
        0.0,
        (g.h - 1) as f64,
    ) as usize;
    cy * g.w as usize + cx
}

/// World-space center of cell `idx`. Mirrors `cellCenter`.
#[inline]
pub fn cell_center(g: &FieldGrid, idx: usize) -> Vec2 {
    let cx = (idx % g.w as usize) as f64;
    let cy = (idx / g.w as usize) as f64;
    Vec2 {
        x: g.origin_x + (cx + 0.5) * g.cell_size,
        y: g.origin_y + (cy + 0.5) * g.cell_size,
    }
}

/// Bilinear sample of a `f32` field channel at a world point. Mirrors
/// `sampleField`.
pub fn sample_field(g: &FieldGrid, field: &[f32], p: Vec2) -> f64 {
    let fx = clamp(
        (p.x - g.origin_x) / g.cell_size - 0.5,
        0.0,
        g.w as f64 - 1.001,
    );
    let fy = clamp(
        (p.y - g.origin_y) / g.cell_size - 0.5,
        0.0,
        g.h as f64 - 1.001,
    );
    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(g.w as usize - 1);
    let y1 = (y0 + 1).min(g.h as usize - 1);
    let tx = fx - x0 as f64;
    let ty = fy - y0 as f64;
    let w = g.w as usize;
    let v00 = field[y0 * w + x0] as f64;
    let v10 = field[y0 * w + x1] as f64;
    let v01 = field[y1 * w + x0] as f64;
    let v11 = field[y1 * w + x1] as f64;
    (v00 * (1.0 - tx) + v10 * tx) * (1.0 - ty) + (v01 * (1.0 - tx) + v11 * tx) * ty
}

/// Whether the cell containing `p` is water. Mirrors `isWaterAt`.
#[inline]
pub fn is_water_at(g: &FieldGrid, p: Vec2) -> bool {
    g.water[cell_index_at(g, p)] == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_grid_is_96_squared() {
        let g = create_field_grid(None);
        assert_eq!(g.w, 96);
        assert_eq!(g.h, 96);
        assert_eq!(g.cell_size, 125.0);
        assert_eq!(g.terrain.len(), 96 * 96);
        assert_eq!(g.origin_x, -6000.0);
    }

    #[test]
    fn cell_center_roundtrips_through_index() {
        let g = create_field_grid(None);
        for &idx in &[0usize, 1, 96, 500, 96 * 96 - 1] {
            let c = cell_center(&g, idx);
            assert_eq!(cell_index_at(&g, c), idx);
        }
    }
}
