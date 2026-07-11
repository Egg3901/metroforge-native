//! Stylized water material (owner ask: real-looking water that stays
//! Mirror's-Edge-clean, not photoreal). Potato keeps flat vertex-color water
//! baked into the terrain mesh (zero extra fill — llvmpipe release smoke).
//! Low/Medium/High spawn a separate opaque water mesh with this custom WGSL
//! shader (`water.wgsl`, embedded via `load_internal_asset` like sky/outline).
//!
//! Geometry is built in `terrain.rs` (it already owns the water mask /
//! shoreline `water_frac`); this module owns the material, uniform sync, and
//! the small UV-carrying mesh buffer used for that water draw.

use bevy::asset::{load_internal_asset, weak_handle};
use bevy::pbr::{Material, NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{AsBindGroup, ShaderRef};

use mf_state::{QualityTier, SubwayView};

use crate::daynight::DayNightState;
use crate::palette;

/// Slightly above [`crate::terrain::WATER_LEVEL_Y`] so the water overlay
/// does not z-fight shoreline land verts that share the same flat Y.
pub const WATER_SURFACE_Y: f32 = crate::terrain::WATER_LEVEL_Y + 0.05;

const WATER_SHADER_HANDLE: Handle<Shader> = weak_handle!("a3c8e1f2-4b5d-6e7f-8a9b-0c1d2e3f4a5b");

/// Marker on the stylized water mesh so subway dim / material sync can find
/// it without reaching into terrain state.
#[derive(Component)]
pub struct WaterSurface;

pub struct MfWaterPlugin;

impl Plugin for MfWaterPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, WATER_SHADER_HANDLE, "water.wgsl", Shader::from_wgsl);
        app.add_plugins(bevy::pbr::MaterialPlugin::<WaterMaterial>::default())
            .add_systems(
                Update,
                update_water_uniforms_system.in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

/// Standalone water `Material` (not an `ExtendedMaterial` — same pattern as
/// `SkyMaterial` / `OutlineMaterial`). All fields share `#[uniform(0)]` so
/// `AsBindGroup` packs them into one buffer matching `WaterUniform` in
/// `water.wgsl`.
#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct WaterMaterial {
    #[uniform(0)]
    pub water_color: Vec4,
    #[uniform(0)]
    pub sky_color: Vec4,
    #[uniform(0)]
    pub foam_color: Vec4,
    /// (sun_direction.xyz, sun_elevation).
    #[uniform(0)]
    pub sun: Vec4,
    /// (time_secs, night_factor, quality 1|2, subway_dim).
    #[uniform(0)]
    pub params: Vec4,
    /// (night shimmer rgb, shimmer strength).
    #[uniform(0)]
    pub shimmer: Vec4,
}

impl Material for WaterMaterial {
    fn vertex_shader() -> ShaderRef {
        WATER_SHADER_HANDLE.into()
    }

    fn fragment_shader() -> ShaderRef {
        WATER_SHADER_HANDLE.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        // Opaque on purpose: NYC harbors are huge, and a transparent water
        // pass would tank fill rate (and the release llvmpipe smoke). Foam /
        // fresnel are composited in-shader, not via blending.
        AlphaMode::Opaque
    }
}

impl Default for WaterMaterial {
    fn default() -> Self {
        WaterMaterial {
            water_color: color_to_vec4(palette::water()),
            sky_color: color_to_vec4(palette::sky_day()),
            foam_color: Vec4::new(0.85, 0.92, 0.95, 1.0),
            sun: Vec4::new(0.0, 1.0, 0.0, 1.0),
            params: Vec4::new(0.0, 0.0, 2.0, 1.0),
            shimmer: Vec4::new(0.35, 0.45, 0.65, 0.12),
        }
    }
}

/// Build a default [`WaterMaterial`] for the current theme / quality tier.
pub fn make_water_material(quality: QualityTier) -> WaterMaterial {
    WaterMaterial {
        water_color: color_to_vec4(palette::water()),
        params: Vec4::new(0.0, 0.0, quality.knobs().water_quality.max(1) as f32, 1.0),
        ..Default::default()
    }
}

