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
//! Driven by the `outline_enabled` knob via [`EffectiveKnobs`] (preset
//! merged with Advanced overrides). Originally High-only per spec, it is
//! now on for every tier: the tier-truth pass found this one-chunk draw is
//! the single biggest readability win for the unlit Potato/Low tiers (flat
//! white massing with no lighting reads as edgeless mush without it), and
//! the per-chunk scoping keeps it affordable even on Potato. Turning the
//! knob off despawns the outline entity so the cost is exactly zero when
//! off, not just visually hidden.

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

/// Camera-distance fade band for the outline (issue: "black scribble clump").
///
/// The dense-center chunk is ONE merged mesh of hundreds of packed buildings.
/// Inverted-hull pushes every face out `OUTLINE_PUSH_M`, including the roof
/// caps. Up close you view the core from the side, so the pushed roof caps
/// hide behind taller fronts and only the silhouette rim shows -- the intended
/// crisp hairline. But at the far autostart camera (`zoom_to_fit` =
/// worldSize * 0.75 = ~9km on NYC) you look down across the whole core and
/// every pushed-up black roof cap tiles together into one solid black blanket
/// -- the "black scribble clump".
///
/// The packed sub-pixel buildings mean there is no "thin" intermediate at
/// this distance: any nonzero push tiles the per-building black slivers into a
/// solid fill (measured -- even a 16% push still blobs). So the fix scales the
/// push down over the fade band AND despawns-by-hiding the entity past `FAR`,
/// which is the sanctioned trade (outlines are a close-range readability aid;
/// at a 9km overview a crisp edge was never going to survive anyway). Below
/// `NEAR` (all gameplay / editing distances and the verify harness' 1.4km hero
/// shot) the outline is at full strength.
const OUTLINE_FADE_NEAR_M: f32 = 3_000.0;
/// Beyond this camera distance the outline is fully hidden (push already ~0).
const OUTLINE_FADE_FAR_M: f32 = 5_500.0;

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
                (
                    maintain_outline_system,
                    outline_distance_fade_system.after(maintain_outline_system),
                )
                    .in_set(crate::MfRenderSet::Dynamic),
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
    /// Handle to the live outline material so `outline_distance_fade_system`
    /// can drive its push distance toward zero as the camera dollies out.
    material: Option<Handle<OutlineMaterial>>,
    /// World-space center of the dense-center chunk the outline copies, used
    /// as the camera-distance anchor for the fade.
    center: Vec3,
}

fn maintain_outline_system(
    effective: Res<EffectiveKnobs>,
    dense: Res<BuildingsDenseCenter>,
    chunks: Query<(Entity, &BuildingChunk, &Mesh3d)>,
    mut state: ResMut<OutlineState>,
    mut commands: Commands,
    mut materials: ResMut<Assets<OutlineMaterial>>,
) {
    if !effective.0.outline_enabled {
        if let Some(e) = state.entity.take() {
            commands.entity(e).try_despawn();
        }
        state.source_mesh = None;
        state.material = None;
        return;
    }

    let target = dense.0;
    let nearest = chunks.iter().min_by(|(_, a, _), (_, b, _)| {
        a.center
            .distance_squared(target)
            .total_cmp(&b.center.distance_squared(target))
    });
    let Some((_source_entity, chunk, mesh3d)) = nearest else {
        return;
    };
    state.center = Vec3::new(chunk.center.x, 0.0, chunk.center.y);

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
            MeshMaterial3d(material.clone()),
            Transform::IDENTITY,
            Visibility::Visible,
            NotShadowCaster,
            NotShadowReceiver,
            Name::new("building-outline-dense-center"),
        ))
        .id();
    state.entity = Some(entity);
    state.source_mesh = Some(mesh3d.0.id());
    state.material = Some(material);
}

/// Scales the outline push toward zero as the camera dollies out and hides the
/// entity past [`OUTLINE_FADE_FAR_M`], so the dense-center slivers never merge
/// into a solid black blob at far (autostart `zoom_to_fit`) distances. Up close
/// the push is at full [`OUTLINE_PUSH_M`] strength so edges stay crisp. See the
/// fade-band constants for the full rationale.
fn outline_distance_fade_system(
    state: Res<OutlineState>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut materials: ResMut<Assets<OutlineMaterial>>,
    mut vis: Query<&mut Visibility>,
) {
    let Some(entity) = state.entity else {
        return;
    };
    // Nearest camera to the outline anchor (there is normally exactly one 3D
    // camera; min picks sanely if a promo/photo camera coexists).
    let Some(cam_dist) = cameras
        .iter()
        .map(|t| t.translation().distance(state.center))
        .min_by(f32::total_cmp)
    else {
        return;
    };

    // 0 at/below NEAR, 1 at/above FAR, smooth in between.
    let t = ((cam_dist - OUTLINE_FADE_NEAR_M) / (OUTLINE_FADE_FAR_M - OUTLINE_FADE_NEAR_M))
        .clamp(0.0, 1.0);
    let fade = 1.0 - t * t * (3.0 - 2.0 * t); // 1.0 -> 0.0 via smoothstep

    if let Some(handle) = &state.material {
        let push = OUTLINE_PUSH_M * fade;
        // Only touch the asset (and trigger a GPU re-upload) when the push
        // actually moves — a static camera must not re-upload every frame.
        let needs_write = materials
            .get(handle)
            .is_some_and(|m| (m.params.x - push).abs() > 1e-4);
        if needs_write {
            if let Some(mat) = materials.get_mut(handle) {
                mat.params.x = push;
            }
        }
    }

    if let Ok(mut v) = vis.get_mut(entity) {
        let want = if cam_dist >= OUTLINE_FADE_FAR_M {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
        if *v != want {
            *v = want;
        }
    }
}
