//! Unserved-demand / travel-demand desire-line overlays (ship-plan #25,
//! v0.3): "the single most important insight view" per the web version this
//! ports from. `KeyCode::KeyG` cycles Off -> Demand -> Unserved -> Off
//! (`mf_state::OverlayState`, owned there rather than here so `mf-render`
//! can read it too and fade the transit network's vivid colors while an
//! overlay owns the stage).
//!
//! Two modes, deliberately very different in what they draw:
//!
//! - **Demand**: a client-computed, network-independent gravity model over
//!   `LatestFields`' population/jobs grids ("where the city wants to go",
//!   everywhere, regardless of what's been built). The web version's first
//!   cut of this instead reused the sim's assignment-engine OD pairs, which
//!   only cover trips the engine actually evaluated — heavily biased toward
//!   existing station catchments, so the overlay only ever lit up near track
//!   already built. This mode fixes that by computing its own gravity model
//!   straight from the raw population/jobs fields.
//! - **Unserved**: the sim's own `DemandPayload` (`mf_state::LatestDemand`)
//!   — OD pairs the assignment engine found underserved by the CURRENT
//!   network. Still useful as "trips being lost to cars right now", just no
//!   longer the only lens on offer.
//!
//! Both modes render as elevated arcs (gizmos, immediate-mode, zero asset
//! churn — same technique `tools.rs` uses for build-tool ghosts), not
//! straight ground lines: think flight-route maps, a smooth parabolic bow
//! from A to B, grounded at both ends.
//!
//! `KeyCode::KeyG` is read directly in THIS file rather than `input.rs`:
//! keybind ownership stays with the feature landing it this wave, so a
//! parallel `input.rs` worktree can't collide with this one over the same
//! file.
//!
//! `MfOverlaysPlugin` is deliberately NOT registered in `main.rs` yet
//! (v0.3 integration wires it in, same "one `add_plugins` call away from
//! live" situation `tools.rs`/`build_ui.rs` were in before their waves
//! landed); the blanket allow below mirrors their precedent.
#![allow(dead_code)]

use bevy::prelude::*;
use mf_protocol::ToastTone;
use mf_state::{
    CurrentCity, HeightAt, LatestDemand, LatestFields, OverlayMode, OverlayState, SubwayView,
};

use crate::hud::ToastLog;
use crate::state::AppState;

pub struct MfOverlaysPlugin;

impl Plugin for MfOverlaysPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GravityDemandCache>().add_systems(
            Update,
            (overlay_toggle_system, overlay_draw_system)
                .chain()
                .run_if(in_state(AppState::InGame)),
        );
    }
}

// ---------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------

/// Above this `SubwayView.t`, every overlay backs off entirely — subway
/// view already has its own visual story (art-direction §7), and stacking
/// city-scale arcs on top of it would just read as clutter. Same gate value
/// `reveal_input.rs` uses for the cursor-reveal effect, for the same reason.
const SUBWAY_T_GATE: f32 = 0.3;

/// Unserved mode: cap on how many of the sim's `DemandLine`s get drawn,
/// strongest-by-weight kept, when more arrive than this in one payload.
const UNSERVED_DRAW_CAP: usize = 400;

/// Demand (gravity) mode: coarsen the population/jobs fields down to at
/// most this many cells per axis before hunting for centroids — keeps the
/// centroid search and the O(pop_centroids * job_centroids) pairing cheap
/// regardless of how fine the underlying simulation grid is.
const GRAVITY_TARGET_CELLS: usize = 48;
/// How many population and (separately) jobs hotspots to keep per city.
const GRAVITY_CENTROID_COUNT: usize = 40;
/// Minimum separation between two picked centroids, in COARSE-grid cells
/// (Euclidean), so the top-K picks spread across the map instead of all
/// clumping around one dense downtown block.
const GRAVITY_MIN_SEP_CELLS: f32 = 3.0;
/// Demand (gravity) mode: cap on how many OD pairs get drawn.
const GRAVITY_OD_CAP: usize = 250;

/// Arc rendering: total sample points per drawn line (endpoints included),
/// per the brief's "sample 12 points along the straight line between
/// endpoints".
const ARC_SAMPLES: usize = 12;

