//! Arterial street-lamp glow at night: emissive dots spaced ~40m along
//! arterial road polylines, merged into one mesh per 8x8 world chunk (same
//! pattern as [`crate::trees`]), culled with `tree_draw_distance_m`.
//!
//! Geometry is static (rebuilds with fields/roads/theme/quality); night
//! visibility is a cheap per-chunk `Visibility` flip driven by
//! `DayNightState.night_factor`. No per-lamp lights — one unlit emissive
//! material shared across all chunks.

use bevy::prelude::*;

use mf_state::{CurrentCity, HeightAt, QualityTier, Theme};

use crate::daynight::DayNightState;
use crate::mesh_utils::{
    append_cuboid, arc_length_table, densify_polyline, point_along, MeshBuffers,
};
use crate::palette;
use crate::roads::{ARTERIAL_WIDTH, BRIDGE_DECK_Y, ROAD_Y_OFFSET};
use crate::RenderCacheStats;

const CHUNKS_PER_SIDE: usize = 8;
/// Spacing between lamp glow dots along arterial centerlines.
const LAMP_SPACING_M: f32 = 40.0;
/// Glow blob size (half-extent of the emissive cuboid).
const LAMP_HALF: f32 = 1.1;
const LAMP_HEIGHT: f32 = 2.2;
/// Lamp sits above the road deck so it reads as a pole-top glow at city zoom.
const LAMP_Y_ABOVE_ROAD: f32 = 7.0;
/// Hide lamps until dusk has progressed enough to matter.
const LAMP_VISIBLE_NIGHT: f32 = 0.12;

pub struct MfStreetLampsPlugin;

impl Plugin for MfStreetLampsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StreetLampsState>().add_systems(
            Update,
            (
                build_street_lamps_system.in_set(crate::MfRenderSet::Statics),
                street_lamp_visibility_system.in_set(crate::MfRenderSet::Dynamic),
            ),
        );
    }
}

#[derive(Resource, Default)]
struct StreetLampsState {
    /// `(fields version, roads.len(), total points, theme, enabled, densify bits)`.
    key: Option<(u32, usize, usize, Theme, bool, u32)>,
    entities: Vec<Entity>,
}

#[derive(Component)]
struct StreetLampChunk {
    center: Vec2,
}

