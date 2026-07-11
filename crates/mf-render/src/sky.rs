//! Gradient sky dome (owner feedback, verbatim: "no skybox"). Adds a large,
//! theme- and day/night-aware gradient (horizon haze -> zenith) behind the
//! whole scene, plus horizon distance fog to seat the world edge.
//!
//! ## Dome vs Bevy 0.16 `Atmosphere`
//! Bevy 0.16 ships an `Atmosphere` component (Sébastien Hillaire's
//! physically-based multi-scatter sky). It was ruled out for two reasons,
//! not just perf:
//! 1. It renders a photoreal, physically-scattered sky (haze, sun disc,
//!    aerial perspective) — the owner's ask was explicitly "NOT a photo
//!    skybox", and a physically-based atmosphere reads exactly that way
//!    next to the flat vertex-color cel palette everywhere else in the
//!    scene. A hand-authored two-stop gradient is the art-direction-correct
//!    choice independent of cost.
//! 2. It is also the expensive option to even evaluate here: it requires a
//!    compute-shader multi-scatter LUT pass every frame, which is a much
//!    heavier ask of `llvmpipe` (software rasterizer, no real compute
//!    throughput) than one extra low-poly opaque mesh draw. Given (1)
//!    already rules it out on look alone, it was not felt necessary to spend
//!    render budget proving that in a real llvmpipe trace — a static
//!    gradient dome is strictly cheaper by construction (one unlit draw
//!    call, ~642 verts, no LUT passes, no compute pipeline).
//!
//! ## Why the dome follows the camera
//! The dome mesh is spawned once at Startup and re-centered on the camera
//! every frame (`follow_camera_system`) rather than left static at the world
//! origin: MetroForge's camera can dolly up to `MAX_DOLLY` (20km) from the
//! target, and cities aren't a fixed size, so a world-origin-anchored dome
//! would either have to be implausibly large or risk the camera ending up
//! outside it. Re-centering on the camera means a modest, fixed local radius
//! (see [`DOME_RADIUS`]) always fully encloses the visible frustum,
//! regardless of where in the city the camera currently sits — the same
//! trick real-time skyboxes use everywhere.

use bevy::asset::{load_internal_asset, weak_handle};
use bevy::pbr::{
    Material, MaterialPipeline, MaterialPipelineKey, NotShadowCaster, NotShadowReceiver,
};
use bevy::prelude::*;
use bevy::render::mesh::MeshVertexBufferLayoutRef;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderRef, SpecializedMeshPipelineError,
};

use mf_state::Theme;

use crate::daynight::DayNightState;
use crate::palette;

/// Local-space radius of the dome mesh. Must stay comfortably inside the
/// camera's far clip plane (`mf-game`'s `camera.rs` sets it to 60km
/// specifically to give this dome room) since the dome is re-centered on the
/// camera every frame rather than anchored to the world.
const DOME_RADIUS: f32 = 58_000.0;

const SKY_SHADER_HANDLE: Handle<Shader> = weak_handle!("7c9e6f4e-6b8b-4a02-9e6b-3a1f0e9c3d21");

pub struct MfSkyPlugin;

impl Plugin for MfSkyPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, SKY_SHADER_HANDLE, "sky.wgsl", Shader::from_wgsl);
        app.add_plugins(bevy::pbr::MaterialPlugin::<SkyMaterial>::default())
            .add_systems(Startup, spawn_sky_dome_system)
            .add_systems(
                Update,
                (follow_camera_system, update_sky_colors_system)
                    .in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

/// One `Vec4` uniform buffer at binding 0 (this is a standalone `Material`,
/// not an `ExtendedMaterial`, so unlike `RevealExtension` there is no
/// `StandardMaterial` already occupying bindings 0..99 to dodge).
#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct SkyMaterial {
    #[uniform(0)]
    horizon: Vec4,
    #[uniform(0)]
    zenith: Vec4,
    /// (dome_radius, gradient curve power, city_glow_strength, reserved).
    #[uniform(0)]
    params: Vec4,
    /// Soft light-pollution dome color (rgb) + unused w. Mixed into the
    /// horizon band at night; strength lives in `params.z`.
    #[uniform(0)]
    city_glow: Vec4,
}

impl Material for SkyMaterial {
    fn vertex_shader() -> ShaderRef {
        SKY_SHADER_HANDLE.into()
    }

    fn fragment_shader() -> ShaderRef {
        SKY_SHADER_HANDLE.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }

