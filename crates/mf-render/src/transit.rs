//! Transit network visualization (spec §3.3 `transit.rs`): stations, track
//! infrastructure ribbons, and the rainbow route stripes painted on the
//! roads (with chevron arrows) — this is the layer where the player's work
//! literally paints color onto the otherwise monochrome city.
//!
//! Rebuilds on structural change (station/track/route identity), mirroring
//! `renderer.ts`'s `setUi` `structureChanged` gate; station ring color
//! (crowding) updates every UI tick (2 Hz) without a full rebuild.

use std::collections::HashMap;

use bevy::prelude::*;

use mf_protocol::{TransitMode, UiState};
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
const STRIPE_WIDTH: f32 = 3.5;
const BUNDLE_GAP: f32 = 14.0;
const CHEVRON_SPACING: f32 = 120.0;
const CHEVRON_LENGTH: f32 = 8.0;
const CHEVRON_WIDTH: f32 = 3.0;

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
    signature: Option<Signature>,
    station_entities: Vec<Entity>,
    track_entities: Vec<Entity>,
    route_entities: Vec<Entity>,
}

#[derive(PartialEq, Clone)]
struct Signature {
    station_count: usize,
    track_count: usize,
    routes: Vec<(i64, String, Vec<i64>)>,
}

fn signature_of(ui: &UiState) -> Signature {
    Signature {
        station_count: ui.stations.len(),
        track_count: ui.tracks.len(),
        routes: ui
            .routes
            .iter()
            .map(|r| (r.id, r.color.clone(), r.station_ids.clone()))
            .collect(),
    }
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
    mut ring_query: Query<(&StationRing, &MeshMaterial3d<StandardMaterial>)>,
) {
    if !ui.is_changed() {
        return;
    }
    let Some(u) = &ui.0 else {
        return;
    };

    let sig = signature_of(u);
    if state.signature.as_ref() != Some(&sig) {
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
    let body_material = materials.add(StandardMaterial {
        double_sided: true,
        cull_mode: None,
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
        let ring_material = materials.add(StandardMaterial {
            double_sided: true,
            cull_mode: None,
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
                StationRing { mode: st.mode },
            ))
            .id();
        state.station_entities.push(body);
        state.station_entities.push(ring);
    }
}

fn update_station_crowding(
    ui: &UiState,
    materials: &mut Assets<StandardMaterial>,
    ring_query: &mut Query<(&StationRing, &MeshMaterial3d<StandardMaterial>)>,
) {
    let max_ridership = ui
        .stations
        .iter()
        .map(|s| s.ridership)
        .fold(1.0_f64, f64::max);
    // Rings were (re)spawned in station order in `rebuild_stations`; zip
    // positionally since we don't carry a station-id component today (v1
    // scope — see known-gaps in the final report).
    for ((ring, mat_handle), st) in ring_query.iter_mut().zip(ui.stations.iter()) {
        let t = (st.ridership / max_ridership).clamp(0.0, 1.0) as f32;
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
        for w in r.station_ids.windows(2) {
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
        }
        if path.len() < 2 {
            continue;
        }

        let color = palette::vivid_route_color(ri);
        let mut normal_buf = MeshBuffers::new();
        append_ribbon(
            &mut normal_buf,
            &path,
            STRIPE_Y_OFFSET,
            STRIPE_WIDTH,
            color,
            |x, z| height_at.sample(x, z),
        );
        append_chevrons(&mut normal_buf, &path, height_at, color);
        let mesh = meshes.add(normal_buf.build());
        let material = materials.add(StandardMaterial {
            double_sided: true,
            cull_mode: None,
            base_color: Color::WHITE,
            emissive: palette::emissive(color, 0.1),
            alpha_mode: AlphaMode::Blend,
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
            let bold_material = materials.add(StandardMaterial {
                double_sided: true,
                cull_mode: None,
                base_color: Color::WHITE,
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
            buf.push_tri(
                Vec3::new(tip.x, y, tip.y),
                Vec3::new(left.x, y, left.y),
                Vec3::new(right.x, y, right.y),
                Vec3::Y,
                bright,
            );
        }
        d += CHEVRON_SPACING;
    }
}
