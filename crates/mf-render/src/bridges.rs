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
/// Drop applied to the model root so its deck-top (authored z=0, cambering up
/// to +1.2m) stays strictly BELOW the roads.rs ribbon riding at BRIDGE_DECK_Y
/// — exactly one visible deck surface, no z-fight.
const MODEL_DECK_DROP: f32 = 1.4;
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
            (build_bridges_system, log_bridge_aabb_system).in_set(crate::MfRenderSet::Statics),
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

/// The full placement decision: which spans get a model, and every road whose
/// over-water chord is covered by one (twin carriageways of the same crossing
/// share a single model but ALL get their ribbon grade-structure suppressed).
pub struct BridgePlan {
    /// Deduplicated, one model per physical crossing.
    pub models: Vec<BridgePlacement>,
    /// Road indices whose elevated slab/piers/shadow `roads.rs` must suppress.
    pub covered_roads: std::collections::HashSet<usize>,
}

/// Decide, per road, whether a bridge MODEL covers its over-water span and
/// which style. Pure function of `(city, terrain)` — called by BOTH the model
/// placer here and the `roads.rs` ribbon suppressor, so they never disagree.
///
/// DEDUP (in-game bug, 2026-07-13): NYC ships the same physical crossing as
/// several road DTOs (twin carriageways / split directions), which spawned
/// 2-3 overlapping bridge models per river at slightly different rotations.
/// Candidates whose chord midpoints sit within half their mean span of an
/// already-accepted (longer) candidate are treated as the same crossing:
/// they keep ribbon suppression but spawn no second model.
pub fn plan_bridge_placements(
    city_json: &mf_protocol::StaticCityJson,
    height_at: &HeightAt,
) -> BridgePlan {
    let mut candidates: Vec<BridgePlacement> = Vec::new();

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
        candidates.push(BridgePlacement {
            road_idx,
            kind,
            a,
            b,
            len,
        });
    }

    let covered_roads: std::collections::HashSet<usize> =
        candidates.iter().map(|p| p.road_idx).collect();

    // Longest-first greedy dedup: keep a candidate only if its midpoint is not
    // on a crossing we already accepted.
    candidates.sort_by(|x, y| {
        y.len
            .partial_cmp(&x.len)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut models: Vec<BridgePlacement> = Vec::new();
    for c in candidates {
        let mid = (c.a + c.b) * 0.5;
        let dup = models.iter().any(|m| {
            let mmid = (m.a + m.b) * 0.5;
            mid.distance(mmid) < (m.len + c.len) * 0.25
        });
        if !dup {
            models.push(c);
        }
    }

    // Promote the single longest suspension span to the Brooklyn signature.
    // (`models` is sorted longest-first, so the first suspension wins.)
    if let Some(first_susp) = models.iter_mut().find(|m| m.kind == BridgeKind::Suspension) {
        first_susp.kind = BridgeKind::Brooklyn;
    }

    BridgePlan {
        models,
        covered_roads,
    }
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
    // Rekey on the terrain sampler like roads.rs does: on some boots this
    // system first runs against the flat pre-DEM `HeightAt` (nothing samples
    // as water -> "plan 0 models") and the structural signature alone would
    // then cache that empty plan forever. `height_at.is_changed()` is true
    // the frame terrain rewrites the sampler, so the real water mask always
    // gets a replan (found via the plan-count log, 2026-07-13).
    if state.signature == Some(signature) && !height_at.is_changed() {
        return;
    }
    state.signature = Some(signature);

    // Rebuild: clear previously placed models.
    for e in &existing {
        commands.entity(e).despawn();
    }

    let plan = plan_bridge_placements(city_json, &height_at);
    tracing::info!(
        "mf-render bridges: plan {} model(s), {} covered road(s)",
        plan.models.len(),
        plan.covered_roads.len()
    );
    for p in plan.models {
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
                translation: Vec3::new(mid.x, BRIDGE_DECK_Y - MODEL_DECK_DROP, mid.y),
                rotation: Quat::from_rotation_y(theta),
                scale,
            },
            Visibility::default(),
            BridgeModelInstance,
        ));
        tracing::info!(
            "mf-render bridges: placed {:?} road#{} span={:.0}m a=({:.0},{:.0}) b=({:.0},{:.0}) mid=({:.0},{:.0})",
            p.kind,
            p.road_idx,
            p.len,
            p.a.x,
            p.a.y,
            p.b.x,
            p.b.y,
            mid.x,
            mid.y
        );
    }
}

/// One-shot world-space AABB log per spawned bridge model, once its glTF
/// children have streamed in. Diagnostic for placement/scale bugs: a placed
/// Brooklyn span should log ~span-length long and ~90m tall; a short or flat
/// AABB means the scene was scaled or partially spawned (owner-rejected
/// in-game shot 2026-07-13 read the towers at a third of their height).
fn log_bridge_aabb_system(
    instances: Query<Entity, With<BridgeModelInstance>>,
    children: Query<&Children>,
    meshes: Query<(&GlobalTransform, &bevy::render::primitives::Aabb)>,
    mut logged: Local<std::collections::HashSet<Entity>>,
) {
    for root in &instances {
        if logged.contains(&root) {
            continue;
        }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        let mut n = 0usize;
        for e in children.iter_descendants(root) {
            let Ok((gt, aabb)) = meshes.get(e) else {
                continue;
            };
            let c: Vec3 = aabb.center.into();
            let he: Vec3 = aabb.half_extents.into();
            for i in 0..8 {
                let corner = c + he
                    * Vec3::new(
                        if i & 1 == 0 { -1.0 } else { 1.0 },
                        if i & 2 == 0 { -1.0 } else { 1.0 },
                        if i & 4 == 0 { -1.0 } else { 1.0 },
                    );
                let w = gt.transform_point(corner);
                lo = lo.min(w);
                hi = hi.max(w);
            }
            n += 1;
        }
        if n > 0 {
            logged.insert(root);
            let size = hi - lo;
            tracing::info!(
                "mf-render bridges: model AABB meshes={} size=({:.0},{:.0},{:.0}) y=[{:.1}..{:.1}]",
                n,
                size.x,
                size.y,
                size.z,
                lo.y,
                hi.y
            );
        }
    }
}
