// Building "reveal" dissolve — depth-prepass / shadow-pass fragment shader
// (issue #18). `bevy_pbr`'s shadow map rendering reuses `PrepassPipeline<M>`
// (see vendored `bevy_pbr::render::light::specialize_shadows`/
// `queue_shadows`), so WITHOUT this file matching reveal.wgsl's discard
// test, a "dissolved" building would still fully occlude the depth buffer
// and cast a full shadow — visible as a building you can see through still
// blocking the sun. `RevealExtension::alpha_mode()` (`AlphaMode::Mask`) is
// what makes Bevy actually run a fragment shader here at all
// (`MeshPipelineKey::MAY_DISCARD`; see reveal.rs's module doc).
//
// Mirrors the branching structure of the vendored
// `bevy_pbr::render::pbr_prepass.wgsl` one-for-one (down to which ifdefs
// gate which `FragmentOutput` fields) so this keeps compiling if a future
// quality tier turns on normal-prepass/motion-vector-prepass consumers
// (SSAO, TAA) — neither is wired up anywhere in this codebase today, so
// those two branches are unexercised in practice; the discard test itself
// (the actual point of this file) does not depend on either.

#import bevy_pbr::{
    prepass_io::{VertexOutput, FragmentOutput},
    pbr_bindings,
    pbr_types,
    pbr_functions,
    pbr_prepass_functions,
}

// Identical layout to reveal.wgsl's `RevealUniform` — same binding, same
// buffer, both shaders just read it.
struct RevealUniform {
    reveal: vec4<f32>,
    params: vec4<f32>,
}

@group(2) @binding(100)
var<uniform> reveal_uniform: RevealUniform;

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

// Deliberately duplicated from reveal.wgsl — see that file's matching
// comment for why (no cross-module `#import` path for our own shader code
// without a third embedded asset).
fn reveal_should_discard(world_xz: vec2<f32>, frag_xy: vec2<f32>) -> bool {
    let dist = distance(world_xz, reveal_uniform.reveal.xy);
    let t_geom = smoothstep(reveal_uniform.reveal.z, reveal_uniform.reveal.w, dist);
    let t = mix(1.0, t_geom, reveal_uniform.params.x);
    return reveal_bayer_threshold(frag_xy) >= t;
}

#ifdef PREPASS_FRAGMENT
@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    if reveal_should_discard(in.world_position.xz, in.position.xy) {
        discard;
    }

    var out: FragmentOutput;

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif // UNCLIPPED_DEPTH_ORTHO_EMULATION

#ifdef NORMAL_PREPASS
    // Buildings never have a normal map (see buildings.rs: flat
    // `StandardMaterial` color fields only), so this skips straight to the
    // no-normal-map branch of the vendored pbr_prepass.wgsl instead of
    // reimplementing TBN/normal-map sampling that would never fire here.
    let flags = pbr_bindings::material.flags;
    let double_sided = (flags & pbr_types::STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;
    let world_normal = pbr_functions::prepare_world_normal(in.world_normal, double_sided, is_front);
    out.normal = vec4(world_normal * 0.5 + vec3(0.5), 1.0);
#endif // NORMAL_PREPASS

#ifdef MOTION_VECTOR_PREPASS
    out.motion_vector = pbr_prepass_functions::calculate_motion_vector(in.world_position, in.previous_world_position);
#endif // MOTION_VECTOR_PREPASS

    return out;
}
#else
@fragment
fn fragment(in: VertexOutput) {
    if reveal_should_discard(in.world_position.xz, in.position.xy) {
        discard;
    }
}
#endif // PREPASS_FRAGMENT
