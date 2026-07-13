// Gradient sky dome (owner feedback: "no skybox" -> add one, but a flat
// procedural gradient, not a photo cubemap). A large inverted-normal sphere
// follows the camera (see sky.rs's `follow_camera_system`) so its surface is
// always the farthest thing in view; this fragment shader paints it as a
// simple two-stop vertical gradient (horizon haze -> zenith) rather than
// sampling any texture, matching the flat cel-shaded art direction instead
// of physically-based scattering (see sky.rs module doc for the
// Atmosphere-vs-dome tradeoff this was picked over).
//
// At night, a soft city-glow (light-pollution) band is mixed into the
// horizon via `params.z` / `city_glow` — additive to the gradient, not a
// rewrite of it.

#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

// One bind-group-2 uniform buffer (four `Vec4` fields at the same
// `#[uniform(0)]` binding get merged by `AsBindGroup`, same pattern as
// `RevealExtension` in reveal.rs):
//   horizon   = sky color near the horizon (rgb, unused w)
//   zenith    = sky color straight up (rgb, unused w)
//   params    = (dome_radius, gradient curve power, city_glow_strength, reserved)
//   city_glow = warm light-pollution color (rgb, unused w)
struct SkyUniform {
    horizon: vec4<f32>,
    zenith: vec4<f32>,
    params: vec4<f32>,
    city_glow: vec4<f32>,
    // Below-horizon backdrop stop (rgb): the diorama slab floats in a void,
    // so the region under the eye line must be a smooth continuation of the
    // gradient (soft studio backdrop), not a flat clamp band.
    below: vec4<f32>,
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
    let t_signed = clamp(in.local_position.y / radius, -1.0, 1.0);
    // Above the eye line: horizon -> zenith (unchanged). Below it: the same
    // horizon color eases smoothly into the `below` backdrop stop, so the
    // void under the floating slab reads as a soft studio backdrop instead
    // of the old hard clamp band (t was clamped at 0 below the horizon,
    // painting the entire lower hemisphere one flat color).
    var color: vec3<f32>;
    var curved: f32;
    if t_signed >= 0.0 {
        curved = pow(t_signed, max(sky_uniform.params.y, 0.01));
        color = mix(sky_uniform.horizon.rgb, sky_uniform.zenith.rgb, curved);
    } else {
        curved = 0.0;
        let down = pow(-t_signed, 0.65);
        color = mix(sky_uniform.horizon.rgb, sky_uniform.below.rgb, down);
    }

    // Soft city-glow dome: strongest at the horizon (low curved), fades
    // toward zenith. `params.z` is night_factor-scaled strength from Rust.
    let glow_strength = sky_uniform.params.z;
    if glow_strength > 0.001 {
        // Confined to a band around the eye line in BOTH directions: the old
        // `1.0 - curved` weight was 1.0 across the entire below-horizon
        // hemisphere, which painted the diorama void warm beige at night
        // instead of a subtle horizon lift.
        let horizon_weight = pow(max(1.0 - abs(t_signed) * 6.0, 0.0), 2.0);
        color = mix(color, sky_uniform.city_glow.rgb, horizon_weight * glow_strength);
    }

    return vec4<f32>(color, 1.0);
}
