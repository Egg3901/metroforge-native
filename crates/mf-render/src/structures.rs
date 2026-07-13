//! `structures.rs` — placement of the generic reusable structure kit
//! (`tools/blender/gen_tunnel.py`, `gen_viaduct.py`, `gen_rail_viaduct.py`,
//! `gen_rail_bridge.py`): concrete tunnel portals, tileable elevated road and
//! rail viaduct segments, and a girder rail bridge for track water crossings.
//!
//! This is the Bevy half of the kit — the model generators own the geometry,
//! this module scales/rotates/tiles the committed `.glb` scenes over the city
//! and suppresses the cheap procedural extrusion beneath them (the
//! double-render fix). It mirrors [`crate::bridges`]: placement is a pure
//! function of `(city, tracks, terrain)`, called by BOTH this placer and the
//! `roads.rs` / `transit.rs` extrusion suppressors so they never disagree.
//!
//! Coordinate convention (matches roads.rs / bridges.rs): a DTO point (px, py)
//! is world `Vec3::new(px, height, py)`. Models are authored facing +X along
//! the run/span, deck top at model-y ≈ 0, width along model-z; placement
//! stretches only the run/width axes, never the vertical, except the viaduct
//! piers which are vertical-scaled DOWN to reach the sampled ground.
//!
//! Tier policy: tunnel portals and the rail bridge are ALL-TIER (like the
//! long-span bridges). The two viaducts are a Medium+ model UPGRADE — Potato/
//! Low keep the cheap extruded deck+piers (`unlit_material` == Potato|Low is
//! the shared gate the suppressors read).

use std::collections::HashSet;

use bevy::prelude::*;
use bevy::scene::SceneRoot;

use mf_state::{CurrentCity, HeightAt, LatestUi, QualityTier};

use crate::models::ModelHandles;
use crate::roads::{BRIDGE_DECK_Y, GRADE_STEP_Y};

// ── authored model dimensions (mirror the generators) ───────────────────────
/// Portal authored mouth width (gen_tunnel.py MOUTH_W) — Z scaled to corridor.
const PORTAL_MOUTH_W: f32 = 12.0;
/// Road viaduct authored segment length / deck width / pier reach (gen_viaduct).
const RV_SEG_LEN: f32 = 24.0;
const RV_DECK_W: f32 = 20.0;
const RV_PIER_REACH: f32 = 12.8;
/// Rail viaduct authored segment length / deck width / pier reach.
const RAILV_SEG_LEN: f32 = 20.0;
const RAILV_DECK_W: f32 = 9.0;
const RAILV_PIER_REACH: f32 = 12.3;
/// Rail viaduct deck clearance above ground (mirrors transit ELEVATED_CLEARANCE_M).
const RAILV_CLEARANCE_M: f32 = 12.0;
/// Rail bridge authored total deck length (SPAN 120 + 2*OVERHANG 8) and width.
const RAIL_BRIDGE_LEN: f32 = 136.0;
const RAIL_BRIDGE_W: f32 = 9.0;
/// Shortest over-water track chord (m) that gets a rail bridge model.
const RAIL_BRIDGE_MIN_M: f32 = 40.0;
/// Target presented widths.
const ROAD_VIADUCT_W: f32 = 20.0;
const RAIL_DECK_TARGET_W: f32 = 9.0;
/// Drop so a placed model deck sits a hair under the ribbon it carries (no
/// z-fight), matching bridges.rs MODEL_DECK_DROP.
const MODEL_DECK_DROP: f32 = 1.4;

/// A road counts as an elevated-viaduct candidate (Medium+ model upgrade).
pub fn is_road_viaduct(road: &mf_protocol::RoadDto) -> bool {
    !road.is_tunnel && road.grade_level >= 1
}

/// A track grade string names an elevated run.
pub fn is_track_elevated(grade: &str) -> bool {
    grade == "elevated"
}

/// Whether a (non-bus) track's longest over-water chord earns a rail-bridge
/// model. The pure predicate `transit.rs` calls to suppress its own bridge
/// side-rails under the model. `pts` are the RAW track DTO points.
pub fn track_gets_rail_bridge(
    pts: &[Vec2],
    height_at: &HeightAt,
    mode: mf_protocol::TransitMode,
) -> bool {
    if mode == mf_protocol::TransitMode::Bus {
        return false;
    }
    over_water_chord(pts, height_at).is_some_and(|(_, _, len)| len >= RAIL_BRIDGE_MIN_M)
}