// Color ramps (art-direction-adjacent hex triplets, matched to the brief).
const STEEL_BLUE: (u8, u8, u8) = (0x4a, 0x7b, 0xa6);
const AMBER: (u8, u8, u8) = (0xff, 0xbf, 0x00);
const HOT_PINK: (u8, u8, u8) = (0xff, 0x2d, 0x95);

// ---------------------------------------------------------------------
// Shared line type (world-space, f32 — protocol/field data converted into
// this once, up front, so the drawing code never juggles f64 or grid
// indices).
// ---------------------------------------------------------------------

/// One OD pair ready to draw: ground-plane (x, z) endpoints (bevy world
/// axes — matches `tools.rs`'s convention of protocol `y` -> world `z`)
/// plus its raw weight (arbitrary units, only ever compared against other
/// weights from the same batch).
#[derive(Debug, Clone, Copy, PartialEq)]
struct OverlayLine {
    ax: f32,
    az: f32,
    bx: f32,
    bz: f32,
    weight: f32,
}

/// Keeps the `cap` strongest lines by weight, returned in ASCENDING weight
/// order so a simple front-to-back draw loop naturally paints the strongest
/// pairs last (on top) — brief: "stronger pairs draw last".
fn cap_and_order_by_weight(mut lines: Vec<OverlayLine>, cap: usize) -> Vec<OverlayLine> {
    lines.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    lines.truncate(cap);
    lines.reverse();
    lines
}

/// `weight / max_weight`, clamped to 0..1, with a `max_weight <= 0` guard
/// (an all-zero-demand payload, or before any lines have arrived) so the
/// division never produces NaN/Inf that would poison the color ramp.
fn normalized_weight(weight: f32, max_weight: f32) -> f32 {
    if max_weight <= 0.0 {
        0.0
    } else {
        (weight / max_weight).clamp(0.0, 1.0)
    }
}

fn lerp_rgb(low: (u8, u8, u8), high: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let r = low.0 as f32 + (high.0 as f32 - low.0 as f32) * t;
    let g = low.1 as f32 + (high.1 as f32 - low.1 as f32) * t;
    let b = low.2 as f32 + (high.2 as f32 - low.2 as f32) * t;
    Color::srgb(r / 255.0, g / 255.0, b / 255.0)
}

/// Demand mode ramps cool (steel blue, low weight) -> warm (amber, high
/// weight); Unserved keeps the original amber -> hot-pink ramp. Gizmos have
/// no reliable alpha blend, so intensity is carried by hue/color only (no
/// alpha channel used here), per the original brief.
fn overlay_color(mode: OverlayMode, normalized_t: f32) -> Color {
    match mode {
        OverlayMode::Demand => lerp_rgb(STEEL_BLUE, AMBER, normalized_t),
        OverlayMode::Unserved => lerp_rgb(AMBER, HOT_PINK, normalized_t),
        OverlayMode::Off => Color::WHITE, // unreachable: draw system returns early on Off
    }
}

// ---------------------------------------------------------------------
// Arc geometry (pure — no ECS/Bevy types beyond `Vec3`, unit-tested below)
// ---------------------------------------------------------------------

/// Parabola that is 0 at `t=0` and `t=1` and `peak_height` at `t=0.5`:
/// `4*t*(1-t)` is the standard normalized parabolic bump.
fn parabolic_lift(t: f32, peak_height: f32) -> f32 {
    4.0 * t * (1.0 - t) * peak_height
}

/// Arc apex height (meters above the endpoints), scaled by span so short
/// hops get a gentle bow and city-spanning ones get a dramatic one, clamped
/// to a sane band so neither a tiny nor a huge OD pair goes off looking
/// silly (too-flat to read, or towering over the skyline).
fn arc_peak_height(dist_m: f32) -> f32 {
    (dist_m * 0.15).clamp(40.0, 400.0)
}

/// Height at parameter `t` along the arc: linear interpolation between the
/// two (already-grounded) endpoint heights, plus the parabolic lift. At
/// `t=0`/`t=1` the lift is exactly zero so the arc is grounded at both
/// ends; the lift alone carries the bow shape in between.
fn arc_height(t: f32, ground_a: f32, ground_b: f32, peak_height: f32) -> f32 {
    let linear = ground_a + (ground_b - ground_a) * t;
    linear + parabolic_lift(t, peak_height)
}

