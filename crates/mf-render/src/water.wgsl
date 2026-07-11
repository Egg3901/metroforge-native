// Stylized water (Mirror's Edge-clean, NOT photoreal). Procedural ripples,
// sun glint, shoreline foam, sky fresnel, and a faint night shimmer — all
// ALU-only, no textures (art-direction: "NO texture"). Quality is gated in
// the uniform `params.z` (1 = single static layer, 2 = full dual-layer
// animated). Potato never draws this mesh (flat vertex-color water stays
// baked into the terrain mesh).

#import bevy_pbr::{
    mesh_functions::{get_world_from_local, mesh_position_local_to_clip},
    mesh_view_bindings::view,
}

// One bind-group-2 uniform buffer (AsBindGroup merges same-binding Vec4s):
//   water_color = base water rgb (palette::water), unused w
//   sky_color   = horizon haze rgb for fresnel tint, unused w
//   foam_color  = light shoreline foam rgb, unused w
//   sun         = (sun_direction.xyz, sun_elevation)
//   params      = (time_secs, night_factor, quality 1|2, subway_dim 0..1)
//   shimmer     = (night emissive rgb, shimmer strength)
struct WaterUniform {
    water_color: vec4<f32>,
    sky_color: vec4<f32>,
    foam_color: vec4<f32>,
    sun: vec4<f32>,
    params: vec4<f32>,
    shimmer: vec4<f32>,
}

@group(2) @binding(0)
var<uniform> water_uniform: WaterUniform;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // x = water_frac (1 = open water, 0 = land). Foam / city-side shimmer
    // derive from this; y unused.
    @location(2) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) water_frac: f32,
};

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = get_world_from_local(vertex.instance_index);
    let world = world_from_local * vec4<f32>(vertex.position, 1.0);
    out.clip_position = mesh_position_local_to_clip(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    out.world_position = world.xyz;
    out.water_frac = vertex.uv.x;
    return out;
}

// Cheap 2D value-noise hash — enough for stylized sparkle, not a texture.
fn hash21(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453);
}

// One scrolling ripple "normal" layer. Amplitude is deliberately small so
// the surface stays Mirror's-Edge flat rather than choppy ocean.
fn ripple_normal(xz: vec2<f32>, time: f32, scale: f32, speed: vec2<f32>, amp: f32) -> vec3<f32> {
    let uv = xz * scale + speed * time;
    let dx = cos(uv.x) * sin(uv.y * 1.3);
    let dz = sin(uv.x * 0.9) * cos(uv.y);
    return normalize(vec3<f32>(-dx * amp, 1.0, -dz * amp));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let time = water_uniform.params.x;
    let night = clamp(water_uniform.params.y, 0.0, 1.0);
    let quality = water_uniform.params.z;
    let dim = clamp(water_uniform.params.w, 0.0, 1.0);
    let water_frac = clamp(in.water_frac, 0.0, 1.0);

    // Low: one static layer (time frozen). Medium/High: two scrolling layers.
    let t = select(0.0, time, quality >= 2.0);
    var n = ripple_normal(in.world_position.xz, t, 0.045, vec2<f32>(0.07, 0.04), 0.12);
    if quality >= 2.0 {
        let n2 = ripple_normal(in.world_position.xz, t, 0.09, vec2<f32>(-0.05, 0.08), 0.08);
        n = normalize(n + n2 * 0.65);
    }

    let view_dir = normalize(view.world_position.xyz - in.world_position);
    let sun_dir = normalize(water_uniform.sun.xyz);
    let sun_elev = clamp(water_uniform.sun.w, 0.0, 1.0);

    // Soft fresnel toward the horizon sky tint — kills the "blue cardboard"
    // look without going reflective/photoreal.
    let ndotv = clamp(dot(n, view_dir), 0.0, 1.0);
    let fresnel = pow(1.0 - ndotv, 2.4) * 0.55;
    var color = mix(water_uniform.water_color.rgb, water_uniform.sky_color.rgb, fresnel);

    // Specular glint from the sun direction. Dims at night / low sun.
    let half_v = normalize(sun_dir + view_dir);
    let spec = pow(clamp(dot(n, half_v), 0.0, 1.0), 64.0);
    let day_spec = (1.0 - night) * sun_elev;
    color = color + vec3<f32>(1.0, 0.97, 0.92) * (spec * 0.55 * day_spec);

    // Shoreline foam band from water_frac (3x3-softened mask baked into UV).
    // Peaks where water meets land so coastlines read crisply at altitude.
    let foam = smoothstep(0.15, 0.45, water_frac) * (1.0 - smoothstep(0.55, 0.95, water_frac));
    color = mix(color, water_uniform.foam_color.rgb, foam * 0.7);

    // Night: city-side water picks up a faint emissive shimmer (stronger
    // near shore / harbors where water_frac is mid-range, weaker in open
    // water). Ripple specular is already dimmed via day_spec above.
    let city_side = smoothstep(0.2, 0.7, 1.0 - water_frac * 0.5);
    let sparkle = hash21(floor(in.world_position.xz * 0.35));
    let shimmer = water_uniform.shimmer.rgb
        * water_uniform.shimmer.w
        * night
        * city_side
        * smoothstep(0.72, 0.95, sparkle);
    color = color + shimmer;

    // Subway-view ground dim (same role as TerrainSurface base_color dim).
    color = color * dim;

    return vec4<f32>(color, 1.0);
}