#[derive(Component)]
struct StructureInstance;

#[derive(Resource, Default)]
struct StructuresState {
    signature: Option<(usize, usize, usize)>,
}

pub struct MfStructuresPlugin;

impl Plugin for MfStructuresPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StructuresState>().add_systems(
            Update,
            (build_structures_system, log_structure_aabb_system)
                .in_set(crate::MfRenderSet::Statics),
        );
    }
}

fn medium_plus(tier: QualityTier) -> bool {
    matches!(tier, QualityTier::Medium | QualityTier::High)
}

fn polyline(points: &[f64]) -> Vec<Vec2> {
    points
        .chunks_exact(2)
        .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
        .collect()
}

/// Water-aware ground sample used for deck heights (mirrors roads.rs `sample`).
fn deck_base(height_at: &HeightAt, x: f32, z: f32) -> f32 {
    let h = height_at.sample(x, z);
    if h <= crate::terrain::WATER_LEVEL_Y + 0.01 {
        BRIDGE_DECK_Y
    } else {
        h
    }
}

/// Longest contiguous over-water run of a polyline (mirrors bridges.rs).
fn over_water_chord(pts: &[Vec2], height_at: &HeightAt) -> Option<(Vec2, Vec2, f32)> {
    let over = |p: &Vec2| height_at.sample(p.x, p.y) <= crate::terrain::WATER_LEVEL_Y + 0.01;
    let mut best: Option<(usize, usize, f32)> = None;
    let mut run: Option<usize> = None;
    for i in 0..pts.len() {
        if over(&pts[i]) {
            run.get_or_insert(i);
        } else if let Some(s) = run.take() {
            let len = pts[s].distance(pts[i.saturating_sub(1)]);
            if best.is_none_or(|b| len > b.2) {
                best = Some((s, i - 1, len));
            }
        }
    }
    if let Some(s) = run {
        let e = pts.len() - 1;
        let len = pts[s].distance(pts[e]);
        if best.is_none_or(|b| len > b.2) {
            best = Some((s, e, len));
        }
    }
    best.map(|(s, e, len)| (pts[s], pts[e], len))
}

/// Yaw so the authored +X run axis aligns to `dir` (matches bridges.rs).
fn yaw_for(dir: Vec2) -> f32 {
    (-dir.y).atan2(dir.x)
}

/// Spawn one kit model instance, scaled/rotated/placed.
#[allow(clippy::too_many_arguments)]
fn spawn_model(
    commands: &mut Commands,
    scene: Handle<Scene>,
    center: Vec2,
    deck_y: f32,
    dir: Vec2,
    scale: Vec3,
) {
    commands.spawn((
        SceneRoot(scene),
        Transform {
            translation: Vec3::new(center.x, deck_y, center.y),
            rotation: Quat::from_rotation_y(yaw_for(dir)),
            scale,
        },
        Visibility::default(),
        StructureInstance,
    ));
}