/// Builds the `ARC_SAMPLES`-point polyline for one OD pair, sampling ground
/// height only at the two endpoints (not along the way — these are flight-
/// route-style bows, not terrain-hugging lines, per the corrected brief).
fn arc_points(line: OverlayLine, height_at: &HeightAt) -> Vec<Vec3> {
    let ground_a = height_at.sample(line.ax, line.az);
    let ground_b = height_at.sample(line.bx, line.bz);
    let dist = ((line.bx - line.ax).powi(2) + (line.bz - line.az).powi(2)).sqrt();
    let peak = arc_peak_height(dist);

    (0..ARC_SAMPLES)
        .map(|i| {
            let t = i as f32 / (ARC_SAMPLES - 1) as f32;
            let x = line.ax + (line.bx - line.ax) * t;
            let z = line.az + (line.bz - line.az) * t;
            let y = arc_height(t, ground_a, ground_b, peak);
            Vec3::new(x, y, z)
        })
        .collect()
}

fn draw_arc(gizmos: &mut Gizmos, height_at: &HeightAt, line: OverlayLine, color: Color) {
    gizmos.linestrip(arc_points(line, height_at), color);
}

// ---------------------------------------------------------------------
// Demand mode: client-side gravity model over LatestFields
// ---------------------------------------------------------------------

/// Cached gravity-model result, recomputed only when `LatestFields.version`
/// changes (per the brief) rather than every frame — the coarsen + top-K +
/// pairing pipeline is cheap for one call per fields update (every 7
/// sim-days) but has no business running 60x/second.
#[derive(Resource, Default)]
struct GravityDemandCache {
    computed_for_version: Option<u32>,
    lines: Vec<OverlayLine>,
    max_weight: f32,
}

/// How many `GRAVITY_TARGET_CELLS`-sized coarse cells fit along the longer
/// field axis; always >= 1 so a degenerate 0-size field can't divide by zero.
fn coarsen_stride(field_w: usize, field_h: usize) -> usize {
    let longest = field_w.max(field_h).max(1);
    (longest as f32 / GRAVITY_TARGET_CELLS as f32)
        .ceil()
        .max(1.0) as usize
}

/// Downsamples a `field_w * field_h` row-major grid into
/// `ceil(field_w/stride) * ceil(field_h/stride)` coarse cells by SUMMING
/// (not point-sampling) every fine cell into its coarse bucket — a coarse
/// cell's value is then the actual total population/jobs mass in that
/// patch of city, which is what the gravity formula below wants, rather
/// than one arbitrarily-chosen fine cell's value standing in for its whole
/// neighborhood.
fn coarsen_sum(
    values: &[f32],
    field_w: usize,
    field_h: usize,
    stride: usize,
) -> (Vec<f32>, usize, usize) {
    let coarse_w = field_w.div_ceil(stride);
    let coarse_h = field_h.div_ceil(stride);
    let mut out = vec![0.0f32; coarse_w * coarse_h];
    for row in 0..field_h {
        let cy = row / stride;
        for col in 0..field_w {
            let cx = col / stride;
            out[cy * coarse_w + cx] += values[row * field_w + col];
        }
    }
    (out, coarse_w, coarse_h)
}

/// Greedy top-K local maxima over a coarse grid: repeatedly takes the
/// highest remaining value whose cell is at least `min_sep` cells
/// (Euclidean, coarse-grid units) from every centroid already picked, so
/// hotspots spread across the map instead of clumping around one dense
/// block. Zero-value cells are never picked (nothing there to be a center
/// of). Returns `(col, row, value)` triples, highest value first.
fn top_k_centroids(
    grid: &[f32],
    w: usize,
    h: usize,
    k: usize,
    min_sep: f32,
) -> Vec<(usize, usize, f32)> {
    let mut candidates: Vec<(usize, usize, f32)> = (0..h)
        .flat_map(|row| (0..w).map(move |col| (col, row)))
        .map(|(col, row)| (col, row, grid[row * w + col]))
        .filter(|(_, _, v)| *v > 0.0)
        .collect();
    candidates.sort_by(|a, b| b.2.total_cmp(&a.2));

    let min_sep_sq = min_sep * min_sep;
    let mut picked: Vec<(usize, usize, f32)> = Vec::with_capacity(k);
    for cand in candidates {
        if picked.len() >= k {
            break;
        }
        let far_enough = picked.iter().all(|(pc, pr, _)| {
            let dx = pc.abs_diff(cand.0) as f32;
            let dy = pr.abs_diff(cand.1) as f32;
            dx * dx + dy * dy >= min_sep_sq
        });
        if far_enough {
            picked.push(cand);
        }
    }
    picked
}

