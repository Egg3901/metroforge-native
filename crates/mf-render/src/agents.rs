//! Pedestrian/passenger agents (spec §3.3 `agents.rs`): one mesh of small
//! flat quads, rebuilt from `LatestFrame`'s stride-3 agent array whenever it
//! changes, capped per tier (`EffectiveKnobs.agent_cap`; potato = 0,
//! i.e. agents are entirely disabled on the weakest tier).
//!
//! One `Mesh` asset lives for the app's whole life (created once); a rebuild
//! overwrites its attributes/indices in place via `Assets<Mesh>::get_mut`
//! rather than `meshes.add`-ing a fresh asset every pass — the latter would
//! be a brand-new GPU buffer allocation + upload plus an old-asset teardown
//! every single frame, when `LatestFrame` (and thus the agent positions)
//! only actually changes at the sim's tick rate.

use bevy::prelude::*;
use bevy::render::mesh::PrimitiveTopology;
use bevy::render::render_asset::RenderAssetUsages;

use mf_protocol::UiState;
use mf_state::{EffectiveKnobs, HeightAt, LatestFrame, LatestUi};

use crate::mesh_utils::MeshBuffers;
use crate::RenderCacheStats;

const AGENT_SIZE: f32 = 2.2;
const AGENT_Y_OFFSET: f32 = 0.8;

/// Cohort passenger swell (v0.9 B4): floor on how thin the visible crowd goes
/// at the quietest hour (2am). Never zero, so a live network still shows a
/// skeleton crowd overnight rather than an empty city.
const SWELL_MIN: f32 = 0.22;
/// Demand-factor window mapped onto `[SWELL_MIN, 1.0]`. `cohort_demand.factor`
/// (and `demand_factor`) have a daily mean of 1.0, dipping well under overnight
/// and peaking ~1.3+ at the AM/PM rushes.
const SWELL_DEMAND_LOW: f32 = 0.45;
const SWELL_DEMAND_HIGH: f32 = 1.30;

/// Fraction of the sampled agent population to draw right now, sourced from the
/// wire's cohort-driven demand shape so the visible crowd swells at the peaks
/// and thins overnight. Preference order (richest signal first):
///   1. `cohort_demand.factor` — the live cohort demand multiplier,
///   2. `demand_factor` — the sim-depth peak/off-peak multiplier,
///   3. `service_period` — the coarse period id (amPeak/midday/.../night),
///   4. no signal -> 1.0 (draw the full sample, pre-v0.9 behavior).
///
/// Pure (takes the decoded `UiState`, returns a scalar) so the mapping is
/// unit-testable and deterministic — no RNG, no wall-clock.
fn swell_factor(ui: Option<&UiState>) -> f32 {
    let Some(u) = ui else {
        return 1.0;
    };
    if let Some(cd) = &u.cohort_demand {
        return swell_from_demand(cd.factor);
    }
    if let Some(df) = u.demand_factor {
        return swell_from_demand(df);
    }
    if let Some(period) = u.service_period.as_deref() {
        return swell_from_period(period);
    }
    1.0
}

/// Map a demand multiplier (daily mean 1.0) onto `[SWELL_MIN, 1.0]`.
fn swell_from_demand(d: f64) -> f32 {
    let t = (d as f32 - SWELL_DEMAND_LOW) / (SWELL_DEMAND_HIGH - SWELL_DEMAND_LOW);
    SWELL_MIN + t.clamp(0.0, 1.0) * (1.0 - SWELL_MIN)
}

/// Coarse fallback when only the service-period id is on the wire.
fn swell_from_period(period: &str) -> f32 {
    match period {
        "amPeak" | "pmPeak" => 1.0,
        "midday" => 0.72,
        "evening" => 0.55,
        "night" => SWELL_MIN,
        _ => 1.0,
    }
}

// Phase: 0 walk, 1 ride, 2 wait (spec §1.2).
// Art direction: vivid color is reserved for the transit network — agents
// stay greyscale, with phase readable via brightness only.
fn phase_color(phase: f32) -> Color {
    if phase < 0.5 {
        Color::srgb(0.55, 0.57, 0.6) // walk: mid grey
    } else if phase < 1.5 {
        Color::srgb(0.72, 0.74, 0.76) // ride: lighter grey
    } else {
        Color::srgb(0.40, 0.42, 0.45) // wait: darker grey
    }
}

