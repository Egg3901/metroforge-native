//! Transit network visualization (spec §3.3 `transit.rs`): stations, track
//! infrastructure ribbons, and the rainbow route stripes painted on the
//! roads (with chevron arrows) — this is the layer where the player's work
//! literally paints color onto the otherwise monochrome city.
//!
//! Rebuilds on structural change (station/track/route identity), mirroring
//! `renderer.ts`'s `setUi` `structureChanged` gate; station ring color
//! (crowding) updates every UI tick (2 Hz) without a full rebuild.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use bevy::prelude::*;

use mf_protocol::{TransitMode, UiState, UiStation};
use mf_state::{HeightAt, LatestUi, QualityTier};

use crate::mesh_utils::{
    append_ribbon, arc_length_table, offset_polyline, point_along, MeshBuffers,
};
use crate::palette;

const STATION_RADIUS: f32 = 14.0;
const STATION_HEIGHT: f32 = 10.0;
const STATION_RING_INNER: f32 = 15.0;
const STATION_RING_OUTER: f32 = 20.0;

const TRACK_Y_OFFSET: f32 = 2.0;
const STRIPE_Y_OFFSET: f32 = 0.6;
/// Wide enough to read as a painted transit band from overview zoom, not a
/// thread (owner feedback on the first network demo).
const STRIPE_WIDTH: f32 = 8.0;
/// Bundled parallel routes butt edge to edge like a striped ribbon: the
/// offset step equals the stripe width exactly, so adjacent bands touch
/// with zero gap (owner: routes should read inline with each other).
const BUNDLE_GAP: f32 = STRIPE_WIDTH;
const CHEVRON_SPACING: f32 = 120.0;
const CHEVRON_LENGTH: f32 = 14.0;
const CHEVRON_WIDTH: f32 = 6.0;

pub struct MfTransitPlugin;

impl Plugin for MfTransitPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TransitState>().add_systems(
            Update,
            transit_update_system.in_set(crate::MfRenderSet::Statics),
        );
    }
}

/// Marker on a station's ring entity, so `subway.rs` can find metro rings to
/// glow and `daynight.rs`/quality systems can recolor by mode.
#[derive(Component)]
pub struct StationRing {
    pub mode: TransitMode,
    /// Joins this ring back to its `UiStation` for crowding updates — rings
    /// and `ui.stations` are never assumed to iterate in the same order.
    station_id: i64,
    /// Crowding `t` (ridership/max) last written to the material, quantized
    /// to 1/64 buckets so equal-looking ticks skip the `materials.get_mut`
    /// write (2 Hz churn otherwise touches every ring's material every tick).
    crowding_bucket: Option<u8>,
}

/// Marker on the normal-width route stripe entity (always visible; faded to
/// alpha 0.3 in subway view per art-direction §7, except metro which swaps
/// to [`MetroBoldTube`] instead).
#[derive(Component)]
pub struct RouteStripe {
    pub mode: TransitMode,
}

/// The bold, 2x-width, emissive metro tube shown only in subway view
/// (art-direction §7). One per metro route, initially hidden.
#[derive(Component)]
pub struct MetroBoldTube;

#[derive(Resource, Default)]
struct TransitState {
    signature: Option<u64>,
    station_entities: Vec<Entity>,
    track_entities: Vec<Entity>,
    route_entities: Vec<Entity>,
}

/// Structural fingerprint of `ui` (station/track/route identity), used to
/// gate the full rebuild. A `u64` hash instead of a cloned `Signature`
/// struct: this runs on every `LatestUi` change (2 Hz) and the prior
/// per-field clone (route colors + station-id vecs) allocated on every tick
/// just to be thrown away after one `==`. A 64-bit hash collision would miss
/// a rebuild, but is astronomically unlikely for this data and cheap to
/// accept given the alternative is allocation on the hot compare path.
fn signature_of(ui: &UiState) -> u64 {
    let mut hasher = DefaultHasher::new();
    ui.stations.len().hash(&mut hasher);
    ui.tracks.len().hash(&mut hasher);
    for r in &ui.routes {
        r.id.hash(&mut hasher);
        r.color.as_bytes().hash(&mut hasher);
        r.station_ids.hash(&mut hasher);
    }
    hasher.finish()
}

