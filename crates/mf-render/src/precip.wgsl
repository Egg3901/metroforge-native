// View-space instanced precipitation (rain streaks / snow flakes), v0.7.
//
// ONE mesh, ONE draw: a big vertex buffer of unit quads (per-vertex `corner`
// + per-particle `seed`); every particle's world position is computed here in
// the vertex shader from `time` + the camera position, so the CPU never
// touches a particle (no per-frame transform upload, no per-pixel CPU loop).
// Particles live in a cylinder around the camera and recycle as they fall, so
// the field follows the camera for free. Count is chosen per quality tier on
// the CPU side (High ~4k / Medium ~1.5k / Low ~400 / Potato 0).
//
// Art direction: rain/snow are near-white/desaturated so they never fight the
// transit colour; wet streets + bloomed stripes carry the mood, not tinted air.

#import bevy_pbr::mesh_view_bindings::view

// One bind-group-2 uniform buffer:
//   p0   = (time_secs, fall_speed, radius_m, vertical_span_m)
//   p1   = (streak_len_m, width_m, sway_freq, kind)   kind: 0 rain, 1 snow
//   wind = (wind_x, wind_z, sway_amp_m, streak_align)  align: 0 billboard-up, 1 along-fall
//   tint = (r, g, b, alpha)
struct PrecipUniform {
    p0: vec4<f32>,
    p1: vec4<f32>,
    wind: vec4<f32>,
    tint: vec4<f32>,
}

@group(2) @binding(0)
var<uniform> u: PrecipUniform;

struct Vertex {
    @location(0) corner: vec2<f32>,
    @location(1) seed: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) corner: vec2<f32>,
};

fn hash11(p: f32) -> f32 {
    var x = fract(p * 0.1031);
    x = x * (x + 33.33);
    x = x * (x + x);
    return fract(x);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;

    let time = u.p0.x;
    let fall_speed = u.p0.y;
    let radius = u.p0.z;
    let span = max(u.p0.w, 1.0);
    let streak = u.p1.x;
    let width = u.p1.y;
    let sway_freq = u.p1.z;
    let kind = u.p1.w;
    let wind = u.wind.xy;
    let sway_amp = u.wind.z;
    let align = u.wind.w;

    let seed = vertex.seed;
    let h1 = hash11(seed + 0.13);
    let h2 = hash11(seed + 5.77);
    let h3 = hash11(seed + 11.31);

    let cam = view.world_position;

    // Vertical recycle: particle cycles down through a band around the camera.
    let fall_total = time * fall_speed + h3 * span;
    let y = cam.y + 0.5 * span - fract(fall_total / span) * span;

    // Horizontal placement in a cylinder around the camera; drift with wind and
    // wrap so it stays local. `- radius .. radius` via fract on a shifted range.
    let bx = (h1 - 0.5) * 2.0 * radius + wind.x * time;
    let bz = (h2 - 0.5) * 2.0 * radius + wind.y * time;
    let wrapx = (fract(bx / (2.0 * radius) + 0.5) - 0.5) * 2.0 * radius;
    let wrapz = (fract(bz / (2.0 * radius) + 0.5) - 0.5) * 2.0 * radius;
    var cx = cam.x + wrapx;
    var cz = cam.z + wrapz;

    // Snow sways sideways as it drifts.
    let sway = sin(time * sway_freq + h1 * 6.2831) * sway_amp * kind;
    cx = cx + sway;

    let center = vec3<f32>(cx, y, cz);

    // Camera basis in world space for billboarding.
    let right = normalize(view.world_from_view[0].xyz);
    let cam_up = normalize(view.world_from_view[1].xyz);
    // Fall direction (down, slanted by wind) — streaks elongate along it.
    let fall_dir = normalize(vec3<f32>(-wind.x * 0.06, -1.0, -wind.y * 0.06));
    let along = normalize(mix(cam_up, -fall_dir, align));

    let world = center
        + right * vertex.corner.x * width
        + along * (vertex.corner.y - 0.5) * mix(width, streak, align);

    out.clip_position = view.clip_from_world * vec4<f32>(world, 1.0);
    out.corner = vertex.corner;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Soft across width; gentle fade along the streak so it doesn't hard-cap.
    let edge = 1.0 - abs(in.corner.x) * 2.0;
    let lengthwise = 1.0 - abs(in.corner.y - 0.5) * 1.6;
    let a = clamp(edge, 0.0, 1.0) * clamp(lengthwise, 0.0, 1.0);
    return vec4<f32>(u.tint.rgb, u.tint.a * a);
}