/// Tile fixed-length viaduct segments along an elevated polyline. Segments are
/// placed at their centers every `seg_len` (authored length), oriented to the
/// local direction, deck at `deck_of(center)`, pier vertical-scaled so its foot
/// reaches the sampled ground (never floating; only stretches for deep grades).
#[allow(clippy::too_many_arguments)]
fn tile_viaduct(
    commands: &mut Commands,
    scene: &Handle<Scene>,
    pts: &[Vec2],
    height_at: &HeightAt,
    seg_len: f32,
    deck_w: f32,
    target_w: f32,
    pier_reach: f32,
    deck_lift: f32,
) -> usize {
    // Arc-length walk.
    let mut cum = vec![0.0f32];
    for w in pts.windows(2) {
        cum.push(cum.last().unwrap() + w[0].distance(w[1]));
    }
    let total = *cum.last().unwrap_or(&0.0);
    if total < seg_len * 0.5 {
        return 0;
    }
    let point_at = |d: f32| -> (Vec2, Vec2) {
        for i in 0..pts.len() - 1 {
            if d <= cum[i + 1] || i == pts.len() - 2 {
                let seg = (cum[i + 1] - cum[i]).max(1e-3);
                let f = ((d - cum[i]) / seg).clamp(0.0, 1.0);
                let p = pts[i].lerp(pts[i + 1], f);
                let dir = (pts[i + 1] - pts[i]).normalize_or_zero();
                return (p, dir);
            }
        }
        (pts[0], Vec2::X)
    };
    let mut placed = 0;
    let mut d = seg_len * 0.5;
    while d < total {
        let (center, dir) = point_at(d);
        if dir != Vec2::ZERO {
            let ground = deck_base(height_at, center.x, center.y);
            let deck_y = ground + deck_lift;
            let drop = (deck_y - height_at.sample(center.x, center.y)).max(deck_lift);
            let y_scale = (drop / pier_reach).max(1.0);
            let scale = Vec3::new(
                (seg_len + 0.4) / seg_len, // hair of overlap to hide seams
                y_scale,
                target_w / deck_w,
            );
            spawn_model(
                commands,
                scene.clone(),
                center,
                deck_y - MODEL_DECK_DROP,
                dir,
                scale,
            );
            placed += 1;
        }
        d += seg_len;
    }
    placed
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn build_structures_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<mf_state::LatestFields>,
    ui: Res<LatestUi>,
    height_at: Res<HeightAt>,
    quality: Res<QualityTier>,
    handles: Option<Res<ModelHandles>>,
    mut state: ResMut<StructuresState>,
    existing: Query<Entity, With<StructureInstance>>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    if fields.0.is_none() {
        return;
    }
    let Some(handles) = handles else { return };
    let ui_state = ui.0.as_ref();

    let signature = (
        city_json.roads.len(),
        city_json.roads.iter().map(|r| r.points.len()).sum(),
        ui_state.map(|u| u.tracks.len()).unwrap_or(0),
    );
    // Elevation-channel rekey (same race as bridges.rs): the first boot samples
    // the flat pre-DEM `HeightAt` (nothing is water / nothing is graded to
    // real relief); `height_at.is_changed()` fires the frame terrain rewrites
    // the sampler, forcing a replan onto the real water mask.
    if state.signature == Some(signature) && !height_at.is_changed() && !quality.is_changed() {
        return;
    }
    state.signature = Some(signature);

    for e in &existing {
        commands.entity(e).despawn();
    }

    let mplus = medium_plus(*quality);
    let mut n_portal = 0usize;
    let mut n_road_via = 0usize;
    let mut n_rail_via = 0usize;
    let mut n_rail_bridge = 0usize;

    // ── 1. Road tunnel portals (all tiers) ──────────────────────────────────
    // A concrete portal at each mouth of a buried road, +X facing OUTWARD.
    for road in &city_json.roads {
        if !road.is_tunnel {
            continue;
        }
        let pts = polyline(&road.points);
        if pts.len() < 2 {
            continue;
        }
        let width = road_width(&road.cls);
        let ends = [(pts[0], pts[1]), (pts[pts.len() - 1], pts[pts.len() - 2])];
        for (mouth, inward) in ends {
            let outward = (mouth - inward).normalize_or_zero();
            if outward == Vec2::ZERO {
                continue;
            }
            let ground = height_at.sample(mouth.x, mouth.y);
            let scale = Vec3::new(1.0, 1.0, (width / PORTAL_MOUTH_W).max(0.6));
            spawn_model(
                &mut commands,
                handles.portal_tunnel.clone(),
                mouth,
                ground,
                outward,
                scale,
            );
            n_portal += 1;
        }
    }

    // ── 2. Road viaduct segments (Medium+ upgrade) ──────────────────────────
    if mplus {
        for road in &city_json.roads {
            if !is_road_viaduct(road) {
                continue;
            }
            let pts = polyline(&road.points);
            if pts.len() < 2 {
                continue;
            }
            let lift = road.grade_level as f32 * GRADE_STEP_Y;
            n_road_via += tile_viaduct(
                &mut commands,
                &handles.viaduct_road,
                &pts,
                &height_at,
                RV_SEG_LEN,
                RV_DECK_W,
                road_width(&road.cls).max(ROAD_VIADUCT_W * 0.5),
                RV_PIER_REACH,
                lift,
            );
        }
    }

    // ── 3. Metro/rail tracks: portals + elevated viaducts + water bridges ────
    if let Some(u) = ui_state {
        // Station -> grades present, so a tunnel-track endpoint only gets a
        // portal where the line actually SURFACES (a non-tunnel grade shares
        // the station) — mirrors transit.rs `neighbor_grade_at`, so portals
        // fire at real tunnel mouths, not at buried tunnel-to-tunnel joints.
        let mut station_grades: std::collections::HashMap<i64, Vec<String>> =
            std::collections::HashMap::new();
        for t in &u.tracks {
            for sid in [t.from_station_id, t.to_station_id] {
                let list = station_grades.entry(sid).or_default();
                if !list.iter().any(|g| g == &t.grade) {
                    list.push(t.grade.clone());
                }
            }
        }
        let surfaces = |sid: i64| -> bool {
            station_grades
                .get(&sid)
                .is_some_and(|gs| gs.iter().any(|g| g != "tunnel"))
        };

        for t in &u.tracks {
            if t.mode == mf_protocol::TransitMode::Bus {
                continue; // buses ride roads, not viaducts/bridges
            }
            let pts = polyline(&t.points);
            if pts.len() < 2 {
                continue;
            }
            // Metro tunnel portals at surfacing endpoints (Medium+ upgrade;
            // Potato/Low keep transit.rs' trapezoid mouth). +X faces outward.
            if mplus && t.grade == "tunnel" {
                for (mouth, inward, sid) in [
                    (pts[0], pts[1], t.from_station_id),
                    (pts[pts.len() - 1], pts[pts.len() - 2], t.to_station_id),
                ] {
                    if !surfaces(sid) {
                        continue;
                    }
                    let outward = (mouth - inward).normalize_or_zero();
                    if outward == Vec2::ZERO {
                        continue;
                    }
                    let ground = height_at.sample(mouth.x, mouth.y);
                    let tw = if t.mode == mf_protocol::TransitMode::Bus {
                        5.0
                    } else {
                        8.0
                    };
                    let scale = Vec3::new(1.0, 1.0, (tw / PORTAL_MOUTH_W).max(0.55));
                    spawn_model(
                        &mut commands,
                        handles.portal_tunnel.clone(),
                        mouth,
                        ground,
                        outward,
                        scale,
                    );
                    n_portal += 1;
                    tracing::info!(
                        "mf-render structures: placed metro portal at ({:.0},{:.0})",
                        mouth.x,
                        mouth.y
                    );
                }
            }
            // Rail bridge over the longest over-water chord (all tiers).
            if let Some((a, b, len)) = over_water_chord(&pts, &height_at) {
                if len >= RAIL_BRIDGE_MIN_M {
                    let mid = (a + b) * 0.5;
                    let dir = (b - a).normalize_or_zero();
                    let scale = Vec3::new(
                        len / RAIL_BRIDGE_LEN,
                        1.0,
                        RAIL_DECK_TARGET_W / RAIL_BRIDGE_W,
                    );
                    spawn_model(
                        &mut commands,
                        handles.rail_bridge.clone(),
                        mid,
                        BRIDGE_DECK_Y - MODEL_DECK_DROP,
                        dir,
                        scale,
                    );
                    n_rail_bridge += 1;
                    tracing::info!(
                        "mf-render structures: placed rail-bridge span={:.0}m mid=({:.0},{:.0})",
                        len,
                        mid.x,
                        mid.y
                    );
                }
            }
            // Elevated rail viaduct segments (Medium+ upgrade).
            if mplus && is_track_elevated(&t.grade) {
                if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
                    tracing::info!(
                        "mf-render structures: rail viaduct run a=({:.0},{:.0}) b=({:.0},{:.0})",
                        a.x,
                        a.y,
                        b.x,
                        b.y
                    );
                }
                n_rail_via += tile_viaduct(
                    &mut commands,
                    &handles.viaduct_rail,
                    &pts,
                    &height_at,
                    RAILV_SEG_LEN,
                    RAILV_DECK_W,
                    RAIL_DECK_TARGET_W,
                    RAILV_PIER_REACH,
                    RAILV_CLEARANCE_M,
                );
            }
        }
    }

    tracing::info!(
        "mf-render structures: portals={} road-viaduct-seg={} rail-viaduct-seg={} rail-bridge={} (medium_plus={})",
        n_portal,
        n_road_via,
        n_rail_via,
        n_rail_bridge,
        mplus
    );
}

fn road_width(cls: &str) -> f32 {
    match cls {
        "arterial" => 16.0,
        "collector" => 11.0,
        _ => 8.0,
    }
}

/// One-shot world-space AABB log per placed structure model, once its glTF
/// children have streamed in (placement/scale diagnostic, mirrors bridges.rs).
fn log_structure_aabb_system(
    instances: Query<Entity, With<StructureInstance>>,
    children: Query<&Children>,
    meshes: Query<(&GlobalTransform, &bevy::render::primitives::Aabb)>,
    mut logged: Local<HashSet<Entity>>,
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
                "mf-render structures: model AABB meshes={} size=({:.0},{:.0},{:.0}) y=[{:.1}..{:.1}]",
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
