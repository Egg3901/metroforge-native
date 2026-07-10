// Building "reveal" dissolve — main opaque-pass fragment shader (issue #18).
// Dissolves buildings around `RevealState.center` (cursor / close camera,
// see mf-game's reveal_input.rs) via DITHERED DISCARD, not alpha blending:
// see reveal.rs's module doc for why (vertex colors don't reach Blend
// StandardMaterials in this Bevy 0.16 setup, and Blend costs transparent-
// pass sorting). This file replaces `bevy_pbr::render::pbr.wgsl`'s
// non-prepass fragment entry point one-for-one (same imports/structure,
// just an early discard test spliced in before lighting) — see
// `reveal_prepass.wgsl` for the depth-prepass/shadow-pass counterpart that
// keeps those passes agreeing with this one.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

// Packed into ONE bind-group-100 uniform buffer (two `Vec4` fields at the
// same Rust-side `#[uniform(100)]` binding get merged by `AsBindGroup` into
// this exact layout — see `RevealExtension` in reveal.rs):
//   reveal = (center_x, center_z, inner_radius, outer_radius) world space
//   params = (strength 0..1, reserved, reserved, reserved)
struct RevealUniform {
    reveal: vec4<f32>,
    params: vec4<f32>,
}

@group(2) @binding(100)
var<uniform> reveal_uniform: RevealUniform;

// Classic 4x4 ordered (Bayer) dither matrix, normalized to steps of 1/16.
// Indexed by the fragment's screen pixel coordinate mod 4 — a fixed,
// per-pixel pattern (not noise, not animated), which is what reads as an
// even "screen door" mesh instead of a swimming or banded gradient.
const BAYER_4X4: array<f32, 16> = array(
    0.0 / 16.0, 8.0 / 16.0, 2.0 / 16.0, 10.0 / 16.0,
    12.0 / 16.0, 4.0 / 16.0, 14.0 / 16.0, 6.0 / 16.0,
    3.0 / 16.0, 11.0 / 16.0, 1.0 / 16.0, 9.0 / 16.0,
    15.0 / 16.0, 7.0 / 16.0, 13.0 / 16.0, 5.0 / 16.0,
);

fn reveal_bayer_threshold(frag_xy: vec2<f32>) -> f32 {
    let ix = u32(frag_xy.x) % 4u;
    let iy = u32(frag_xy.y) % 4u;
    return BAYER_4X4[iy * 4u + ix];
}

// Shared (by intent, duplicated) with `reveal_prepass.wgsl`: WGSL has no
// path to `#import` our own non-`bevy_pbr` module without registering a
// third embedded shader asset purely to hold ~10 lines, which is more
// ceremony than the duplication it would save.
fn reveal_should_discard(world_xz: vec2<f32>, frag_xy: vec2<f32>) -> bool {
    let dist = distance(world_xz, reveal_uniform.reveal.xy);
    let t_geom = smoothstep(reveal_uniform.reveal.z, reveal_uniform.reveal.w, dist);
    // strength == 0 forces t to 1.0 (never discard — the effect is fully
    // off); strength == 1 uses the raw geometric falloff. Values in between
    // ease the hole in/out without moving inner/outer themselves.
    let t = mix(1.0, t_geom, reveal_uniform.params.x);
    return reveal_bayer_threshold(frag_xy) >= t;
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    // Discard before doing any lighting work — cheaper, and this is the
    // whole point of the effect (a dissolved building costs nothing beyond
    // this test plus the depth-prepass/shadow-pass equivalent in
    // reveal_prepass.wgsl).
    if reveal_should_discard(in.world_position.xz, in.position.xy) {
        discard;
    }

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        // Unlit tiers (quality::apply_quality_to_buildings_material_system)
        // still go through this same discard test above — the reveal
        // effect is independent of the lit/unlit knob, which is exactly the
        // "works on potato" requirement.
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