#[allow(clippy::too_many_arguments)]
fn transit_update_system(
    mut commands: Commands,
    ui: Res<LatestUi>,
    height_at: Res<HeightAt>,
    quality: Res<QualityTier>,
    mut state: ResMut<TransitState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ring_query: Query<(&mut StationRing, &MeshMaterial3d<StandardMaterial>)>,
) {
    if !ui.is_changed() {
        return;
    }
    let Some(u) = &ui.0 else {
        return;
    };

    let sig = signature_of(u);
    if state.signature != Some(sig) {
        state.signature = Some(sig);
        rebuild_stations(
            &mut commands,
            u,
            &height_at,
            &mut state,
            &mut meshes,
            &mut materials,
        );
        rebuild_tracks(
            &mut commands,
            u,
            &height_at,
            &quality,
            &mut state,
            &mut meshes,
            &mut materials,
        );
        rebuild_routes(
            &mut commands,
            u,
            &height_at,
            &mut state,
            &mut meshes,
            &mut materials,
        );
    } else {
        update_station_crowding(u, &mut materials, &mut ring_query);
    }
}

fn rebuild_stations(
    commands: &mut Commands,
    ui: &UiState,
    height_at: &HeightAt,
    state: &mut TransitState,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    for e in state.station_entities.drain(..) {
        commands.entity(e).despawn();
    }
    let body_mesh = meshes.add(
        Cylinder::new(STATION_RADIUS, STATION_HEIGHT)
            .mesh()
            .anchor(bevy::render::mesh::CylinderAnchor::Bottom),
    );
    let ring_mesh = meshes.add(Annulus::new(STATION_RING_INNER, STATION_RING_OUTER));
    // Solid cylinder, always opaque, built by Bevy's own `Cylinder` primitive
    // (correctly wound by construction) — single-sided/back-face-culled is
    // correct.
    let body_material = materials.add(StandardMaterial {
        base_color: palette::building_top(),
        ..default()
    });

    for st in &ui.stations {
        let ground_y = height_at.sample(st.x as f32, st.y as f32);
        let body = commands
            .spawn((
                Mesh3d(body_mesh.clone()),
                MeshMaterial3d(body_material.clone()),
                Transform::from_xyz(st.x as f32, ground_y, st.y as f32),
                Visibility::default(),
                Name::new(format!("station-{}", st.id)),
            ))
            .id();
        // Verified single-sided-safe: Bevy's `Annulus` mesh builder emits a
        // flat disc in local XY with every normal `(0,0,1)` (local +Z), and
        // its own source comment states the index order is deliberately CCW
        // as seen from +Z (bevy_mesh dim2.rs `AnnulusMeshBuilder::build`).
        // This entity's transform rotates it `-FRAC_PI_2` around X; applying
        // the standard X-rotation matrix to local +Z gives
        // `(0, -sin(-PI/2), cos(-PI/2)) = (0, 1, 0)` — world `+Y`, i.e.
        // facing straight up at the top-down camera. Rotation doesn't flip
        // winding (no reflection), so front-face-CCW-from-+Z stays
        // front-face-CCW-from-+Y: single-sided is correct here, not just a
        // "leave it double-sided to be safe" case.
        let ring_material = materials.add(StandardMaterial {
            base_color: palette::mode_accent(st.mode),
            emissive: palette::emissive(palette::mode_accent(st.mode), 0.15),
            ..default()
        });
        let ring = commands
            .spawn((
                Mesh3d(ring_mesh.clone()),
                MeshMaterial3d(ring_material),
                Transform::from_xyz(st.x as f32, ground_y + STATION_HEIGHT + 0.1, st.y as f32)
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
                Visibility::default(),
                StationRing {
                    mode: st.mode,
                    station_id: st.id,
                    crowding_bucket: None,
                },
            ))
            .id();
        state.station_entities.push(body);
        state.station_entities.push(ring);
    }
}

/// Crowding buckets: quantize `t` (ridership fraction) to 1/64 so a tick
/// whose ridership barely moved doesn't re-touch the material.
const CROWDING_BUCKETS: f32 = 64.0;

fn quantize_crowding(t: f32) -> u8 {
    (t * CROWDING_BUCKETS).round() as u8
}

