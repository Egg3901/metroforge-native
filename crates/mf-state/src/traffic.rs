//! Latest sim traffic frame (protocol msgType=3): a congestion density grid
//! plus hotspot list. Received by `mf-net`, mirrored here, and consumed by
//! `mf-game`'s Traffic overlay, which paints congestion onto the road network.

use bevy_ecs::prelude::*;
use mf_protocol::Traffic;

/// Most recent traffic frame from the sim, or `None` until the first arrives.
#[derive(Resource, Debug, Clone, Default)]
pub struct LatestTraffic(pub Option<Traffic>);

impl LatestTraffic {
    /// Max density across the grid, for normalizing per-cell values to `0..1`.
    /// `0.0` when there is no frame or the grid is flat/empty.
    pub fn max_density(&self) -> f32 {
        self.0
            .as_ref()
            .map(|t| t.values.iter().copied().fold(0.0_f32, f32::max))
            .unwrap_or(0.0)
    }

    /// Bilinear-free nearest-cell density sample at world `(x, z)`. `0.0` when
    /// there is no frame or the grid has no extent.
    pub fn density_at(&self, x: f32, z: f32) -> f32 {
        let Some(t) = &self.0 else {
            return 0.0;
        };
        if t.w == 0 || t.h == 0 || t.cell_size <= 0.0 {
            return 0.0;
        }
        let cx = (((x - t.origin_x) / t.cell_size) as i32).clamp(0, t.w as i32 - 1) as usize;
        let cy = (((z - t.origin_y) / t.cell_size) as i32).clamp(0, t.h as i32 - 1) as usize;
        t.values.get(cy * t.w as usize + cx).copied().unwrap_or(0.0)
    }
}