#[allow(clippy::too_many_arguments)]
fn build_street_lamps_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<mf_state::LatestFields>,
    height_at: Res<HeightAt>,
    quality: Res<QualityTier>,
    theme: Res<Theme>,
    mut state: ResMut<StreetLampsState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut stats: ResMut<RenderCacheStats>,
) {
    let Some(cj) = &city.static_city else {
        return;
    };
    let Some(f) = &fields.0 else {
        return;
    };
    let knobs = quality.knobs();
    let densify_step = knobs.ribbon_densify_step_m;
    let key = (
        f.version,
        cj.roads.len(),
        cj.roads.iter().map(|r| r.points.len()).sum(),
        *theme,
        knobs.street_lamps_enabled,
        densify_step.to_bits(),
    );
    if state.key == Some(key) {
        return;
    }
    state.key = Some(key);
    for e in state.entities.drain(..) {
        commands.entity(e).despawn();
    }

    if !knobs.street_lamps_enabled {
        stats.street_lamp_chunks = 0;
        return;
    }

    let world_size = cj.world_size as f32;
    let road_scale = cj.road_scale as f32;
    let mut bufs: Vec<MeshBuffers> = (0..CHUNKS_PER_SIDE * CHUNKS_PER_SIDE)
        .map(|_| MeshBuffers::new())
        .collect();
    let mut centers = vec![Vec2::ZERO; CHUNKS_PER_SIDE * CHUNKS_PER_SIDE];
    let chunk_world = world_size / CHUNKS_PER_SIDE as f32;
    for cz in 0..CHUNKS_PER_SIDE {
        for cx in 0..CHUNKS_PER_SIDE {
            let i = cz * CHUNKS_PER_SIDE + cx;
            centers[i] = Vec2::new(
                -world_size * 0.5 + (cx as f32 + 0.5) * chunk_world,
                -world_size * 0.5 + (cz as f32 + 0.5) * chunk_world,
            );
        }
    }

    // Warm sodium / LED street-lamp glow — bright enough to bloom on
    // Medium/High, still readable as a point on Low without bloom.
    let glow = Color::srgb(1.0, 0.82, 0.45);
    let half = world_size * 0.5;

    for road in &cj.roads {
        if road.cls.as_str() != "arterial" {
            continue;
        }
        let pts: Vec<Vec2> = road
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        let pts = densify_polyline(&pts, densify_step.max(LAMP_SPACING_M * 0.5));
        let (cum, total) = arc_length_table(&pts);
        if total < LAMP_SPACING_M * 0.5 {
            continue;
        }
        // Offset lamps slightly off centerline so they read as roadside
        // poles rather than sitting in the middle of the black ribbon.
        let side = (ARTERIAL_WIDTH as f32 * road_scale) * 0.35;
        let mut d = LAMP_SPACING_M * 0.5;
        let mut side_sign = 1.0_f32;
        while d < total {
            let (pos, dir) = point_along(&pts, &cum, d);
            if dir != Vec2::ZERO {
                let perp = Vec2::new(-dir.y, dir.x) * side * side_sign;
                let x = pos.x + perp.x;
                let z = pos.y + perp.y;
                let ground = height_at.sample(x, z);
                let deck = if ground <= crate::terrain::WATER_LEVEL_Y + 0.01 {
                    BRIDGE_DECK_Y
                } else {
                    ground
                };
                let y = deck + ROAD_Y_OFFSET + LAMP_Y_ABOVE_ROAD;
                let cx = (((x + half) / world_size) * CHUNKS_PER_SIDE as f32)
                    .clamp(0.0, (CHUNKS_PER_SIDE - 1) as f32) as usize;
                let cz = (((z + half) / world_size) * CHUNKS_PER_SIDE as f32)
                    .clamp(0.0, (CHUNKS_PER_SIDE - 1) as f32) as usize;
                let buf = &mut bufs[cz * CHUNKS_PER_SIDE + cx];
                append_cuboid(
                    buf,
                    Vec2::new(x, z),
                    y,
                    LAMP_HALF,
                    LAMP_HALF,
                    LAMP_HEIGHT,
                    glow,
                    glow,
                    glow,
                );
            }
            d += LAMP_SPACING_M;
            side_sign = -side_sign;
        }
    }

    let material = materials.add(StandardMaterial {
        base_color: glow,
        emissive: palette::emissive(glow, 3.5),
        unlit: true,
        alpha_mode: AlphaMode::Opaque,
        ..default()
    });
    for (i, buf) in bufs.into_iter().enumerate() {
        if buf.is_empty() {
            continue;
        }
        let e = commands
            .spawn((
                Mesh3d(meshes.add(buf.build())),
                MeshMaterial3d(material.clone()),
                Transform::IDENTITY,
                // Start hidden; night visibility system reveals at dusk.
                Visibility::Hidden,
                StreetLampChunk { center: centers[i] },
                Name::new("street-lamps"),
            ))
            .id();
        state.entities.push(e);
    }
    stats.street_lamp_chunks = state.entities.len();
}

fn street_lamp_visibility_system(
    quality: Res<QualityTier>,
    day_night: Res<DayNightState>,
    chunks: Query<(Entity, &StreetLampChunk)>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut visibility: Query<&mut Visibility>,
) {
    let night_on = day_night.night_factor >= LAMP_VISIBLE_NIGHT;
    let Ok(cam) = cameras.single() else {
        for (entity, _) in &chunks {
            if let Ok(mut vis) = visibility.get_mut(entity) {
                *vis = Visibility::Hidden;
            }
        }
        return;
    };
    let cam_xz = Vec2::new(cam.translation.x, cam.translation.z);
    let max_dist = quality.knobs().tree_draw_distance_m;
    for (entity, chunk) in &chunks {
        let Ok(mut vis) = visibility.get_mut(entity) else {
            continue;
        };
        if !night_on {
            *vis = Visibility::Hidden;
            continue;
        }
        let in_range = match max_dist {
            None => true,
            Some(limit) => cam_xz.distance(chunk.center) <= limit,
        };
        *vis = if in_range {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}