fn update_station_crowding(
    ui: &UiState,
    materials: &mut Assets<StandardMaterial>,
    ring_query: &mut Query<(&mut StationRing, &MeshMaterial3d<StandardMaterial>)>,
) {
    let max_ridership = ui
        .stations
        .iter()
        .map(|s| s.ridership)
        .fold(1.0_f64, f64::max);
    // Join rings to stations by id, not by query iteration order (ECS query
    // order isn't guaranteed to track spawn order). Rings for stations no
    // longer present keep their current color rather than guessing.
    let station_by_id: HashMap<i64, &UiStation> = ui.stations.iter().map(|s| (s.id, s)).collect();
    for (mut ring, mat_handle) in ring_query.iter_mut() {
        let Some(st) = station_by_id.get(&ring.station_id) else {
            continue;
        };
        let t = (st.ridership / max_ridership).clamp(0.0, 1.0) as f32;
        let bucket = quantize_crowding(t);
        if ring.crowding_bucket == Some(bucket) {
            continue;
        }
        ring.crowding_bucket = Some(bucket);
        let base = palette::mode_accent(ring.mode);
        let hot = palette::brighten(palette::vivid_route_color(0), 0.2); // hot red accent
        let color = base.mix(&hot, t * 0.6);
        if let Some(mat) = materials.get_mut(&mat_handle.0) {
            mat.base_color = color;
            mat.emissive = palette::emissive(color, 0.15 + t * 0.3);
        }
    }
}

fn rebuild_tracks(
    commands: &mut Commands,
    ui: &UiState,
    height_at: &HeightAt,
    quality: &QualityTier,
    state: &mut TransitState,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    for e in state.track_entities.drain(..) {
        commands.entity(e).despawn();
    }
    let unlit = quality.knobs().unlit_material;
    // Group by mode+grade so each combination gets one merged mesh/material
    // (small, fixed set: 4 modes x 3 grades).
    let mut groups: HashMap<(TransitMode, String), MeshBuffers> = HashMap::new();
    for t in &ui.tracks {
        let pts: Vec<Vec2> = t
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        let width = if t.mode == TransitMode::Bus { 5.0 } else { 8.0 };
        let buf = groups.entry((t.mode, t.grade.clone())).or_default();
        append_ribbon(
            buf,
            &pts,
            TRACK_Y_OFFSET,
            width,
            palette::mode_accent(t.mode),
            |x, z| height_at.sample(x, z),
        );
    }
    for ((mode, grade), buf) in groups {
        if buf.is_empty() {
            continue;
        }
        let alpha = if grade == "tunnel" { 0.18 } else { 0.28 };
        let mesh = meshes.add(buf.build());
        // Genuinely translucent always (0.18/0.28, never faded to 1.0 by
        // subway.rs) — stays `Blend`, unlike the road/stripe materials.
        // `double_sided`/`cull_mode` also stay as before: this is an
        // `append_ribbon`-built, `Blend`-mode, no-reactive-`unlit`-updater
        // material — the same shape as roads.rs's road-class materials,
        // where single-siding was A/B-diff-verified to visibly brighten the
        // subway+low-quality combination versus baseline (see the long
        // comment there). Reverted alongside that fix out of caution rather
        // than independently re-verified.
        let material = materials.add(StandardMaterial {
            double_sided: true,
            cull_mode: None,
            base_color: Color::WHITE.with_alpha(alpha),
            alpha_mode: AlphaMode::Blend,
            unlit,
            ..default()
        });
        let e = commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::IDENTITY,
                Visibility::default(),
                Name::new(format!("track-{mode:?}-{grade}")),
            ))
            .id();
        state.track_entities.push(e);
    }
}