pub struct MfAgentsPlugin;

impl Plugin for MfAgentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AgentsState>().add_systems(
            Update,
            (
                update_agents_system,
                apply_quality_to_agents_material_system,
            )
                .in_set(crate::MfRenderSet::Dynamic),
        );
    }
}

#[derive(Resource, Default)]
struct AgentsState {
    entity: Option<Entity>,
    material: Option<Handle<StandardMaterial>>,
    /// Created lazily the first time there's at least one agent to draw, then
    /// reused for the rest of the app's life — its attributes are overwritten
    /// in place on each rebuild instead of allocating a new `Mesh` asset.
    mesh: Option<Handle<Mesh>>,
    /// CPU scratch reused across ~20 Hz rebuilds (cleared, not reallocated).
    scratch: MeshBuffers,
    /// Last cohort-swell fraction applied, quantized to 1/64 buckets. `LatestUi`
    /// changes every 2 Hz tick (so `is_changed()` is always true and would
    /// defeat the frame-rate skip gate); only a bucket step in the swell
    /// actually changes how many agents to draw, so this gates the rebuild.
    last_swell_bucket: Option<u8>,
}

/// Flip the shared agents material's `unlit` flag when the quality tier
/// changes, without waiting for the next `update_agents_system` rebuild.
fn apply_quality_to_agents_material_system(
    effective: Res<EffectiveKnobs>,
    state: Res<AgentsState>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !effective.is_changed() {
        return;
    }
    let Some(handle) = &state.material else {
        return;
    };
    if let Some(mat) = materials.get_mut(handle) {
        mat.unlit = effective.0.unlit_material;
    }
}

