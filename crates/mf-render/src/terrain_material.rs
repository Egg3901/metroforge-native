//! Terrain material extension: scrolling cloud-shadow projection.
//!
//! Terrain stays an `ExtendedMaterial<StandardMaterial, TerrainExtension>`
//! so the ground can sample the same [`crate::atmosphere::CloudShadowParams`]
//! noise as buildings. Potato/Low leave `cloud.z == 0` (no darkening).

use bevy::asset::{load_internal_asset, weak_handle};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderRef};

pub type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExtension>;

const TERRAIN_SHADER_HANDLE: Handle<Shader> = weak_handle!("a1c4e8f2-5b7d-4e9a-8c3f-1d2e3f4a5b6c");

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct TerrainExtension {
    /// Cloud shadow: (offset_u, offset_v, strength, inv_scale).
    #[uniform(100)]
    pub cloud: Vec4,
    /// Weather: (snow_depth, _, _, _). `snow_depth` (0..1) lerps the lit
    /// ground/park output toward white so accumulation reads on the already
    /// white city without touching baked vertex colors (v0.7). The remaining
    /// lanes are reserved so the uniform can grow without a bind-group churn.
    #[uniform(103)]
    pub weather: Vec4,
    #[texture(101)]
    #[sampler(102)]
    pub cloud_noise: Option<Handle<Image>>,
}

impl Default for TerrainExtension {
    fn default() -> Self {
        TerrainExtension {
            cloud: Vec4::new(0.0, 0.0, 0.0, 1.0 / 1_100.0),
            weather: Vec4::ZERO,
            cloud_noise: None,
        }
    }
}

impl MaterialExtension for TerrainExtension {
    fn fragment_shader() -> ShaderRef {
        TERRAIN_SHADER_HANDLE.into()
    }
}

pub struct MfTerrainMaterialPlugin;

impl Plugin for MfTerrainMaterialPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(
            app,
            TERRAIN_SHADER_HANDLE,
            "terrain_cloud.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(MaterialPlugin::<TerrainMaterial>::default());
    }
}
