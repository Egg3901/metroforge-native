// Terrain main-pass fragment: stock PBR plus scrolling cloud-shadow multiply
// from atmosphere's 2D noise (world-XZ projection). Strength 0 → identity.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

struct TerrainUniform {
    cloud: vec4<f32>,
}

struct WeatherUniform {
    // x = snow_depth (0..1); y,z,w reserved.
    weather: vec4<f32>,
}

@group(2) @binding(100)
var<uniform> terrain_uniform: TerrainUniform;

@group(2) @binding(101)
var cloud_noise_texture: texture_2d<f32>;

@group(2) @binding(102)
var cloud_noise_sampler: sampler;

@group(2) @binding(103)
var<uniform> weather_uniform: WeatherUniform;

fn cloud_shadow_factor(world_xz: vec2<f32>) -> f32 {
    let strength = terrain_uniform.cloud.z;
    if strength < 0.001 {
        return 1.0;
    }
    let uv = world_xz * terrain_uniform.cloud.w + terrain_uniform.cloud.xy;
    let n = textureSample(cloud_noise_texture, cloud_noise_sampler, uv).r;
    return 1.0 - n * strength;
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    let shadow = cloud_shadow_factor(in.world_position.xz);
    var rgb = out.color.rgb * shadow;
    // Snow accumulation: lerp toward a near-white settled-snow tone. Applied
    // after lighting so the whitened ground still takes the sky/ambient grade
    // (overcast snow reads cool, not a flat paper cutout). Slightly off-white
    // so it separates from the buildings' pure-white cel albedo.
    let snow = clamp(weather_uniform.weather.x, 0.0, 1.0);
    rgb = mix(rgb, vec3<f32>(0.94, 0.95, 0.97), snow * 0.85);
    out.color = vec4(rgb, out.color.a);
    return out;
}