#[allow(clippy::too_many_arguments)]
fn update_agents_system(
    frame: Res<LatestFrame>,
    ui: Res<LatestUi>,
    height_at: Res<HeightAt>,
    effective: Res<EffectiveKnobs>,
    mut state: ResMut<AgentsState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut visibility: Query<&mut Visibility>,
    mut stats: ResMut<RenderCacheStats>,
) {
    let cap = effective.0.agent_cap as usize;
    let entity = if let Some(e) = state.entity {
        e
    } else {
        // Flat +Y quads viewed only from above (top-down camera) — verified
        // CCW-from-+Y below (fixed to match), so single-sided is correct.
        let mat = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: effective.0.unlit_material,
            ..default()
        });
        let e = commands
            .spawn((
                Mesh3d(Handle::default()),
                MeshMaterial3d(mat.clone()),
                Transform::IDENTITY,
                Visibility::Hidden,
                Name::new("agents"),
            ))
            .id();
        state.entity = Some(e);
        state.material = Some(mat);
        e
    };

    if cap == 0 {
        if let Ok(mut vis) = visibility.get_mut(entity) {
            *vis = Visibility::Hidden;
        }
        stats.agent_entities = 0;
        return;
    }

    // Cohort passenger swell (v0.9 B4): fraction of the sampled population to
    // draw right now, from the wire's cohort demand shape. Quantized to 1/64 so
    // a barely-moved factor doesn't rebuild the mesh every 2 Hz UI tick.
    let swell = swell_factor(ui.0.as_ref());
    let swell_bucket = (swell * 64.0).round() as u8;
    let swell_changed = state.last_swell_bucket != Some(swell_bucket);

    // `LatestFrame` arrives at the sim's tick rate, well under render frame
    // rate; an `EffectiveKnobs` change can move `cap` (and thus `draw_count`)
    // without any new frame data, and a swell-bucket step moves how many of the
    // sample we draw without any new positions. None changing means the agent
    // mesh can't possibly need to look any different from what's already built.
    if !frame.is_changed() && !effective.is_changed() && !swell_changed {
        return;
    }
    state.last_swell_bucket = Some(swell_bucket);
    let Some(f) = &frame.0 else {
        return;
    };
    // Prefix subsample by the swell fraction: agents 0..draw_count of the
    // sampled array. Deterministic (index-based, no RNG), so a peak crowd is a
    // superset of the off-peak one — passengers swell/thin, never teleport.
    let sampled = (f.agent_count as usize).min(cap);
    let draw_count = ((sampled as f32 * swell).round() as usize).min(sampled);
    if draw_count == 0 {
        if let Ok(mut vis) = visibility.get_mut(entity) {
            *vis = Visibility::Hidden;
        }
        return;
    }

    // Pre-size once for the current cap (4 verts / 6 indices per agent quad).
    state.scratch.ensure_capacity(cap * 4, cap * 6);
    state.scratch.clear();
    let half = AGENT_SIZE * 0.5;
    for i in 0..draw_count {
        let base = i * 3;
        let (Some(&x), Some(&y), Some(&phase)) = (
            f.agents.get(base),
            f.agents.get(base + 1),
            f.agents.get(base + 2),
        ) else {
            break;
        };
        let ground_y = height_at.sample(x, y) + AGENT_Y_OFFSET;
        let color = phase_color(phase);
        // Winding vs the declared `+Y` normal: same corner pattern as
        // `terrain.rs`'s grid quad ((x0,z0),(x1,z0),(x1,z1),(x0,z1)), which
        // works out to a `-Y` cross product (CCW from below, not above) —
        // see the comment there for the derivation. Swapping the middle two
        // corners to ((x0,z0),(x0,z1),(x1,z1),(x1,z0)) reverses the quad and
        // flips the cross product to `+Y`, matching the declared normal.
        state.scratch.push_flat_quad(
            Vec3::new(x - half, ground_y, y - half),
            Vec3::new(x - half, ground_y, y + half),
            Vec3::new(x + half, ground_y, y + half),
            Vec3::new(x + half, ground_y, y - half),
            Vec3::Y,
            color,
        );
    }

    // Transplant scratch attributes into the one long-lived mesh asset via
    // `get_mut` so this rebuild re-uploads existing GPU buffers instead of
    // allocating fresh ones and tearing down the old.
    let is_new_mesh = state.mesh.is_none();
    let mesh_handle = state
        .mesh
        .get_or_insert_with(|| {
            meshes.add(Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            ))
        })
        .clone();
    if is_new_mesh {
        commands.entity(entity).insert(Mesh3d(mesh_handle.clone()));
    }
    if let Some(mesh) = meshes.get_mut(&mesh_handle) {
        state.scratch.apply_to_mesh(mesh);
    }
    if let Ok(mut vis) = visibility.get_mut(entity) {
        *vis = Visibility::Visible;
    }
    stats.agent_entities = if state.entity.is_some() { 1 } else { 0 };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_ui_draws_full_sample() {
        assert_eq!(swell_factor(None), 1.0);
    }

    #[test]
    fn demand_peak_saturates_to_one() {
        // At/above the high window, draw the full sample.
        assert!((swell_from_demand(1.30) - 1.0).abs() < 1e-4);
        assert!((swell_from_demand(1.8) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn demand_trough_floors_at_swell_min() {
        // At/below the low window, thin to the overnight floor (never zero).
        assert!((swell_from_demand(0.45) - SWELL_MIN).abs() < 1e-4);
        assert!((swell_from_demand(0.1) - SWELL_MIN).abs() < 1e-4);
    }

    #[test]
    fn demand_is_monotonic_between_trough_and_peak() {
        let a = swell_from_demand(0.7);
        let b = swell_from_demand(1.0);
        let c = swell_from_demand(1.2);
        assert!(SWELL_MIN <= a && a < b && b < c && c <= 1.0);
    }

    #[test]
    fn period_fallback_peaks_full_and_night_thin() {
        assert_eq!(swell_from_period("amPeak"), 1.0);
        assert_eq!(swell_from_period("pmPeak"), 1.0);
        assert!(swell_from_period("midday") > swell_from_period("evening"));
        assert!((swell_from_period("night") - SWELL_MIN).abs() < 1e-4);
        // Unknown period is treated as full (never accidentally blanks the city).
        assert_eq!(swell_from_period("brunch"), 1.0);
    }
}
