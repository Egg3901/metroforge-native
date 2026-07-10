//! Ground mesh (spec §3.3 `terrain.rs`): rebuilt whenever `LatestFields`'s
//! `version` (or the quality tier's subdivision divisor) changes. Also owns
//! replacing `mf_state::HeightAt`'s placeholder flat-ground closure with a
//! real bilinear sampler once fields have arrived, so every other layer
//! (roads/buildings/transit/vehicles/agents/camera) can position things on
//! the ground.

use std::sync::Arc;

use bevy::prelude::*;

use mf_state::{CurrentCity, HeightAt, LatestFields, QualityTier};

use crate::mesh_utils::MeshBuffers;
use crate::palette;

/// Max relief per spec §3.3 ("max relief 200-400 m") — picked the midpoint.
pub const TERRAIN_Z_SCALE: f32 = 300.0;
/// Water is rendered dead flat, slightly below the nominal ground plane so
/// shorelines don't z-fight with adjacent land vertices.
pub const WATER_LEVEL_Y: f32 = -0.4;

/// Marker on the terrain ground mesh so other layers (subway view dim) can
/// find its material without reaching into `TerrainState`.
#[derive(Component)]
pub struct TerrainSurface;

pub struct MfTerrainPlugin;

impl Plugin for MfTerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainState>().add_systems(
            Update,
            (
                build_terrain_system,
                apply_quality_to_terrain_material_system,
            )
                .in_set(crate::MfRenderSet::Terrain),
        );
    }
}

#[derive(Resource, Default)]
struct TerrainState {
    /// `(fields.version, subdiv_divisor)` — the geometry-affecting knobs.
    key: Option<(u32, u32)>,
    entity: Option<Entity>,
    material: Option<Handle<StandardMaterial>>,
}

/// Real bilinear terrain sampler (replaces `mf_state::HeightAt`'s flat-
/// ground placeholder). Cheap to clone (all backing arrays are `Arc`), so
/// the `HeightAt` closure can hold one directly.
struct TerrainSampleData {
    field_w: i32,
    field_h: i32,
    cell_size: f32,
    origin_x: f32,
    origin_y: f32,
    terrain: Arc<Vec<f32>>,
    water: Arc<Vec<u8>>,
}

impl TerrainSampleData {
    fn bilinear_f32(&self, arr: &[f32], gx: f32, gy: f32) -> f32 {
        let (x0, y0, x1, y1, tx, ty) = self.corners(gx, gy);
        let w = self.field_w;
        let v00 = arr[(y0 * w + x0) as usize];
        let v10 = arr[(y0 * w + x1) as usize];
        let v01 = arr[(y1 * w + x0) as usize];
        let v11 = arr[(y1 * w + x1) as usize];
        (v00 * (1.0 - tx) + v10 * tx) * (1.0 - ty) + (v01 * (1.0 - tx) + v11 * tx) * ty
    }

    fn bilinear_u8(&self, arr: &[u8], gx: f32, gy: f32) -> f32 {
        let (x0, y0, x1, y1, tx, ty) = self.corners(gx, gy);
        let w = self.field_w;
        let v00 = arr[(y0 * w + x0) as usize] as f32;
        let v10 = arr[(y0 * w + x1) as usize] as f32;
        let v01 = arr[(y1 * w + x0) as usize] as f32;
        let v11 = arr[(y1 * w + x1) as usize] as f32;
        (v00 * (1.0 - tx) + v10 * tx) * (1.0 - ty) + (v01 * (1.0 - tx) + v11 * tx) * ty
    }

    #[allow(clippy::type_complexity)]
    fn corners(&self, gx: f32, gy: f32) -> (i32, i32, i32, i32, f32, f32) {
        let x0 = gx.floor().clamp(0.0, (self.field_w - 1) as f32) as i32;
        let y0 = gy.floor().clamp(0.0, (self.field_h - 1) as f32) as i32;
        let x1 = (x0 + 1).min(self.field_w - 1);
        let y1 = (y0 + 1).min(self.field_h - 1);
        let tx = (gx - x0 as f32).clamp(0.0, 1.0);
        let ty = (gy - y0 as f32).clamp(0.0, 1.0);
        (x0, y0, x1, y1, tx, ty)
    }

