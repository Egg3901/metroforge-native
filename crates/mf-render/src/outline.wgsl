// Cel-shading black outline (owner feedback, verbatim: "cel shading/black
// outline for buildings"). Classic inverted-hull technique: this shader
// draws a SECOND copy of the source building mesh, pushed outward a little
// along each vertex's local normal, with only its back faces kept (front
// faces culled -- see outline.rs's `specialize`). The result peeks out only
// at silhouette edges as a thin dark rim; everywhere else the original,
// unpushed building surface (drawn separately, in front) covers it
// entirely.

#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

// One bind-group-2 uniform buffer:
//   color  = outline rgb (unused w)
//   params = (push_distance_m, reserved, reserved, reserved)
struct OutlineUniform {
    color: vec4<f32>,
    params: vec4<f32>,
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
    return out;
}

@fragment
fn fragment() -> @location(0) vec4<f32> {
    return vec4<f32>(outline_uniform.color.rgb, 1.0);
}
