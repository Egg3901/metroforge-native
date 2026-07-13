//! `bridges.rs` — Pilot A placement. Replaces the flat deck ribbon of long
//! bridge road spans over water with a scripted suspension-bridge glTF model
//! (see `tools/blender/gen_bridge.py`), scaled to the span.
//!
//! Reads the SAME source data `roads.rs` consumes (`CurrentCity.static_city`)
//! WITHOUT touching `roads.rs`: each `RoadDto` carries `is_bridge` and a
//! polyline of world-meter points. A span qualifies when it is bridge-flagged
//! (or runs over water) AND its over-water chord is longer than
//! `MIN_BRIDGE_SPAN_M`. The single longest qualifying span gets the Brooklyn
//! signature variant (the East-River-scale crossing); the rest get the generic
//! suspension model. Fallback: if no model fits or assets aren't ready, the
//! `roads.rs` ribbon (which we do not remove) remains — so nothing regresses.
//!
//! Coordinate convention (matches roads.rs / terrain.rs): a DTO point (px, py)
//! is world position `Vec3::new(px, height, py)` — world X→Bevy X, world Y→Bevy
//! Z, Bevy Y is up. Models are authored facing +X along the span.

use bevy::prelude::*;
use bevy::scene::SceneRoot;

use mf_state::{CurrentCity, HeightAt};

use crate::models::ModelHandles;

/// Minimum over-water chord (meters) to swap in a suspension model.
const MIN_BRIDGE_SPAN_M: f32 = 250.0;
/// Authored total deck length of the bridge models (gen_bridge.py:
/// SPAN 480 + 2*OVERHANG 90 = 660). Used to scale X to the real span.
const MODEL_DECK_LEN_M: f32 = 660.0;
/// Authored deck width of the bridge models (gen_bridge.py DECK_W).
const MODEL_DECK_W_M: f32 = 26.0;
/// Deck placement height (mirrors roads.rs BRIDGE_DECK_Y).
const BRIDGE_DECK_Y: f32 = 8.0;
/// Target deck width the placed model should present (a wide arterial-ish
/// crossing). Kept fixed for a consistent silhouette.
const TARGET_DECK_W_M: f32 = 30.0;

#[derive(Component)]
struct BridgeModelInstance;

/// Cache signature so we only rebuild placements when the city changes.
#[derive(Resource, Default)]
struct BridgesState {
    signature: Option<(usize, usize)>,
}

pub struct MfBridgesPlugin;

impl Plugin for MfBridgesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BridgesState>().add_systems(
            Update,
            build_bridges_system.in_set(crate::MfRenderSet::Statics),
        );
    }
}

/// Longest contiguous over-water run of a polyline, returned as
/// (start_point, end_point, straight_chord_len). `None` if no water run.
fn over_water_chord(pts: &[Vec2], height_at: &HeightAt) -> Option<(Vec2, Vec2, f32)> {
    let over_water = |p: &Vec2| height_at.sample(p.x, p.y) <= crate::terrain::WATER_LEVEL_Y + 0.01;
    let mut best: Option<(usize, usize, f32)> = None;
    let mut run_start: Option<usize> = None;
    for i in 0..pts.len() {
        if over_water(&pts[i]) {
            run_start.get_or_insert(i);
        } else if let Some(s) = run_start.take() {
            let len = pts[s].distance(pts[i.saturating_sub(1)]);
            if best.is_none_or(|b| len > b.2) {
                best = Some((s, i - 1, len));
            }
        }
    }
    if let Some(s) = run_start {
        let e = pts.len() - 1;
        let len = pts[s].distance(pts[e]);
        if best.is_none_or(|b| len > b.2) {
            best = Some((s, e, len));
        }
    }
    best.map(|(s, e, len)| (pts[s], pts[e], len))
}

#[allow(clippy::type_complexity)]
fn build_bridges_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<mf_state::LatestFields>,
    height_at: Res<HeightAt>,
    handles: Option<Res<ModelHandles>>,
    mut state: ResMut<BridgesState>,
    existing: Query<Entity, With<BridgeModelInstance>>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    // Same race guard as roads.rs: wait for real terrain before sampling water.
    if fields.0.is_none() {
        return;
    }
    let Some(handles) = handles else { return };

    let signature = (
        city_json.roads.len(),
        city_json.roads.iter().map(|r| r.points.len()).sum(),
    );
    if state.signature == Some(signature) {
        return;
    }
    state.signature = Some(signature);

    // Rebuild: clear previously placed models.
    for e in &existing {
        commands.entity(e).despawn();
    }

    // Collect every qualifying span first so we can pick the longest for the
    // Brooklyn signature variant.
    let mut candidates: Vec<(Vec2, Vec2, f32)> = Vec::new();
    for road in &city_json.roads {
        let pts: Vec<Vec2> = road
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        // Qualify on a real water crossing long enough to be worth a model.
        // `is_bridge` is respected as a hint but the span gate is what drives
        // the swap (short bridge-flagged culverts keep the ribbon).
        let Some((a, b, len)) = over_water_chord(&pts, &height_at) else {
            continue;
        };
        if len < MIN_BRIDGE_SPAN_M {
            continue;
        }
        let _ = road.is_bridge;
        candidates.push((a, b, len));
    }

    if candidates.is_empty() {
        return;
    }
    // Longest span → Brooklyn signature variant (East-River-scale crossing).
    let longest_idx = candidates
        .iter()
        .enumerate()
        .max_by(|x, y| x.1 .2.partial_cmp(&y.1 .2).unwrap())
        .map(|(i, _)| i)
        .unwrap();

    for (i, (a, b, len)) in candidates.iter().enumerate() {
        let scene = if i == longest_idx {
            handles.bridge_brooklyn.clone()
        } else {
            handles.bridge_suspension.clone()
        };
        let mid = (*a + *b) * 0.5;
        let dir = (*b - *a).normalize_or_zero();
        // Align authored +X span axis to the chord direction.
        // rotation_y(θ) maps +X → (cosθ, 0, -sinθ); want (dir.x, 0, dir.y).
        let theta = (-dir.y).atan2(dir.x);
        let scale = Vec3::new(
            len / MODEL_DECK_LEN_M,
            1.0,
            TARGET_DECK_W_M / MODEL_DECK_W_M,
        );
        commands.spawn((
            SceneRoot(scene),
            Transform {
                translation: Vec3::new(mid.x, BRIDGE_DECK_Y, mid.y),
                rotation: Quat::from_rotation_y(theta),
                scale,
            },
            Visibility::default(),
            BridgeModelInstance,
        ));
        tracing::info!(
            "mf-render bridges: placed {} span len={:.0}m at ({:.0},{:.0})",
            if i == longest_idx {
                "BROOKLYN"
            } else {
                "suspension"
            },
            len,
            mid.x,
            mid.y
        );
    }
}