    /// `(x, z)` here is world X / Bevy Z (coordinate convention: world Y ->
    /// Bevy Z).
    fn sample(&self, x: f32, z: f32) -> f32 {
        if self.field_w < 2 || self.field_h < 2 {
            return 0.0;
        }
        let gx = (x - self.origin_x) / self.cell_size;
        let gy = (z - self.origin_y) / self.cell_size;
        let water_frac = self.bilinear_u8(&self.water, gx, gy);
        if water_frac > 0.5 {
            return WATER_LEVEL_Y;
        }
        self.bilinear_f32(&self.terrain, gx, gy) * TERRAIN_Z_SCALE
    }
}

#[allow(clippy::too_many_arguments)]
fn build_terrain_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<LatestFields>,
    quality: Res<QualityTier>,
    mut state: ResMut<TerrainState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut height_at: ResMut<HeightAt>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    let Some(f) = &fields.0 else {
        return;
    };
    let divisor = quality.knobs().terrain_subdiv_divisor.max(1);
    let key = (f.version, divisor);
    if state.key == Some(key) {
        return;
    }
    state.key = Some(key);

    if let Some(e) = state.entity.take() {
        commands.entity(e).despawn();
    }

    let field_w = city_json.field_w;
    let field_h = city_json.field_h;
    if field_w < 2 || field_h < 2 || f.terrain.len() != (field_w * field_h) as usize {
        return;
    }
    let cell_size = city_json.cell_size as f32;
    let origin_x = city_json.origin_x as f32;
    let origin_y = city_json.origin_y as f32;

    // Stepped grid indices for this tier's subdivision divisor, always
    // including the far edge so the mesh reaches the city's full extent.
    let stepped = |n: u32, step: u32| -> Vec<u32> {
        let mut v: Vec<u32> = (0..n).step_by(step as usize).collect();
        if *v.last().unwrap_or(&0) != n - 1 {
            v.push(n - 1);
        }
        v
    };
    let xs = stepped(field_w, divisor);
    let ys = stepped(field_h, divisor);

    let mut buf = MeshBuffers::new();
    let ground = palette::ground();
    let water = palette::water();
    let park = palette::park();

    let vertex_at = |ix: usize, iy: usize| -> (Vec3, Color) {
        let gx = xs[ix];
        let gy = ys[iy];
        let idx = (gy * field_w + gx) as usize;
        let is_water = f.water.get(idx).copied().unwrap_or(0) >= 1;
        let is_park = f.parks.get(idx).copied().unwrap_or(0) >= 1;
        let y = if is_water {
            WATER_LEVEL_Y
        } else {
            f.terrain.get(idx).copied().unwrap_or(0.0) * TERRAIN_Z_SCALE
        };
        let color = if is_water {
            water
        } else if is_park {
            park
        } else {
            ground
        };
        let x = origin_x + gx as f32 * cell_size;
        let z = origin_y + gy as f32 * cell_size;
        (Vec3::new(x, y, z), color)
    };

    for iy in 0..ys.len().saturating_sub(1) {
        for ix in 0..xs.len().saturating_sub(1) {
            let (p00, c00) = vertex_at(ix, iy);
            let (p10, c10) = vertex_at(ix + 1, iy);
            let (p11, c11) = vertex_at(ix + 1, iy + 1);
            let (p01, c01) = vertex_at(ix, iy + 1);
            buf.push_quad(p00, p10, p11, p01, Vec3::Y, c00, c10, c11, c01);
        }
    }
    if buf.is_empty() {
        return;
    }
    let mesh = meshes.add(buf.build());

    let unlit = quality.knobs().unlit_material;
    let material = materials.add(StandardMaterial {
        double_sided: true,
        cull_mode: None,
        base_color: Color::WHITE,
        unlit,
        perceptual_roughness: 1.0,
        ..default()
    });
    state.material = Some(material.clone());

    let entity = commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::default(),
            TerrainSurface,
        ))
        .id();
    state.entity = Some(entity);

    let data = Arc::new(TerrainSampleData {
        field_w: field_w as i32,
        field_h: field_h as i32,
        cell_size,
        origin_x,
        origin_y,
        terrain: Arc::new(f.terrain.clone()),
        water: Arc::new(f.water.clone()),
    });
    height_at.0 = Box::new(move |x, z| data.sample(x, z));
}

/// If only the quality tier changed (not the fields version/subdivision —
/// that's the full-rebuild path above), just flip the existing material's
/// `unlit` flag rather than rebuilding the whole ground mesh.
fn apply_quality_to_terrain_material_system(
    quality: Res<QualityTier>,
    state: Res<TerrainState>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !quality.is_changed() {
        return;
    }
    let Some(handle) = &state.material else {
        return;
    };
    if let Some(mat) = materials.get_mut(handle) {
        mat.unlit = quality.knobs().unlit_material;
    }
}