/// Components to attach when spawning the water mesh entity.
pub fn water_bundle(
    mesh: Handle<Mesh>,
    material: Handle<WaterMaterial>,
) -> (
    Mesh3d,
    MeshMaterial3d<WaterMaterial>,
    Transform,
    Visibility,
    NotShadowCaster,
    NotShadowReceiver,
    Name,
    WaterSurface,
) {
    (
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        Visibility::default(),
        // Water never casts/receives shadows: NYC-scale harbors would blow
        // the shadow-map budget, and flat water with baked cel lighting
        // reads cleaner without self-shadow acne.
        NotShadowCaster,
        NotShadowReceiver,
        Name::new("water-surface"),
        WaterSurface,
    )
}

/// Mesh buffer for the water overlay: position / normal / UV0 (water_frac).
/// Kept separate from `MeshBuffers` so land meshes stay UV-free.
#[derive(Default)]
pub struct WaterMeshBuffers {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl WaterMeshBuffers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_quad(
        &mut self,
        p0: Vec3,
        p1: Vec3,
        p2: Vec3,
        p3: Vec3,
        f0: f32,
        f1: f32,
        f2: f32,
        f3: f32,
    ) {
        let base = self.positions.len() as u32;
        let n = Vec3::Y.to_array();
        for (p, f) in [(p0, f0), (p1, f1), (p2, f2), (p3, f3)] {
            self.positions.push(p.to_array());
            self.normals.push(n);
            self.uvs.push([f, 0.0]);
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    pub fn build(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

fn color_to_vec4(color: Color) -> Vec4 {
    let srgba = color.to_srgba();
    Vec4::new(srgba.red, srgba.green, srgba.blue, srgba.alpha)
}

/// Pushes theme / day-night / quality / subway dim into every live water
/// material. Time advances every frame on Medium/High so ripples scroll;
/// Low freezes time in-shader via `params.z < 2`. One material write per
/// frame is cheap next to the city draw, so we always sync rather than
/// fighting `SubwayView`'s every-frame `ResMut` change ticks.
fn update_water_uniforms_system(
    time: Res<Time>,
    quality: Res<QualityTier>,
    day_night: Res<DayNightState>,
    subway: Res<SubwayView>,
    surfaces: Query<&MeshMaterial3d<WaterMaterial>, With<WaterSurface>>,
    mut materials: ResMut<Assets<WaterMaterial>>,
) {
    let knobs = quality.knobs();
    if knobs.water_quality == 0 {
        return;
    }
    if surfaces.is_empty() {
        return;
    }
    // Match subway.rs's terrain dim: `1.0 - t * GROUND_DIM` with GROUND_DIM=0.28.
    let dim = 1.0 - subway.t * 0.28;
    let water = color_to_vec4(palette::water());
    let sky = color_to_vec4(palette::sky_day().mix(&palette::sky_night(), day_night.night_factor));
    let horizon = {
        let s = sky;
        Vec4::new(
            s.x + (1.0 - s.x) * 0.35,
            s.y + (1.0 - s.y) * 0.35,
            s.z + (1.0 - s.z) * 0.35,
            1.0,
        )
    };
    let night = day_night.night_factor;
    let foam = Vec4::new(
        0.85 + (0.45 - 0.85) * night,
        0.92 + (0.55 - 0.92) * night,
        0.95 + (0.70 - 0.95) * night,
        1.0,
    );
    let sun = Vec4::new(
        day_night.sun_direction.x,
        day_night.sun_direction.y,
        day_night.sun_direction.z,
        day_night.sun_elevation,
    );
    let q = knobs.water_quality.max(1) as f32;
    let t = if knobs.water_quality >= 2 {
        time.elapsed_secs()
    } else {
        0.0
    };
    let params = Vec4::new(t, night, q, dim);
    let shimmer = Vec4::new(0.35, 0.5, 0.75, 0.14);

    for handle in &surfaces {
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.water_color = water;
            mat.sky_color = horizon;
            mat.foam_color = foam;
            mat.sun = sun;
            mat.params = params;
            mat.shimmer = shimmer;
        }
    }
}