/// Gravity-model OD weight: proportional to both ends' mass, inverse-square
/// in distance (in km, `+1` so co-located centroids don't divide by zero).
fn gravity_weight(pop_mass: f32, job_mass: f32, dist_m: f32) -> f32 {
    let denom = 1.0 + dist_m / 1000.0;
    (pop_mass * job_mass) / (denom * denom)
}

/// Full gravity-model pipeline: coarsen both grids, pick population and
/// jobs hotspots independently, pair every population centroid with every
/// jobs centroid, keep the strongest `GRAVITY_OD_CAP`. Pure (no ECS types)
/// so it's directly unit-testable; `refresh_gravity_cache` below is the
/// thin ECS-facing wrapper that calls this only when `LatestFields` changed.
fn compute_gravity_demand(
    population: &[f32],
    jobs: &[f32],
    field_w: usize,
    field_h: usize,
    cell_size: f32,
    origin_x: f32,
    origin_y: f32,
) -> (Vec<OverlayLine>, f32) {
    if field_w == 0 || field_h == 0 {
        return (Vec::new(), 0.0);
    }
    let stride = coarsen_stride(field_w, field_h);
    let (pop_grid, coarse_w, coarse_h) = coarsen_sum(population, field_w, field_h, stride);
    let (job_grid, _, _) = coarsen_sum(jobs, field_w, field_h, stride);

    let pop_centroids = top_k_centroids(
        &pop_grid,
        coarse_w,
        coarse_h,
        GRAVITY_CENTROID_COUNT,
        GRAVITY_MIN_SEP_CELLS,
    );
    let job_centroids = top_k_centroids(
        &job_grid,
        coarse_w,
        coarse_h,
        GRAVITY_CENTROID_COUNT,
        GRAVITY_MIN_SEP_CELLS,
    );

    let coarse_cell_size = cell_size * stride as f32;
    let world_pos = |col: usize, row: usize| -> (f32, f32) {
        (
            origin_x + (col as f32 + 0.5) * coarse_cell_size,
            origin_y + (row as f32 + 0.5) * coarse_cell_size,
        )
    };

    let mut pairs = Vec::with_capacity(pop_centroids.len() * job_centroids.len());
    for &(pc, pr, pop_mass) in &pop_centroids {
        let (ax, az) = world_pos(pc, pr);
        for &(jc, jr, job_mass) in &job_centroids {
            let (bx, bz) = world_pos(jc, jr);
            let dist = ((bx - ax).powi(2) + (bz - az).powi(2)).sqrt();
            let weight = gravity_weight(pop_mass, job_mass, dist);
            if weight > 0.0 {
                pairs.push(OverlayLine {
                    ax,
                    az,
                    bx,
                    bz,
                    weight,
                });
            }
        }
    }

    let ordered = cap_and_order_by_weight(pairs, GRAVITY_OD_CAP);
    // Ascending order (see `cap_and_order_by_weight`) => the last element,
    // if any, holds the maximum.
    let max_weight = ordered.last().map(|l| l.weight).unwrap_or(0.0);
    (ordered, max_weight)
}

