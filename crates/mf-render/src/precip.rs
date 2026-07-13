//! View-space instanced precipitation particles (rain / snow), v0.7.
//!
//! ONE mesh, ONE draw per active precip type: a static vertex buffer of unit
//! quads whose world positions are computed entirely in `precip.wgsl`'s vertex
//! shader from `time` + the camera position. The CPU only ever writes a small
//! uniform per frame (never a per-particle transform), so this is GPU-friendly
//! and has no per-pixel CPU loop.
//!
//! Tier gating (spec §art-direction §4, BINDING): particle count scales with
//! the quality tier and Potato draws **none** (weather there is the fog-density
//! change in `lib.rs` plus the HUD icon). Snow reuses the same instanced mesh
//! with slower, drifting, rounder parameters.

use bevy::asset::{load_internal_asset, weak_handle, RenderAssetUsages};
use bevy::pbr::{
    Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin, NotShadowCaster,
    NotShadowReceiver,
};
use bevy::prelude::*;
use bevy::render::mesh::{
    Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology,
};
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderRef, SpecializedMeshPipelineError, VertexFormat,
};
use bevy::render::view::NoFrustumCulling;

use mf_state::{QualityTier, WeatherEffects, WeatherRender};

use crate::atmosphere::AtmosphereWind;

const PRECIP_SHADER_HANDLE: Handle<Shader> = weak_handle!("b7e2c9a4-3d61-4f28-9a75-6c1e8f0d2b3a");

/// Per-vertex quad corner: x in [-0.5, 0.5] (across), y in [0, 1] (along).
const ATTRIBUTE_CORNER: MeshVertexAttribute =
    MeshVertexAttribute::new("Precip_Corner", 0x9E010001, VertexFormat::Float32x2);
/// Per-particle seed (replicated across the quad's 4 vertices).
const ATTRIBUTE_SEED: MeshVertexAttribute =
    MeshVertexAttribute::new("Precip_Seed", 0x9E010002, VertexFormat::Float32);

/// Particle counts per tier (spec: High ~4k / Medium ~1.5k / Low ~400 /
/// Potato 0). Snow reuses the same mesh, so one count table covers both.
fn tier_count(tier: QualityTier) -> u32 {
    match tier {
        QualityTier::Potato => 0,
        QualityTier::Low => 400,
        QualityTier::Medium => 1_500,
        QualityTier::High => 4_000,
    }
}

