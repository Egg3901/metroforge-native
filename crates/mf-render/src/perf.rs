//! Optional performance counters for the `MF_PERF` harness in `mf-game`.
//!
//! Always registered (cheap default zeros). Instrumented systems write here
//! when they run; the harness accumulates across the sample window. When
//! `MF_PERF` is unset the harness never reads these, so the only cost is a
//! few atomic stores per instrumented system per frame.
//!
//! Fields use atomics so a [`PerfSpan`] can hold a timer target while the
//! same system also bumps visibility mutation/skip counters (and so the
//! resource stays `Sync` for Bevy).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use bevy::prelude::*;

/// Hot-path CPU timers + visibility-write stats published by instrumented
/// mf-render systems. Units are microseconds spent during the last frame
/// unless noted otherwise. The `MF_PERF` harness resets this each sample.
#[derive(Resource, Debug, Default)]
pub struct PerfCounters {
    pub building_draw_distance_us: AtomicU64,
    pub tree_draw_distance_us: AtomicU64,
    pub street_lamp_visibility_us: AtomicU64,
    pub road_lod_us: AtomicU64,
    pub transit_update_us: AtomicU64,
    pub buildings_rebuild_us: AtomicU64,
    pub roads_rebuild_us: AtomicU64,
    /// Visibility component writes that actually mutated the value.
    pub visibility_mutations: AtomicU32,
    /// Visibility compares that were redundant (value already equal).
    pub visibility_skips: AtomicU32,
}

impl PerfCounters {
    pub fn reset(&self) {
        self.building_draw_distance_us.store(0, Ordering::Relaxed);
        self.tree_draw_distance_us.store(0, Ordering::Relaxed);
        self.street_lamp_visibility_us.store(0, Ordering::Relaxed);
        self.road_lod_us.store(0, Ordering::Relaxed);
        self.transit_update_us.store(0, Ordering::Relaxed);
        self.buildings_rebuild_us.store(0, Ordering::Relaxed);
        self.roads_rebuild_us.store(0, Ordering::Relaxed);
        self.visibility_mutations.store(0, Ordering::Relaxed);
        self.visibility_skips.store(0, Ordering::Relaxed);
    }

    pub fn get_us(&self, field: &AtomicU64) -> u64 {
        field.load(Ordering::Relaxed)
    }

    pub fn get_u32(&self, field: &AtomicU32) -> u32 {
        field.load(Ordering::Relaxed)
    }
}

/// RAII timer that adds elapsed µs into an `AtomicU64` on drop.
pub struct PerfSpan<'a> {
    start: Instant,
    target: &'a AtomicU64,
}

impl<'a> PerfSpan<'a> {
    pub fn start(target: &'a AtomicU64) -> Self {
        Self {
            start: Instant::now(),
            target,
        }
    }
}

impl Drop for PerfSpan<'_> {
    fn drop(&mut self) {
        self.target
            .fetch_add(self.start.elapsed().as_micros() as u64, Ordering::Relaxed);
    }
}

/// Write `Visibility` only when the value actually changes — avoids dirtying
/// Bevy change detection (and the visibility propagation pass) every frame
/// when LOD state is stable.
///
/// Set `MF_PERF_FORCE_VIS_WRITE=1` to always assign (baseline A/B for the
/// harness); counters still record whether the write was redundant.
#[inline]
pub fn set_visibility_if_changed(
    vis: &mut Visibility,
    next: Visibility,
    counters: Option<&PerfCounters>,
) {
    let redundant = *vis == next;
    if redundant {
        if let Some(c) = counters {
            c.visibility_skips.fetch_add(1, Ordering::Relaxed);
        }
        if !force_vis_write() {
            return;
        }
    } else if let Some(c) = counters {
        c.visibility_mutations.fetch_add(1, Ordering::Relaxed);
    }
    *vis = next;
}

fn force_vis_write() -> bool {
    static FORCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FORCE.get_or_init(|| std::env::var_os("MF_PERF_FORCE_VIS_WRITE").is_some())
}

pub struct MfPerfCountersPlugin;

impl Plugin for MfPerfCountersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PerfCounters>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_visibility_skips_redundant_writes() {
        let counters = PerfCounters::default();
        let mut vis = Visibility::Visible;
        set_visibility_if_changed(&mut vis, Visibility::Visible, Some(&counters));
        assert_eq!(counters.visibility_skips.load(Ordering::Relaxed), 1);
        assert_eq!(counters.visibility_mutations.load(Ordering::Relaxed), 0);
        set_visibility_if_changed(&mut vis, Visibility::Hidden, Some(&counters));
        assert_eq!(vis, Visibility::Hidden);
        assert_eq!(counters.visibility_mutations.load(Ordering::Relaxed), 1);
    }
}
