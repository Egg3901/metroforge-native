//! `bridges.rs` — long-span bridge model placement (issue #144).
//!
//! Replaces the flat deck ribbon of a qualifying over-water span with a
//! scripted Blender glTF bridge (see `tools/blender/`), scaled to the span.
//!
//! ONE style per span, picked by the over-water chord length ([`plan_bridge_placements`]):
//!   * `> SUSPENSION_MIN_M` (350m) over water → suspension family. The single
//!     longest such span gets the **Brooklyn** signature variant (the East-River
//!     -scale crossing) until name-matching data lands; the rest get the plain
//!     suspension model.
//!   * `TRUSS_MIN_M..=SUSPENSION_MIN_M` (120–350m) → through-**truss** model, so
//!     mid-length crossings stop using flat deck ribbons.
//!   * shorter → no model; the `roads.rs` deck ribbon stays.
//!
//! Never mixes models within one span. The placement decision is a pure
//! function of `(city, terrain)`; `roads.rs` calls the SAME function to
//! suppress its ribbon + piers under a placed model (the double-render fix),
//! so the two systems can never disagree regardless of schedule order.
//!
//! Coordinate convention (matches roads.rs / terrain.rs): a DTO point (px, py)
//! is world `Vec3::new(px, height, py)`. Models are authored facing +X along
//! the span, deck top at y≈0, and are scaled X→real span, Z→target deck width.
//! Tower/cable HEIGHT (model Y) is left at scale 1 so verticals are never
//! stretched — only the deck/span axis stretches (mandate #144).

use bevy::prelude::*;
use bevy::scene::SceneRoot;

use mf_state::{CurrentCity, HeightAt};

use crate::models::ModelHandles;

/// Longest over-water chord (m) that still uses the flat ribbon: below this a
/// crossing is left to `roads.rs`.
const TRUSS_MIN_M: f32 = 120.0;
/// Chord length (m) at/above which a crossing uses the suspension family
/// instead of the truss model.
const SUSPENSION_MIN_M: f32 = 350.0;

/// Authored total deck length of the suspension models (gen_bridge.py:
/// SPAN 486 + 2*OVERHANG 90 = 666).
const SUSP_DECK_LEN_M: f32 = 666.0;
/// Authored deck width of the suspension models (gen_bridge.py DECK_W).
const SUSP_DECK_W_M: f32 = 26.0;
/// Authored total deck length of the truss model (gen_truss.py: SPAN 180 + 8).
const TRUSS_DECK_LEN_M: f32 = 188.0;
/// Authored deck width of the truss model (gen_truss.py DECK_W).
const TRUSS_DECK_W_M: f32 = 16.0;

/// Deck placement height (mirrors roads.rs BRIDGE_DECK_Y).
const BRIDGE_DECK_Y: f32 = 8.0;
/// Target presented deck width for a suspension crossing (wide arterial feel).
const SUSP_TARGET_W_M: f32 = 30.0;
/// Target presented deck width for a truss crossing (narrower).
const TRUSS_TARGET_W_M: f32 = 18.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BridgeKind {
    Suspension,
    Brooklyn,
    Truss,
}

/// One decided bridge placement: which road it covers, its style, and the
/// over-water chord endpoints/length it spans.
#[derive(Clone, Copy, Debug)]
pub struct BridgePlacement {
    pub road_idx: usize,
    pub kind: BridgeKind,
    pub a: Vec2,
    pub b: Vec2,
    pub len: f32,
}

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

/// Decide, per road, whether a bridge MODEL covers its over-water span and
/// which style. Pure function of `(city, terrain)` — called by BOTH the model
/// placer here and the `roads.rs` ribbon suppressor, so they never disagree.
pub fn plan_bridge_placements(
    city_json: &mf_protocol::StaticCityJson,
    height_at: &HeightAt,
) -> Vec<BridgePlacement> {
    let mut out: Vec<BridgePlacement> = Vec::new();
    let mut longest_susp: Option<(usize, f32)> = None; // (out index, len)

    for (road_idx, road) in city_json.roads.iter().enumerate() {
        let pts: Vec<Vec2> = road
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        let Some((a, b, len)) = over_water_chord(&pts, height_at) else {
            continue;
        };
        let _ = road.is_bridge; // a hint; the span-length gate is authoritative
        let kind = if len >= SUSPENSION_MIN_M {
            BridgeKind::Suspension
        } else if len >= TRUSS_MIN_M {
            BridgeKind::Truss
        } else {
            continue; // short crossing: leave the flat ribbon to roads.rs
        };
        if kind == BridgeKind::Suspension && longest_susp.is_none_or(|(_, bl)| len > bl) {
            longest_susp = Some((out.len(), len));
        }
        out.push(BridgePlacement {
            road_idx,
            kind,
            a,
            b,
            len,
        });
    }

    // Promote the single longest suspension span to the Brooklyn signature.
    if let Some((idx, _)) = longest_susp {
        out[idx].kind = BridgeKind::Brooklyn;
    }
    out
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

    for p in plan_bridge_placements(city_json, &height_at) {
        let (scene, model_len, model_w, target_w) = match p.kind {
            BridgeKind::Brooklyn => (
                handles.bridge_brooklyn.clone(),
                SUSP_DECK_LEN_M,
                SUSP_DECK_W_M,
                SUSP_TARGET_W_M,
            ),
            BridgeKind::Suspension => (
                handles.bridge_suspension.clone(),
                SUSP_DECK_LEN_M,
                SUSP_DECK_W_M,
                SUSP_TARGET_W_M,
            ),
            BridgeKind::Truss => (
                handles.bridge_truss.clone(),
                TRUSS_DECK_LEN_M,
                TRUSS_DECK_W_M,
                TRUSS_TARGET_W_M,
            ),
        };
        let mid = (p.a + p.b) * 0.5;
        let dir = (p.b - p.a).normalize_or_zero();
        // Align authored +X span axis to the chord direction.
        // rotation_y(θ) maps +X → (cosθ, 0, -sinθ); want (dir.x, 0, dir.y).
        let theta = (-dir.y).atan2(dir.x);
        // Stretch ONLY the deck/span axis (X) and the deck width (Z). Height
        // (Y) stays at 1.0 so towers/cables are never vertically stretched.
        let scale = Vec3::new(p.len / model_len, 1.0, target_w / model_w);
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
            "mf-render bridges: placed {:?} road#{} span={:.0}m at ({:.0},{:.0})",
            p.kind,
            p.road_idx,
            p.len,
            mid.x,
            mid.y
        );
    }
}
