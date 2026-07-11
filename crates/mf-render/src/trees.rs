//! Stylized park trees (owner art direction: parks stay painted green with
//! rendered stylized trees). Blocky lollipops in the white-city language:
//! a thin trunk cuboid and a green canopy cuboid with per-tree jitter,
//! scattered over park cells by a deterministic hash, merged into one mesh
//! per 8x8 world chunk (same culling story as buildings), rebuilt on the
//! fields version like every other static layer.
//!
//! Colors come from [`crate::palette`] (theme-aware) rather than hardcoded
//! RGB, and the rebuild key includes `Theme` so a Settings theme switch
//! repaints trunks/canopies.
//!
//! Quality knobs (perf audit): Potato disables trees entirely; Low/Medium
//! cull chunks by camera distance the same way buildings do.

use bevy::prelude::*;

use mf_state::{CurrentCity, HeightAt, LatestFields, QualityTier, Theme};

use crate::mesh_utils::{append_cuboid, hash01, MeshBuffers};
use crate::palette;

const CHUNKS_PER_SIDE: usize = 8;
/// One tree per park cell where the hash clears this density gate.
const TREE_DENSITY: f32 = 0.45;

pub struct MfTreesPlugin;

impl Plugin for MfTreesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TreesState>().add_systems(
            Update,
            (
                build_trees_system.in_set(crate::MfRenderSet::Statics),
                tree_draw_distance_system.in_set(crate::MfRenderSet::Dynamic),
            ),
        );
    }
}

#[derive(Resource, Default)]
struct TreesState {
    /// `(fields version, theme, tree_enabled)` — rebuild on fields bump,
    /// theme switch, or the Potato toggle flipping.
    key: Option<(u32, Theme, bool)>,
    entities: Vec<Entity>,
}

#[derive(Component)]
struct TreeChunk {
    center: Vec2,
}

#[allow(clippy::too_many_arguments)]
fn build_trees_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<LatestFields>,
    height_at: Res<HeightAt>,
    quality: Res<QualityTier>,
    theme: Res<Theme>,
    mut state: ResMut<TreesState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(cj) = &city.static_city else { return };
    let Some(f) = &fields.0 else { return };
    let knobs = quality.knobs();
    let key = (f.version, *theme, knobs.tree_enabled);
    if state.key == Some(key) {
        return;
    }
    state.key = Some(key);
    for e in state.entities.drain(..) {
        commands.entity(e).despawn();
    }

    if !knobs.tree_enabled {
        return;
    }

    let world_size = cj.world_size as f32;
    let cell = cj.cell_size as f32;
    let (w, h) = (cj.field_w as i32, cj.field_h as i32);
    let (ox, oy) = (cj.origin_x as f32, cj.origin_y as f32);
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

    // Trunk: muted building-base tone (white-city wood, not brown clutter).
    // Canopy: theme park green with per-tree jitter.
    let trunk = {
        let c = palette::building_base().to_srgba();
        Color::srgb(
            (c.red * 0.9).clamp(0.0, 1.0),
            (c.green * 0.9).clamp(0.0, 1.0),
            (c.blue * 0.9).clamp(0.0, 1.0),
        )
    };
    let canopy_base = palette::park();

    for gy in 0..h {
        for gx in 0..w {
            let idx = (gy * w + gx) as usize;
            if f.parks.get(idx).copied().unwrap_or(0) < 1 {
                continue;
            }
            let r = hash01(gx, gy);
            if r > TREE_DENSITY {
                continue;
            }
            // Jittered position inside the cell; deterministic per cell.
            let jx = (hash01(gx.wrapping_mul(3), gy) - 0.5) * cell * 0.8;
            let jz = (hash01(gx, gy.wrapping_mul(5)) - 0.5) * cell * 0.8;
            let x = ox + gx as f32 * cell + cell * 0.5 + jx;
            let z = oy + gy as f32 * cell + cell * 0.5 + jz;
            let ground = height_at.sample(x, z);
            let scale = 0.8 + r * 0.9;
            let trunk_h = 2.2 * scale;
            let canopy = 3.6 * scale;
            let tint = 1.0 + (hash01(gx.wrapping_add(9), gy.wrapping_sub(4)) - 0.5) * 0.24;
            let canopy_col = {
                let c = canopy_base.to_srgba();
                Color::srgb(
                    (c.red * tint).clamp(0.0, 1.0),
                    (c.green * tint).clamp(0.0, 1.0),
                    (c.blue * tint).clamp(0.0, 1.0),
                )
            };
            let half = world_size * 0.5;
            let cx = (((x + half) / world_size) * CHUNKS_PER_SIDE as f32)
                .clamp(0.0, (CHUNKS_PER_SIDE - 1) as f32) as usize;
            let cz = (((z + half) / world_size) * CHUNKS_PER_SIDE as f32)
                .clamp(0.0, (CHUNKS_PER_SIDE - 1) as f32) as usize;
            let buf = &mut bufs[cz * CHUNKS_PER_SIDE + cx];
            append_cuboid(
                buf,
                Vec2::new(x, z),
                ground,
                0.35 * scale,
                0.35 * scale,
                trunk_h,
                trunk,
                trunk,
                trunk,
            );
            append_cuboid(
                buf,
                Vec2::new(x, z),
                ground + trunk_h,
                canopy * 0.5,
                canopy * 0.5,
                canopy,
                canopy_col,
                canopy_col,
                canopy_col,
            );
        }
    }

    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: knobs.unlit_material,
        perceptual_roughness: 1.0,
        reflectance: 0.0,
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
                Visibility::default(),
                TreeChunk { center: centers[i] },
                Name::new("park-trees"),
            ))
            .id();
        state.entities.push(e);
    }
}

fn tree_draw_distance_system(
    quality: Res<QualityTier>,
    chunks: Query<(Entity, &TreeChunk)>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut visibility: Query<&mut Visibility>,
    counters: Res<crate::perf::PerfCounters>,
) {
    let _span = tracing::info_span!("tree_draw_distance").entered();
    let _timer = crate::perf::PerfSpan::start(&counters.tree_draw_distance_us);
    let Ok(cam) = cameras.single() else {
        return;
    };
    let cam_xz = Vec2::new(cam.translation.x, cam.translation.z);
    let max_dist = quality.knobs().tree_draw_distance_m;
    for (entity, chunk) in &chunks {
        let Ok(mut vis) = visibility.get_mut(entity) else {
            continue;
        };
        let visible = match max_dist {
            None => true,
            Some(limit) => cam_xz.distance(chunk.center) <= limit,
        };
        let next = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        crate::perf::set_visibility_if_changed(&mut vis, next, Some(&counters));
    }
}