/// Per-STATION-PAIR ribbon widths for a route's stripe (v0.3, ship-plan
/// #25): `STRIPE_WIDTH * (0.7 + load/max_load)` when `segment_loads` aligns
/// 1:1 with `pair_count` (`r.station_ids.windows(2)` count) — the busiest
/// pair on the route always lands at `STRIPE_WIDTH * 1.7`, an empty one at
/// `STRIPE_WIDTH * 0.7`. Falls back to `STRIPE_WIDTH` uniformly for every
/// pair when the lengths don't match (stale sim data, a future protocol
/// change) or every load is non-positive (nothing to normalize against) —
/// defensive by construction, this must never index out of bounds or paint
/// a route with a nonsensical width. Pure function (no ECS/mesh types), so
/// the normalization and both fallback paths are unit-testable directly.
fn segment_widths(pair_count: usize, segment_loads: &[f64]) -> Vec<f32> {
    if pair_count == 0 {
        return Vec::new();
    }
    let aligned = segment_loads.len() == pair_count;
    let max_load = if aligned {
        segment_loads.iter().cloned().fold(0.0_f64, f64::max)
    } else {
        0.0
    };
    if aligned && max_load > 0.0 {
        segment_loads
            .iter()
            .map(|&load| STRIPE_WIDTH * (0.7 + (load / max_load) as f32))
            .collect()
    } else {
        vec![STRIPE_WIDTH; pair_count]
    }
}