/// The four Vec4s the precip shader packs into one bind-group-2 uniform
/// buffer (AsBindGroup merges same-`uniform(0)` Vec4 fields — same pattern as
/// `OutlineMaterial` / `RevealExtension`). Kept as a plain data struct so the
/// resolve/look code stays readable; flattened onto the material on write.
#[derive(Clone, Copy)]
struct PrecipData {
    /// (time_secs, fall_speed, radius_m, vertical_span_m).
    p0: Vec4,
    /// (streak_len_m, width_m, sway_freq, kind[0 rain,1 snow]).
    p1: Vec4,
    /// (wind_x, wind_z, sway_amp_m, streak_align[0 up,1 fall]).
    wind: Vec4,
    /// (r, g, b, alpha).
    tint: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct PrecipMaterial {
    #[uniform(0)]
    p0: Vec4,
    #[uniform(0)]
    p1: Vec4,
    #[uniform(0)]
    wind: Vec4,
    #[uniform(0)]
    tint: Vec4,
}

impl PrecipMaterial {
    fn from_data(d: PrecipData) -> Self {
        PrecipMaterial {
            p0: d.p0,
            p1: d.p1,
            wind: d.wind,
            tint: d.tint,
        }
    }
    fn set(&mut self, d: PrecipData) {
        self.p0 = d.p0;
        self.p1 = d.p1;
        self.wind = d.wind;
        self.tint = d.tint;
    }
}

impl Material for PrecipMaterial {
    fn vertex_shader() -> ShaderRef {
        PRECIP_SHADER_HANDLE.into()
    }
    fn fragment_shader() -> ShaderRef {
        PRECIP_SHADER_HANDLE.into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
    fn specialize(
        _pipeline: &MaterialPipeline<Self>,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vertex_layout = layout.0.get_layout(&[
            ATTRIBUTE_CORNER.at_shader_location(0),
            ATTRIBUTE_SEED.at_shader_location(1),
        ])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        // Particles are double-sided billboards.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

pub struct MfPrecipPlugin;

impl Plugin for MfPrecipPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, PRECIP_SHADER_HANDLE, "precip.wgsl", Shader::from_wgsl);
        app.add_plugins(MaterialPlugin::<PrecipMaterial>::default())
            .init_resource::<PrecipState>()
            .add_systems(
                Update,
                maintain_precip_system.in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

#[derive(Resource, Default)]
struct PrecipState {
    entity: Option<Entity>,
    material: Option<Handle<PrecipMaterial>>,
    mesh: Option<Handle<Mesh>>,
    built_count: u32,
}

/// Build a static mesh of `count` unit quads (4 verts / 6 indices each). Only
/// `corner` + `seed` attributes; positions are synthesized in the shader.
fn build_precip_mesh(count: u32) -> Mesh {
    let n = count as usize;
    let mut corners: Vec<[f32; 2]> = Vec::with_capacity(n * 4);
    let mut seeds: Vec<f32> = Vec::with_capacity(n * 4);
    let mut indices: Vec<u32> = Vec::with_capacity(n * 6);
    let quad = [[-0.5f32, 0.0], [0.5, 0.0], [0.5, 1.0], [-0.5, 1.0]];
    for i in 0..n {
        let base = (i * 4) as u32;
        // Spread seeds across a wide range so hashes decorrelate.
        let seed = (i as f32) * 1.618_034 + 0.5;
        for c in quad {
            corners.push(c);
            seeds.push(seed);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(ATTRIBUTE_CORNER, corners);
    mesh.insert_attribute(ATTRIBUTE_SEED, seeds);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Which precip look the current weather calls for.
struct PrecipLook {
    data: PrecipData,
    visible: bool,
}

fn resolve_look(weather: &WeatherRender, wind_dir: Vec2, elapsed: f32) -> PrecipLook {
    let rain = weather.rain.clamp(0.0, 1.0);
    let snow = weather.snow.clamp(0.0, 1.0);
    let storm = weather.storm.clamp(0.0, 1.0);
    // Exclusive states in practice; if both are up mid-transition, the larger
    // weight wins the look and its alpha carries the crossfade.
    let snowy = snow > rain;
    let amount = rain.max(snow);
    if amount < 0.02 {
        return PrecipLook {
            data: zero_data(),
            visible: false,
        };
    }

    if snowy {
        PrecipLook {
            data: PrecipData {
                p0: Vec4::new(elapsed, 9.0, 300.0, 480.0),
                p1: Vec4::new(2.4, 2.4, 0.55, 1.0),
                wind: Vec4::new(wind_dir.x * 6.0, wind_dir.y * 6.0, 7.0, 0.0),
                tint: Vec4::new(1.0, 1.0, 1.0, 0.85 * snow),
            },
            visible: true,
        }
    } else {
        // Storm rains harder, faster, more slanted.
        let speed = 110.0 + storm * 60.0;
        let slant = 28.0 + storm * 40.0;
        PrecipLook {
            data: PrecipData {
                p0: Vec4::new(elapsed, speed, 340.0, 620.0),
                p1: Vec4::new(16.0 + storm * 10.0, 0.35, 0.0, 0.0),
                wind: Vec4::new(wind_dir.x * slant, wind_dir.y * slant, 0.0, 1.0),
                tint: Vec4::new(0.74, 0.79, 0.90, (0.45 + storm * 0.2) * rain),
            },
            visible: true,
        }
    }
}

fn zero_data() -> PrecipData {
    PrecipData {
        p0: Vec4::new(0.0, 1.0, 300.0, 480.0),
        p1: Vec4::new(1.0, 1.0, 0.0, 0.0),
        wind: Vec4::ZERO,
        tint: Vec4::ZERO,
    }
}

#[allow(clippy::too_many_arguments)]
fn maintain_precip_system(
    time: Res<Time>,
    quality: Res<QualityTier>,
    effects: Res<WeatherEffects>,
    weather: Res<WeatherRender>,
    wind: Res<AtmosphereWind>,
    mut state: ResMut<PrecipState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PrecipMaterial>>,
    mut vis: Query<&mut Visibility>,
) {
    let count = tier_count(*quality);
    if count == 0 {
        // Potato (or count->0): tear the field down entirely.
        if let Some(e) = state.entity.take() {
            commands.entity(e).try_despawn();
        }
        state.material = None;
        state.mesh = None;
        state.built_count = 0;
        return;
    }

    // (Re)build the mesh when the tier's count changes.
    if state.mesh.is_none() || state.built_count != count {
        if let Some(e) = state.entity.take() {
            commands.entity(e).try_despawn();
        }
        let mesh = meshes.add(build_precip_mesh(count));
        let material = materials.add(PrecipMaterial::from_data(zero_data()));
        let entity = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                Transform::IDENTITY,
                Visibility::Hidden,
                // Positions are synthesized in the shader around the camera —
                // the mesh's own AABB is meaningless, so never frustum-cull it.
                NoFrustumCulling,
                NotShadowCaster,
                NotShadowReceiver,
                Name::new("precip-particles"),
            ))
            .id();
        state.entity = Some(entity);
        state.mesh = Some(mesh);
        state.material = Some(material);
        state.built_count = count;
    }

    let wind_dir = Vec2::new(wind.heading.cos(), wind.heading.sin()) * wind.gust;
    let look = if effects.enabled {
        resolve_look(&weather, wind_dir, time.elapsed_secs())
    } else {
        PrecipLook {
            data: zero_data(),
            visible: false,
        }
    };

    if let Some(handle) = &state.material {
        if let Some(mat) = materials.get_mut(handle) {
            mat.set(look.data);
        }
    }
    if let Some(entity) = state.entity {
        if let Ok(mut v) = vis.get_mut(entity) {
            let want = if look.visible {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
            if *v != want {
                *v = want;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_counts_scale_and_potato_is_zero() {
        assert_eq!(tier_count(QualityTier::Potato), 0);
        assert!(
            tier_count(QualityTier::Low) < tier_count(QualityTier::Medium)
                && tier_count(QualityTier::Medium) < tier_count(QualityTier::High)
        );
    }

    #[test]
    fn clear_weather_hides_the_field() {
        let w = WeatherRender::default();
        let look = resolve_look(&w, Vec2::ZERO, 0.0);
        assert!(!look.visible);
    }

    #[test]
    fn rain_and_snow_pick_distinct_looks() {
        let mut rainy = WeatherRender::default();
        rainy.rain = 1.0;
        let r = resolve_look(&rainy, Vec2::X, 1.0);
        assert!(r.visible);
        assert_eq!(r.data.p1.w, 0.0, "rain kind flag (0 = rain)");
        assert_eq!(r.data.wind.w, 1.0, "rain streaks align along fall");

        let mut snowy = WeatherRender::default();
        snowy.snow = 1.0;
        let s = resolve_look(&snowy, Vec2::X, 1.0);
        assert!(s.visible);
        assert_eq!(s.data.p1.w, 1.0, "snow kind flag (1 = snow)");
        assert_eq!(s.data.wind.w, 0.0, "snow flakes billboard upright");
        // Snow is the slower faller.
        assert!(s.data.p0.y < r.data.p0.y);
    }
}
