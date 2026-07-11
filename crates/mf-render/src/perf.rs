//! Optional performance counters for the `MF_PERF` harness in `mf-game`.
//!
//! Always registered (cheap default zeros). Instrumented systems write here
//! when they run; the harness accumulates across the sample window. When
//! `MF_PERF` is unset the harness never reads these, so the only cost is a
//! few atomic stores per instrumented system per frame.

use std::time::Instant;

use bevy::prelude::*;

/// Hot-path CPU timers + visibility-write stats published by instrumented
/// mf-render systems. Units are microseconds spent during the last frame
/// unless noted otherwise. The `MF_PERF` harness resets this each sample.
#[derive(Resource, Debug, Default, Clone)]
pub struct PerfCounters {
    pub building_draw_distance_us: u64,
    pub tree_draw_distance_us: u64,
    pub street_lamp_visibility_us: u64,
    pub road_lod_us: u64,
    pub transit_update_us: u64,
    pub buildings_rebuild_us: u64,
    pub roads_rebuild_us: u64,
    /// Visibility component writes that actually mutated the value.
    pub visibility_mutations: u32,
    /// Visibility compares that skipped the write (already equal).
    pub visibility_skips: u32,
}

/// RAII timer that adds elapsed µs into a `PerfCounters` field on drop.
pub struct PerfSpan<'a> {
    start: Instant,
    target: &'a mut u64,
}

impl<'a> PerfSpan<'a> {
    pub fn start(target: &'a mut u64) -> Self {
        Self {
            start: Instant::now(),
            target,
        }
    }
}

impl Drop for PerfSpan<'_> {
    fn drop(&mut self) {
        *self.target = self.target.saturating_add(self.start.elapsed().as_micros() as u64);
    }
}

/// Write `Visibility` only when the value actually changes — avoids dirtying
/// Bevy change detection (and the visibility propagation pass) every frame
/// when LOD state is stable.
#[inline]
pub fn set_visibility_if_changed(
    vis: &mut Visibility,
    next: Visibility,
    counters: Option<&mut PerfCounters>,
) {
    if *vis == next {
        if let Some(c) = counters {
            c.visibility_skips = c.visibility_skips.saturating_add(1);
        }
        return;
    }
    *vis = next;
    if let Some(c) = counters {
        c.visibility_mutations = c.visibility_mutations.saturating_add(1);
    }
}

pub struct MfPerfCountersPlugin;

impl Plugin for MfPerfCountersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PerfCounters>();
    }
}