fn rebuild_routes(
    commands: &mut Commands,
    ui: &UiState,
    height_at: &HeightAt,
    state: &mut TransitState,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    for e in state.route_entities.drain(..) {
        commands.entity(e).despawn();
    }
    if ui.routes.is_empty() {
        return;
    }

    let station_by_id: HashMap<i64, (f32, f32)> = ui
        .stations
        .iter()
        .map(|s| (s.id, (s.x as f32, s.y as f32)))
        .collect();

    // track geometry keyed by ordered station-pair (both directions).
    let mut track_by_pair: HashMap<(i64, i64), Vec<Vec2>> = HashMap::new();
    for t in &ui.tracks {
        let pts: Vec<Vec2> = t
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        track_by_pair.insert((t.from_station_id, t.to_station_id), pts.clone());
        let rev: Vec<Vec2> = pts.into_iter().rev().collect();
        track_by_pair.insert((t.to_station_id, t.from_station_id), rev);
    }

    // how many routes share each undirected pair -> parallel bundling.
    let mut pair_users: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (ri, r) in ui.routes.iter().enumerate() {
        for w in r.station_ids.windows(2) {
            let (a, b) = (w[0], w[1]);
            let key = if a < b { (a, b) } else { (b, a) };
            let list = pair_users.entry(key).or_default();
            if !list.contains(&ri) {
                list.push(ri);
            }
        }
    }

    for (ri, r) in ui.routes.iter().enumerate() {
        let mut path: Vec<Vec2> = Vec::new();
        // Per-STATION-PAIR segments (post-bundling-offset), tagged with
        // that pair's index into `r.station_ids.windows(2)` — this is the
        // same indexing `r.segment_loads` uses (one load entry per station
        // pair), NOT per drawn point: a pair's track can itself carry
        // several intermediate points (a curve/detour), and every one of
        // those sub-segments must inherit that ONE pair's width rather than
        // each somehow getting its own. Collected separately from `path`
        // (which stays the full concatenated polyline, still needed as-is
        // for `append_chevrons`/the metro bold tube below) so the
        // width-scaled ribbon loop can walk pair-by-pair instead of point-
        // by-point.
        let mut pair_segs: Vec<(usize, Vec<Vec2>)> = Vec::new();
        for (pi, w) in r.station_ids.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            let mut seg = track_by_pair.get(&(a, b)).cloned().unwrap_or_else(|| {
                match (station_by_id.get(&a), station_by_id.get(&b)) {
                    (Some(&sa), Some(&sb)) => vec![Vec2::new(sa.0, sa.1), Vec2::new(sb.0, sb.1)],
                    _ => Vec::new(),
                }
            });
            if seg.len() < 2 {
                continue;
            }
            let key = if a < b { (a, b) } else { (b, a) };
            if let Some(users) = pair_users.get(&key) {
                if users.len() > 1 {
                    let slot = users.iter().position(|&x| x == ri).unwrap_or(0);
                    let offset = (slot as f32 - (users.len() as f32 - 1.0) / 2.0) * BUNDLE_GAP;
                    seg = offset_polyline(&seg, offset);
                }
            }
            if path.is_empty() {
                path.push(seg[0]);
            }
            path.extend_from_slice(&seg[1..]);
            pair_segs.push((pi, seg));
        }
        if path.len() < 2 {
            continue;
        }

        let color = palette::vivid_route_color(ri);
        let mut normal_buf = MeshBuffers::new();
        // Per-segment load width (v0.3, ship-plan #25): one ribbon width
        // per station pair rather than one uniform width for the whole
        // route, so a crowded stretch reads visibly fatter. `segment_widths`
        // is the pure normalization (also unit-tested below); it already
        // falls back to `STRIPE_WIDTH` uniformly when `r.segment_loads`
        // doesn't align 1:1 with the route's station-pair count, so this
        // loop doesn't need its own separate defensive branch — every
        // `pair_segs` entry always gets SOME width from `widths`.
        let expected_pairs = r.station_ids.len().saturating_sub(1);
        let widths = segment_widths(expected_pairs, &r.segment_loads);
        for (pi, seg) in &pair_segs {
            let width = widths.get(*pi).copied().unwrap_or(STRIPE_WIDTH);
            append_ribbon(&mut normal_buf, seg, STRIPE_Y_OFFSET, width, color, |x, z| {
                height_at.sample(x, z)
            });
        }
        append_chevrons(&mut normal_buf, &path, height_at, color);
        let mesh = meshes.add(normal_buf.build());
        // `Blend`, not `Opaque` — see the long comment on the road-class
        // materials in `roads.rs` for why: dynamically flipping this to
        // `Opaque` when steady and back to `Blend` mid-fade (this crate's
        // other candidate for the same perf win) broke rendering in
        // practice once verified via headless screenshot diffing, so this
        // stays unconditionally `Blend` like the original code.
        // `double_sided`/`cull_mode` also stay as before — same
        // append_ribbon/Blend/stale-`unlit` shape as roads.rs's materials,
        // where single-siding was A/B-diff-verified to visibly brighten the
        // subway+low-quality combination (see that comment); not
        // independently re-verified for this material, reverted out of
        // caution.
        // Color at the MATERIAL level (`base_color`), not vertex colors:
        // same root cause and fix as roads.rs's road-class materials. Vertex
        // colors do not reach the shader for `AlphaMode::Blend`
        // `StandardMaterial`s in this Bevy 0.16 setup, so leaving the vivid
        // color only in the ribbon's per-vertex colors (as this used to)
        // rendered every stripe plain white, not the rainbow the vertex data
        // encoded. The per-vertex chevron brightness accent (`append_chevrons`
        // paints its triangles 20% brighter than the ribbon) is lost by the
        // same mechanism: chevrons share this mesh/material, so they render
        // the same flat `color` as the stripe rather than standing out.
        // Accepted: chevrons are still visible via geometry (they're a
        // separate arrow shape sitting on the stripe), just not extra-bright.
        // `perceptual_roughness`/`reflectance` added to match roads.rs's
        // matte discipline: this surface now receives direct sun the same as
        // roads once its base_color actually carries the route color, so it
        // needs the same anti-specular-sheen treatment.
        let material = materials.add(StandardMaterial {
            double_sided: true,
            cull_mode: None,
            base_color: color,
            // Strong enough that the band stays saturated under full daylight
            // (pure diffuse tonemapped to pastel in the first day demo).
            emissive: palette::emissive(color, 0.45),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            ..default()
        });
        let e = commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::IDENTITY,
                Visibility::default(),
                RouteStripe { mode: r.mode },
                Name::new(format!("route-stripe-{}", r.id)),
            ))
            .id();
        state.route_entities.push(e);

        if r.mode == TransitMode::Metro {
            let mut bold_buf = MeshBuffers::new();
            append_ribbon(
                &mut bold_buf,
                &path,
                STRIPE_Y_OFFSET + 0.4,
                STRIPE_WIDTH * 2.0,
                color,
                |x, z| height_at.sample(x, z),
            );
            let bold_mesh = meshes.add(bold_buf.build());
            // Solid whenever visible (subway.rs only ever toggles this
            // entity's Visibility, never its alpha) — `..default()` already
            // gives `AlphaMode::Opaque`, kept implicit here since nothing
            // ever changes it. Unlike the normal stripe above, `Opaque`
            // materials in this Bevy 0.16 setup DO honor per-vertex color
            // (same reason terrain.rs's vertex colors already work, see the
            // comment on roads.rs's road-class materials), so this was never
            // rendering white. `base_color` is set to the route color anyway
            // (rather than left `WHITE`) to match the normal stripe's fix and
            // to not depend on the vertex-color path holding up under a
            // future material change.
            let bold_material = materials.add(StandardMaterial {
                base_color: color,
                emissive: palette::emissive(color, 0.8),
                ..default()
            });
            let bold_e = commands
                .spawn((
                    Mesh3d(bold_mesh),
                    MeshMaterial3d(bold_material),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    MetroBoldTube,
                    Name::new(format!("route-metro-bold-{}", r.id)),
                ))
                .id();
            state.route_entities.push(bold_e);
        }
    }
}

