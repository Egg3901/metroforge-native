//! Cel-shading black outlines on buildings (owner feedback, verbatim: "cel
//! shading/black outline for buildings (enhanced art direction)").
//!
//! ## Technique chosen: inverted-hull, NOT screen-space edge detection
//! Screen-space (depth/normal edge) post-processing was the other option on
//! the table. It was rejected here specifically because of how this scene is
//! built: `buildings.rs` already batches the whole city into 64 big chunk
//! meshes (see `BuildingChunk`), so there's no cheap way to get a
//! *building-only* edge mask out of a full-screen depth/normal buffer
//! without either (a) rendering buildings into a separate depth/normal
//! target (its own extra full-screen passes, on a software rasterizer where
//! every additional full-screen pass is comparatively expensive per
//! `MF_PERF_LOG` sampling — see PR description for numbers) or (b) accepting
//! outlines on terrain/road/water edges too, which is not the "cel-shaded
//! buildings" look asked for. Inverted-hull sidesteps both: it is geometry
//! that only exists where we choose to spawn it, so it is trivial to scope
//! to buildings only.
//!
//! ## Why only the dense-center chunk, not the whole city
//! The scene runs ~2.8M vertices on `llvmpipe`; inverted-hull doubles
//! whatever geometry it's applied to. Applying it city-wide was rejected
//! outright (would roughly double a multi-million-vertex draw on a software
//! rasterizer). Instead this duplicates exactly ONE of the 64 building
//! chunks each rebuild — the one straddling `BuildingsDenseCenter` (the
//! densest, most sight-line-visible cluster; the same "urban core" chunk
//! `mf-game`'s `promo.rs` frames its hero shots around) — which keeps the
//! extra draw a small, bounded fraction of total scene geometry regardless
//! of city size.
//!
//! ## Quality gate
//! Only active when [`EffectiveKnobs::outlines_enabled`] is set (High
//! preset / Advanced override). Otherwise despawns any existing outline
//! entity so the cost is exactly zero when off, not just visually hidden.

use bevy::pbr::{
    Material, MaterialPipeline, MaterialPipelineKey, NotShadowCaster, NotShadowReceiver,
};
use bevy::prelude::*;
use bevy::render::mesh::MeshVertexBufferLayoutRef;
use bevy::render::render_resource::{
    AsBindGroup, Face, RenderPipelineDescriptor, ShaderRef, SpecializedMeshPipelineError,
};

use bevy::asset::{load_internal_asset, weak_handle};
use mf_state::EffectiveKnobs;

use crate::buildings::{BuildingChunk, BuildingsDenseCenter};

/// World-space push distance along each vertex normal. Small on purpose --
/// "crisp thin dark edges ... NOT thick toon borders" (spec). Building walls
/// here run tens of meters, so ~0.9m reads as a hairline rim at normal
/// camera distances without visibly fattening silhouettes up close.
const OUTLINE_PUSH_M: f32 = 0.9;

const OUTLINE_SHADER_HANDLE: Handle<Shader> = weak_handle!("9f1d4b6a-1c3e-4f7a-8b2d-5e6f7a8b9c0d");

pub struct MfOutlinePlugin;

impl Plugin for MfOutlinePlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(
            app,
            OUTLINE_SHADER_HANDLE,
            "outline.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(bevy::pbr::MaterialPlugin::<OutlineMaterial>::default())
            .init_resource::<OutlineState>()
            .add_systems(
                Update,
                maintain_outline_system.in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct OutlineMaterial {
    #[uniform(0)]
    color: Vec4,
    /// (push_distance_m, reserved, reserved, reserved).
    #[uniform(0)]
    params: Vec4,
}

impl Material for OutlineMaterial {
    fn vertex_shader() -> ShaderRef {
        OUTLINE_SHADER_HANDLE.into()
    }

    fn fragment_shader() -> ShaderRef {
        OUTLINE_SHADER_HANDLE.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }

    /// Inverted-hull: cull the (pushed-out) FRONT faces so only the back
    /// faces survive -- those only peek past the original, unpushed surface
    /// at silhouette edges, which is exactly the thin-rim look. Crucially
    /// this only ever touches the standalone `OutlineMaterial` pipeline, not
    /// `BuildingMaterial`'s -- the base building material's `AlphaMode` and
    /// `cull_mode` are completely untouched by this file (see module doc /
    /// PR description for the vertex-color-on-Blend gotcha this avoids).
    fn specialize(
        _pipeline: &MaterialPipeline<Self>,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = Some(Face::Front);
        Ok(())
    }
}

/// Tracks which chunk entity's mesh the current outline copy was built from,
/// so `maintain_outline_system` only rebuilds when the dense-center chunk
/// actually changes (buildings rebuild -> new mesh assets, new chunk
/// entities) rather than re-spawning every frame.
#[derive(Resource, Default)]
struct OutlineState {
    entity: Option<Entity>,
    source_mesh: Option<AssetId<Mesh>>,
}

fn maintain_outline_system(
    effective: Res<EffectiveKnobs>,
    dense: Res<BuildingsDenseCenter>,
    chunks: Query<(Entity, &BuildingChunk, &Mesh3d)>,
    mut state: ResMut<OutlineState>,
    mut commands: Commands,
    mut materials: ResMut<Assets<OutlineMaterial>>,
) {
    if !effective.0.outlines_enabled {
        if let Some(e) = state.entity.take() {
            commands.entity(e).try_despawn();
        }
        state.source_mesh = None;
        return;
    }

    let target = dense.0;
    let nearest = chunks.iter().min_by(|(_, a, _), (_, b, _)| {
        a.center
            .distance_squared(target)
            .total_cmp(&b.center.distance_squared(target))
    });
    let Some((_source_entity, _chunk, mesh3d)) = nearest else {
        return;
    };

    if state.source_mesh == Some(mesh3d.0.id()) {
        return;
    }
    if let Some(e) = state.entity.take() {
        commands.entity(e).try_despawn();
    }

    let material = materials.add(OutlineMaterial {
        color: Vec4::new(0.0, 0.0, 0.0, 1.0),
        params: Vec4::new(OUTLINE_PUSH_M, 0.0, 0.0, 0.0),
    });
    let entity = commands
        .spawn((
            Mesh3d(mesh3d.0.clone()),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            NotShadowCaster,
            NotShadowReceiver,
            Name::new("building-outline-dense-center"),
        ))
        .id();
    state.entity = Some(entity);
    state.source_mesh = Some(mesh3d.0.id());
}
