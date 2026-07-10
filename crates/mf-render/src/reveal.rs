//! Building "reveal" dissolve (issue #18, `mf-render` half): a
//! `MaterialExtension` on top of `StandardMaterial` that discards fragments
//! near `mf_state::RevealState.center` in a dithered (ordered/Bayer)
//! pattern, thinning out toward `outer` and fully solid past it.
//! `mf-game`'s `reveal_input.rs` computes *where* the hole should be
//! (cursor ray, close camera); `buildings.rs` copies that shared
//! `RevealState` into this extension's uniform each frame (see its
//! `apply_reveal_system`, quantized the same way `apply_night_dim_system`
//! already is, for the same no-churn reason) and spawns chunk meshes with
//! [`BuildingMaterial`] instead of a bare `StandardMaterial`.
//!
//! ## Why dithered discard, not alpha blending
//! Two hard, already-observed constraints in this exact Bevy 0.16 setup
//! rule out `AlphaMode::Blend`:
//! - vertex colors do not reach the fragment shader on `Blend`
//!   `StandardMaterial`s in this setup (`buildings.rs`'s per-building
//!   tint/brightness-jitter relies on vertex colors reaching an
//!   *opaque*-pipeline shader);
//! - `Blend` renders in the sorted `Transparent3d` phase — real, ongoing
//!   per-frame cost across every building chunk on screen.
//!
//! `AlphaMode::Mask` instead keeps the material in the unsorted,
//! depth-writing `AlphaMask3d` bin — still "opaque" in the sense that
//! matters here (no blend state, full depth write) — while also setting
//! `MeshPipelineKey::MAY_DISCARD`, which is what makes Bevy actually run a
//! fragment shader during the depth prepass *and* the shadow map pass
//! (shadows render through the same `PrepassPipeline`; see
//! `bevy_pbr::render::light::specialize_shadows` in the vendored source).
//! Without that, discarding in the main pass only would leave a "dissolved"
//! building still fully solid in the shadow map. See `reveal_prepass.wgsl`
//! for the matching prepass/shadow shader this unlocks — it mirrors
//! `reveal.wgsl`'s discard test exactly so both passes agree.
//!
//! ## Shader embedding
//! The shipped binary has no `assets/` folder, so both WGSL files are
//! embedded at compile time via `load_internal_asset!` — the same mechanism
//! `bevy_pbr` itself uses for `pbr.wgsl`/`pbr_prepass.wgsl` (see the
//! vendored `bevy_pbr::lib::PbrPlugin::build`) — rather than the
//! asset-server/`embedded_asset!` path.
//!
//! ## Unlit path
//! `apply_quality_to_buildings_material_system` only ever flips
//! `mat.base.unlit`, which both `reveal.wgsl`'s and the vendored `pbr.wgsl`'s
//! non-prepass fragment functions check *after* our discard test already
//! ran (`STANDARD_MATERIAL_FLAGS_UNLIT_BIT` gates the lit-vs-flat-color
//! branch, not whether the fragment shader runs at all) — so the reveal
//! discard applies identically on unlit (potato/low) and lit tiers. This is
//! the "works on potato" property the mission calls for: the effect is a
//! property of the shared extension, orthogonal to the base material's
//! lit/unlit flag.

use bevy::asset::{load_internal_asset, weak_handle};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderRef};

/// Building chunk material family: `StandardMaterial` (unchanged bindings
/// and behavior — night-dim, quality-unlit, per-building vertex tint all
/// keep working exactly as before) extended with the reveal discard.
/// `buildings.rs` spawns every chunk mesh with this type instead of a bare
/// `StandardMaterial`, so there is exactly one building material family.
pub type BuildingMaterial = ExtendedMaterial<StandardMaterial, RevealExtension>;

const REVEAL_SHADER_HANDLE: Handle<Shader> = weak_handle!("2f9d6d9a-2a35-4d0a-9f7d-6a6a2e5d8a10");
const REVEAL_PREPASS_SHADER_HANDLE: Handle<Shader> =
    weak_handle!("6b3f6f4f-3b7b-4a2b-9c7c-6a3a2e5d8a11");

/// Two `Vec4` fields at the SAME binding index (100) — `AsBindGroup`'s
/// derive merges same-binding uniform fields into one generated struct
/// (see vendored `bevy_render_macros::as_bind_group`), so this is still one
/// bind-group-100 buffer, not two. Binding 100 (not 0) leaves 0..99 free for
/// `StandardMaterial`'s own bindings, per the convention the
/// `extended_material` Bevy example itself documents.
#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct RevealExtension {
    /// (center_x, center_z, inner_radius, outer_radius) — world space.
    #[uniform(100)]
    pub reveal: Vec4,
    /// (strength 0..1, reserved, reserved, reserved).
    #[uniform(100)]
    pub params: Vec4,
}

impl Default for RevealExtension {
    /// Non-degenerate `inner`/`outer` (matches `mf_state::RevealState`'s own
    /// default) even though `strength == 0` already makes the effect inert
    /// — `smoothstep(inner, outer, dist)` with `inner == outer == 0` would
    /// be a degenerate (NaN-prone) edge case on some GPUs, and there is no
    /// reason to court that when a harmless non-zero default is free.
    fn default() -> Self {
        RevealExtension {
            reveal: Vec4::new(0.0, 0.0, 60.0, 180.0),
            params: Vec4::ZERO,
        }
    }
}

impl MaterialExtension for RevealExtension {
    fn fragment_shader() -> ShaderRef {
        REVEAL_SHADER_HANDLE.into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        REVEAL_PREPASS_SHADER_HANDLE.into()
    }

    /// See module doc: `Mask` (not `Blend`) is what keeps the dither-discard
    /// depth-write- and shadow-correct while staying out of the sorted
    /// transparent phase. The threshold value itself is never read by
    /// either shader (both call `discard` directly from the world-space
    /// dither test, never testing `base_color.a`) — `Mask(0.5)` exists only
    /// to select the `AlphaMask3d` bin and set
    /// `MeshPipelineKey::MAY_DISCARD`.
    fn alpha_mode() -> Option<AlphaMode> {
        Some(AlphaMode::Mask(0.5))
    }
}

pub struct MfRevealPlugin;

impl Plugin for MfRevealPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, REVEAL_SHADER_HANDLE, "reveal.wgsl", Shader::from_wgsl);
        load_internal_asset!(
            app,
            REVEAL_PREPASS_SHADER_HANDLE,
            "reveal_prepass.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(MaterialPlugin::<BuildingMaterial>::default());
    }
}