/// Chevron arrows every ~120m along `path`, pointing along direction of
/// travel (station order), same color 20% brighter (art-direction §3).
fn append_chevrons(buf: &mut MeshBuffers, path: &[Vec2], height_at: &HeightAt, color: Color) {
    let (cum, total) = arc_length_table(path);
    if total < CHEVRON_SPACING {
        return;
    }
    let bright = palette::brighten(color, 0.2);
    let mut d = CHEVRON_SPACING * 0.5;
    while d < total {
        let (pos, dir) = point_along(path, &cum, d);
        if dir != Vec2::ZERO {
            let perp = Vec2::new(-dir.y, dir.x);
            let y = height_at.sample(pos.x, pos.y) + STRIPE_Y_OFFSET + 0.02;
            let tip = pos + dir * CHEVRON_LENGTH;
            let left = pos - dir * CHEVRON_LENGTH * 0.3 + perp * CHEVRON_WIDTH * 0.5;
            let right = pos - dir * CHEVRON_LENGTH * 0.3 - perp * CHEVRON_WIDTH * 0.5;
            // Winding vs the declared `+Y` normal: with `perp = (-dz, dx)`,
            // `v1 = left-tip` and `v2 = right-tip` work out (using
            // dx^2+dz^2 == 1) to a right-hand cross product of `-2*a*b*Y`
            // (a,b > 0) — i.e. `(tip,left,right)` winds CCW as seen from
            // below, not from above. `push_tri` needs (p0,p1,p2) CCW from
            // `normal`, so swap the last two args to `(tip,right,left)`,
            // which flips the cross product to `+Y`.
            buf.push_tri(
                Vec3::new(tip.x, y, tip.y),
                Vec3::new(right.x, y, right.y),
                Vec3::new(left.x, y, left.y),
                Vec3::Y,
                bright,
            );
        }
        d += CHEVRON_SPACING;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_widths_zero_pairs_is_empty() {
        assert!(segment_widths(0, &[]).is_empty());
    }

    #[test]
    fn segment_widths_empty_loads_falls_back_to_uniform() {
        let widths = segment_widths(3, &[]);
        assert_eq!(widths, vec![STRIPE_WIDTH; 3]);
    }

    #[test]
    fn segment_widths_mismatched_length_falls_back_to_uniform() {
        // 3 pairs but only 2 load entries — must not panic or misattribute
        // a load to the wrong pair, just fall back uniformly.
        let widths = segment_widths(3, &[10.0, 20.0]);
        assert_eq!(widths, vec![STRIPE_WIDTH; 3]);
    }

    #[test]
    fn segment_widths_all_zero_falls_back_to_uniform() {
        let widths = segment_widths(3, &[0.0, 0.0, 0.0]);
        assert_eq!(widths, vec![STRIPE_WIDTH; 3]);
    }

    #[test]
    fn segment_widths_normalizes_busiest_pair_to_1_7x_stripe_width() {
        let widths = segment_widths(3, &[0.0, 50.0, 100.0]);
        assert_eq!(widths.len(), 3);
        assert!((widths[0] - STRIPE_WIDTH * 0.7).abs() < 0.001);
        assert!((widths[2] - STRIPE_WIDTH * 1.7).abs() < 0.001);
        let mid = STRIPE_WIDTH * (0.7 + 0.5);
        assert!((widths[1] - mid).abs() < 0.001);
    }

    #[test]
    fn segment_widths_single_pair_uses_full_load_as_its_own_max() {
        // One pair, one load: that load IS the max, so it normalizes to
        // 1.0 and lands at the ceiling, not some degenerate divide.
        let widths = segment_widths(1, &[42.0]);
        assert_eq!(widths.len(), 1);
        assert!((widths[0] - STRIPE_WIDTH * 1.7).abs() < 0.001);
    }
}
