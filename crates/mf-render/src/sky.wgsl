// Gradient sky dome (owner feedback: "no skybox" -> add one, but a flat
// procedural gradient, not a photo cubemap). A large inverted-normal sphere
// follows the camera (see sky.rs's `follow_camera_system`) so its surface is
// always the farthest thing in view; this fragment shader paints it as a
// simple two-stop vertical gradient (horizon haze -> zenith) rather than
// sampling any texture, matching the flat cel-shaded art direction instead
// of physically-based scattering (see sky.rs module doc for the
// Atmosphere-vs-dome tradeoff this was picked over).

#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

// One bind-group-2 uniform buffer (three `Vec4` fields at the same
// `#[uniform(0)]` binding get merged by `AsBindGroup`, same pattern as
// `RevealExtension` in reveal.rs):
//   horizon = sky color near the horizon (rgb, unused w)
//   zenith  = sky color straight up (rgb, unused w)
//   params  = (dome_radius, gradient curve power, reserved, reserved)
struct SkyUniform {
    horizon: vec4<f32>,
    zenith: vec4<f32>,
    params: vec4<f32>,
}

@group(2) @binding(0)
var<uniform> sky_uniform: SkyUniform;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local_position: vec3<f32>,
};

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = get_world_from_local(vertex.instance_index);
    out.clip_position = mesh_position_local_to_clip(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    // Gradient factor is computed from the dome's LOCAL position, not world
    // position: the dome is centered on the camera every frame (see
    // `follow_camera_system`), so local Y is already "height above/below the
    // camera's eye line" -- exactly the horizon-to-zenith axis we want,
    // independent of where in the city the camera currently sits.
    out.local_position = vertex.position;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let radius = max(sky_uniform.params.x, 1.0);
    let t = clamp(in.local_position.y / radius, 0.0, 1.0);
    let curved = pow(t, max(sky_uniform.params.y, 0.01));
    let color = mix(sky_uniform.horizon.rgb, sky_uniform.zenith.rgb, curved);
    return vec4<f32>(color, 1.0);
}
