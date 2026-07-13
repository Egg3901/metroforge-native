// Cel-shading black outline (owner feedback, verbatim: "cel shading/black
// outline for buildings"). Classic inverted-hull technique: this shader
// draws a SECOND copy of the source building mesh, pushed outward a little
// along each vertex's local normal, with only its back faces kept (front
// faces culled -- see outline.rs's `specialize`). The result peeks out only
// at silhouette edges as a thin dark rim; everywhere else the original,
// unpushed building surface (drawn separately, in front) covers it
// entirely.
//
// Reveal discard (issue #141): the building material dissolves fragments
// near the cursor with a screen-door bayer test (see facade.wgsl). This
// hull copies the SAME geometry, so it must dissolve identically -- without
// this test, every fragment the buildings discard exposes the solid-black
// hull behind it, and the reveal hole renders as a black mass instead of
// revealing streets (a 472 m supertall inside the hole becomes an entire
// black tower). Same math, same bayer matrix, same uniforms layout as
// facade.wgsl / reveal_prepass.wgsl.

#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

// One bind-group-2 uniform buffer:
//   color  = outline rgb (unused w)
//   params = (push_distance_m, reveal_strength 0..1, reserved, reserved)
//   reveal = (center_x, center_z, inner_radius, outer_radius) world space
struct OutlineUniform {
    color: vec4<f32>,
    params: vec4<f32>,
    reveal: vec4<f32>,
}

@group(2) @binding(0)
var<uniform> outline_uniform: OutlineUniform;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // World XZ of the (unpushed) vertex, for the reveal distance test --
    // matches the world_position the building shaders test, so hull and
    // building fragments dissolve on the same boundary.
    @location(0) world_xz: vec2<f32>,
};

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = get_world_from_local(vertex.instance_index);
    // Push along the LOCAL normal (the source mesh's `Transform` is
    // identity, see outline.rs, so local space == world space here) rather
    // than world normal -- one fewer matrix multiply, and correct as long as
    // that identity-transform assumption holds.
    let pushed = vertex.position + normalize(vertex.normal) * outline_uniform.params.x;
    out.clip_position = mesh_position_local_to_clip(world_from_local, vec4<f32>(pushed, 1.0));
    out.world_xz = vertex.position.xz;
    return out;
}

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

// Duplicated from facade.wgsl for the same reason reveal_prepass.wgsl
// duplicates it: WGSL has no cheap path to #import our own non-bevy_pbr
// module without a fourth embedded shader asset for ~10 lines.
fn reveal_should_discard(world_xz: vec2<f32>, frag_xy: vec2<f32>) -> bool {
    let dist = distance(world_xz, outline_uniform.reveal.xy);
    let t_geom = smoothstep(outline_uniform.reveal.z, outline_uniform.reveal.w, dist);
    let t = mix(1.0, t_geom, outline_uniform.params.y);
    return reveal_bayer_threshold(frag_xy) >= t;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if reveal_should_discard(in.world_xz, in.clip_position.xy) {
        discard;
    }
    return vec4<f32>(outline_uniform.color.rgb, 1.0);
}