    /// The dome's triangle winding faces outward (default Bevy sphere), but
    /// the camera sits INSIDE it — disable culling so the inner faces (the
    /// ones actually facing the camera) still draw. A ~640-vertex sphere is
    /// cheap enough that drawing both winding directions costs nothing
    /// measurable next to the 2.8M-vertex city it sits behind.
    fn specialize(
        _pipeline: &MaterialPipeline<Self>,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[derive(Component)]
struct SkyDome;

fn spawn_sky_dome_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
) {
    let mesh = meshes.add(Sphere::new(DOME_RADIUS).mesh().ico(3).unwrap());
    let material = materials.add(SkyMaterial {
        horizon: color_to_vec4(palette::sky_day()),
        zenith: color_to_vec4(palette::sky_day()),
        params: Vec4::new(DOME_RADIUS, 1.6, 0.0, 0.0),
        city_glow: Vec4::new(1.0, 0.55, 0.22, 0.0),
    });
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        // The dome must never occlude/be-occluded incorrectly at its own
        // shell radius and must never receive/cast shadows (it isn't part of
        // the game world, just a backdrop).
        NotShadowCaster,
        NotShadowReceiver,
        Name::new("sky-dome"),
        SkyDome,
    ));
}

/// Keeps the dome centered on the camera every frame (module doc: this is
/// what lets a fixed, modest radius always enclose the visible frustum
/// regardless of where in the city the camera currently sits).
fn follow_camera_system(
    cameras: Query<&Transform, (With<Camera3d>, Without<SkyDome>)>,
    mut domes: Query<&mut Transform, With<SkyDome>>,
) {
    let Ok(cam) = cameras.single() else {
        return;
    };
    for mut dome in &mut domes {
        dome.translation = cam.translation;
    }
}

/// Theme- and day/night-aware recolor (spec ask: sky colors must be
/// theme-aware and sourced from `palette.rs`, day/night-aware via
/// `daynight.rs`'s `DayNightState`). Zenith is the palette's flat sky color;
/// horizon is that same color lightened toward white for the "haze" look —
/// real skies scatter more blue out of the thicker atmosphere near the
/// horizon, which reads (at this flat, cel-shaded level of detail) as
/// "horizon is paler than straight up".
fn update_sky_colors_system(
    theme: Res<Theme>,
    day_night: Res<DayNightState>,
    domes: Query<&MeshMaterial3d<SkyMaterial>, With<SkyDome>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
    mut fogs: Query<&mut DistanceFog>,
) {
    if !theme.is_changed() && !day_night.is_changed() {
        return;
    }
    let n = day_night.night_factor;
    let elev = day_night.sun_elevation;
    let golden = {
        let low_sun = (1.0 - (elev / 0.35).clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let not_deep_night = (1.0 - ((n - 0.55).max(0.0) / 0.45)).clamp(0.0, 1.0);
        low_sun * not_deep_night
    };
    let dusk = Color::srgb(1.0, 0.72, 0.48);
    let zenith = palette::sky_day()
        .mix(&dusk, golden * 0.50)
        .mix(&palette::sky_night(), n);
    // Horizon stays slightly paler, but picks up golden warmth at low sun
    // instead of washing toward plain white (which read as gray soup).
    let horizon = zenith
        .mix(&Color::WHITE, 0.22 * (1.0 - golden * 0.7))
        .mix(&dusk, golden * 0.40);
    // Soft sodium/amber city glow — real light-pollution dome. Strength
    // tracks night_factor; color stays warm regardless of theme so the
    // night money-shot reads the same across Light/Dark/Purple.
    let glow_strength = (day_night.night_factor * 0.55).clamp(0.0, 1.0);
    let glow = Color::srgb(1.0, 0.55, 0.22);
    for mat in &domes {
        if let Some(m) = materials.get_mut(&mat.0) {
            m.zenith = color_to_vec4(zenith);
            m.horizon = color_to_vec4(horizon);
            m.params.z = glow_strength;
            m.city_glow = color_to_vec4(glow);
        }
    }
    // Horizon distance fog (nice-to-have per the sky task): tints the far
    // edge of the world toward the horizon haze color so the ground plane
    // doesn't hard-cut against the dome. Cheap (built-in Bevy fog, no extra
    // draw calls) and only recomputed on the same theme/day-night change as
    // the dome colors above.
    for mut fog in &mut fogs {
        fog.color = horizon;
    }
}

fn color_to_vec4(color: Color) -> Vec4 {
    let srgba = color.to_srgba();
    Vec4::new(srgba.red, srgba.green, srgba.blue, srgba.alpha)
}