/// Recomputes `cache` from `fields`/`city` iff `fields.version` moved on
/// since the last computation (or nothing has been computed yet). No-ops
/// (keeping the last good cache) if the static city or fields aren't loaded
/// yet, or if array lengths don't match `field_w * field_h` — a defensive
/// guard against a mid-load partial state, not an expected steady-state path.
fn refresh_gravity_cache(
    cache: &mut GravityDemandCache,
    fields: &LatestFields,
    city: &CurrentCity,
) {
    let Some(f) = &fields.0 else {
        return;
    };
    if cache.computed_for_version == Some(f.version) {
        return;
    }
    let Some(static_city) = &city.static_city else {
        return;
    };
    let field_w = static_city.field_w as usize;
    let field_h = static_city.field_h as usize;
    let expected_len = field_w * field_h;
    if field_w == 0
        || field_h == 0
        || f.population.len() != expected_len
        || f.jobs.len() != expected_len
    {
        return;
    }

    let (lines, max_weight) = compute_gravity_demand(
        &f.population,
        &f.jobs,
        field_w,
        field_h,
        static_city.cell_size as f32,
        static_city.origin_x as f32,
        static_city.origin_y as f32,
    );
    let line_count = lines.len();
    cache.lines = lines;
    cache.max_weight = max_weight;
    cache.computed_for_version = Some(f.version);
    tracing::info!(
        fields_version = f.version,
        od_pairs = line_count,
        "demand overlay: recomputed gravity model"
    );
}

// ---------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------

/// `KeyCode::KeyG` cycles the overlay Off -> Demand -> Unserved -> Off, and
/// pushes a one-time (per session, per mode) explanatory toast into
/// `hud::ToastLog` the first time each of Demand/Unserved is entered —
/// toolbar integration lands later; this wave's "hint" is just the toast.
fn overlay_toggle_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut overlay: ResMut<OverlayState>,
    mut toasts: ResMut<ToastLog>,
    mut demand_toast_shown: Local<bool>,
    mut unserved_toast_shown: Local<bool>,
) {
    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }
    overlay.cycle();
    let s = crate::strings::current();
    match overlay.mode {
        OverlayMode::Demand if !*demand_toast_shown => {
            toasts
                .0
                .push((s.demand_overlay_toast.to_string(), ToastTone::Info));
            *demand_toast_shown = true;
        }
        OverlayMode::Unserved if !*unserved_toast_shown => {
            toasts
                .0
                .push((s.unserved_overlay_toast.to_string(), ToastTone::Info));
            *unserved_toast_shown = true;
        }
        _ => {}
    }
}

/// Draws whichever overlay is active this frame. Gated (at plugin
/// registration) to `InGame`, and here to "an overlay is actually on" and
/// "subway view hasn't taken over" (`SUBWAY_T_GATE`).
#[allow(clippy::too_many_arguments)]
fn overlay_draw_system(
    mut gizmos: Gizmos,
    overlay: Res<OverlayState>,
    subway: Res<SubwayView>,
    height_at: Res<HeightAt>,
    demand: Res<LatestDemand>,
    fields: Res<LatestFields>,
    city: Res<CurrentCity>,
    mut gravity_cache: ResMut<GravityDemandCache>,
    mut logged_unserved_count: Local<Option<usize>>,
) {
    if overlay.mode == OverlayMode::Off {
        return;
    }
    if subway.t > SUBWAY_T_GATE {
        return;
    }

    if overlay.mode == OverlayMode::Demand {
        refresh_gravity_cache(&mut gravity_cache, &fields, &city);
        for &line in &gravity_cache.lines {
            let t = normalized_weight(line.weight, gravity_cache.max_weight);
            draw_arc(
                &mut gizmos,
                &height_at,
                line,
                overlay_color(OverlayMode::Demand, t),
            );
        }
        return;
    }

    // OverlayMode::Unserved
    let Some(payload) = &demand.0 else {
        return;
    };
    let raw_count = payload.lines.len();
    if *logged_unserved_count != Some(raw_count) {
        if raw_count > UNSERVED_DRAW_CAP {
            tracing::info!(
                raw_count,
                cap = UNSERVED_DRAW_CAP,
                "unserved overlay: capping to strongest lines"
            );
        }
        *logged_unserved_count = Some(raw_count);
    }
    let lines: Vec<OverlayLine> = payload
        .lines
        .iter()
        .map(|l| OverlayLine {
            ax: l.x1 as f32,
            az: l.y1 as f32,
            bx: l.x2 as f32,
            bz: l.y2 as f32,
            weight: l.weight as f32,
        })
        .collect();
    let ordered = cap_and_order_by_weight(lines, UNSERVED_DRAW_CAP);
    let max_weight = payload.max_weight as f32;
    for &line in &ordered {
        let t = normalized_weight(line.weight, max_weight);
        draw_arc(
            &mut gizmos,
            &height_at,
            line,
            overlay_color(OverlayMode::Unserved, t),
        );
    }
}

// ---------------------------------------------------------------------
// Pure-helper tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn line(weight: f32) -> OverlayLine {
        OverlayLine {
            ax: 0.0,
            az: 0.0,
            bx: 1.0,
            bz: 1.0,
            weight,
        }
    }

    #[test]
    fn cap_and_order_keeps_strongest_ascending() {
        let lines = vec![line(1.0), line(5.0), line(3.0), line(2.0), line(4.0)];
        let ordered = cap_and_order_by_weight(lines, 3);
        let weights: Vec<f32> = ordered.iter().map(|l| l.weight).collect();
        // Strongest 3 kept (5,4,3), ascending so the draw loop paints 5.0 last.
        assert_eq!(weights, vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn cap_and_order_is_a_no_op_under_the_cap() {
        let lines = vec![line(2.0), line(1.0)];
        let ordered = cap_and_order_by_weight(lines, 10);
        let weights: Vec<f32> = ordered.iter().map(|l| l.weight).collect();
        assert_eq!(weights, vec![1.0, 2.0]);
    }

    #[test]
    fn normalized_weight_guards_zero_max() {
        assert_eq!(normalized_weight(5.0, 0.0), 0.0);
        assert_eq!(normalized_weight(5.0, -1.0), 0.0);
    }

    #[test]
    fn normalized_weight_clamps_and_scales() {
        assert_eq!(normalized_weight(0.0, 10.0), 0.0);
        assert_eq!(normalized_weight(10.0, 10.0), 1.0);
        assert!((normalized_weight(5.0, 10.0) - 0.5).abs() < 1e-6);
        // A weight above max (shouldn't happen upstream, but must not blow
        // past 1.0 and invert the ramp if it does).
        assert_eq!(normalized_weight(20.0, 10.0), 1.0);
    }

    #[test]
    fn demand_color_ramp_endpoints() {
        assert_eq!(
            overlay_color(OverlayMode::Demand, 0.0),
            lerp_rgb(STEEL_BLUE, AMBER, 0.0)
        );
        assert_eq!(
            overlay_color(OverlayMode::Demand, 1.0),
            lerp_rgb(STEEL_BLUE, AMBER, 1.0)
        );
        let low = overlay_color(OverlayMode::Demand, 0.0);
        let high = overlay_color(OverlayMode::Demand, 1.0);
        assert_ne!(low, high);
    }

    #[test]
    fn unserved_color_ramp_endpoints() {
        let low = overlay_color(OverlayMode::Unserved, 0.0);
        let high = overlay_color(OverlayMode::Unserved, 1.0);
        assert_eq!(low, lerp_rgb(AMBER, HOT_PINK, 0.0));
        assert_eq!(high, lerp_rgb(AMBER, HOT_PINK, 1.0));
        assert_ne!(low, high);
    }

    #[test]
    fn arc_peak_height_clamps_at_the_floor_for_short_hops() {
        assert_eq!(arc_peak_height(10.0), 40.0);
    }

    #[test]
    fn arc_peak_height_clamps_at_the_ceiling_for_long_hops() {
        assert_eq!(arc_peak_height(5000.0), 400.0);
    }

    #[test]
    fn arc_peak_height_scales_linearly_mid_range() {
        assert!((arc_peak_height(1000.0) - 150.0).abs() < 1e-4);
    }

    #[test]
    fn arc_height_is_grounded_at_both_endpoints() {
        assert_eq!(arc_height(0.0, 10.0, 20.0, 100.0), 10.0);
        assert_eq!(arc_height(1.0, 10.0, 20.0, 100.0), 20.0);
    }

    #[test]
    fn arc_height_peaks_at_the_midpoint() {
        // At t=0.5 the parabolic lift term is exactly `peak_height`, added
        // on top of the (here, equal) endpoint heights.
        let h = arc_height(0.5, 50.0, 50.0, 120.0);
        assert!((h - 170.0).abs() < 1e-4);
    }

    #[test]
    fn arc_points_has_expected_sample_count_and_grounded_ends() {
        let height_at = HeightAt::default(); // flat ground at y=0
        let pts = arc_points(line(1.0), &height_at);
        assert_eq!(pts.len(), ARC_SAMPLES);
        assert_eq!(pts.first().unwrap().y, 0.0);
        assert_eq!(pts.last().unwrap().y, 0.0);
        // Interior points should be lifted above ground.
        assert!(pts[ARC_SAMPLES / 2].y > 0.0);
    }

    #[test]
    fn coarsen_stride_never_zero_and_shrinks_large_fields() {
        assert_eq!(coarsen_stride(0, 0), 1);
        assert_eq!(coarsen_stride(48, 48), 1);
        assert_eq!(coarsen_stride(96, 48), 2);
    }

    #[test]
    fn coarsen_sum_totals_match_input() {
        // 4x4 field of all-1.0 cells, coarsened with stride 2 -> 2x2 grid,
        // each coarse cell summing 4 fine cells -> every coarse cell = 4.0.
        let values = vec![1.0f32; 16];
        let (coarse, cw, ch) = coarsen_sum(&values, 4, 4, 2);
        assert_eq!((cw, ch), (2, 2));
        assert!(coarse.iter().all(|&v| (v - 4.0).abs() < 1e-6));
    }

    #[test]
    fn top_k_centroids_respects_min_separation() {
        // 5x1 row: two adjacent hotspots (cols 2 and 3) plus one far one
        // (col 0). With min_sep=2, picking col 3 (the taller of the
        // adjacent pair) should exclude col 2 but still allow col 0.
        let grid = vec![5.0, 0.0, 8.0, 9.0, 0.0];
        let picked = top_k_centroids(&grid, 5, 1, 3, 2.0);
        let cols: Vec<usize> = picked.iter().map(|(c, _, _)| *c).collect();
        assert!(cols.contains(&3), "strongest cell must be picked: {cols:?}");
        assert!(
            !cols.contains(&2),
            "col 2 is within min_sep of col 3: {cols:?}"
        );
        assert!(
            cols.contains(&0),
            "col 0 is far enough to also be picked: {cols:?}"
        );
    }

    #[test]
    fn top_k_centroids_ignores_zero_cells() {
        let grid = vec![0.0, 0.0, 0.0, 0.0];
        let picked = top_k_centroids(&grid, 2, 2, 5, 1.0);
        assert!(picked.is_empty());
    }

    #[test]
    fn gravity_weight_decays_with_distance() {
        let close = gravity_weight(100.0, 100.0, 0.0);
        let far = gravity_weight(100.0, 100.0, 5000.0);
        assert!(close > far, "closer pair must weigh more: {close} vs {far}");
        assert!(close > 0.0 && far > 0.0);
    }

    #[test]
    fn gravity_weight_scales_with_mass() {
        let small = gravity_weight(10.0, 10.0, 1000.0);
        let big = gravity_weight(100.0, 100.0, 1000.0);
        assert!(big > small);
    }

    #[test]
    fn compute_gravity_demand_pairs_hotspots_across_a_simple_field() {
        // 8x8 field: a population hotspot in one corner, a jobs hotspot in
        // the opposite corner, everything else zero. Should produce at
        // least one OD pair connecting them with a positive max weight.
        let w = 8;
        let h = 8;
        let mut population = vec![0.0f32; w * h];
        let mut jobs = vec![0.0f32; w * h];
        population[0] = 500.0; // top-left
        jobs[w * h - 1] = 500.0; // bottom-right
        let (lines, max_weight) = compute_gravity_demand(&population, &jobs, w, h, 10.0, 0.0, 0.0);
        assert!(!lines.is_empty());
        assert!(max_weight > 0.0);
    }

    #[test]
    fn compute_gravity_demand_handles_empty_field() {
        let (lines, max_weight) = compute_gravity_demand(&[], &[], 0, 0, 10.0, 0.0, 0.0);
        assert!(lines.is_empty());
        assert_eq!(max_weight, 0.0);
    }
}
