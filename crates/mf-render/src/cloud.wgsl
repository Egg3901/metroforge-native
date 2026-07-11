// Soft cloud-card fragment: density texture drives alpha; uniform carries
// day / golden-hour / night tint. Never fully opaque — city must stay readable.

#import bevy_pbr::forward_io::VertexOutput

struct CloudUniform {
    color: vec4<f32>,
}

@group(2) @binding(0)
var<uniform> cloud: CloudUniform;

@group(2) @binding(1)
var density_texture: texture_2d<f32>;

@group(2) @binding(2)
var density_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let d = textureSample(density_texture, density_sampler, uv).r;
    let a = d * cloud.color.a;
    // Premultiply-ish soft edge: rgb stays tinted, alpha from density.
    return vec4(cloud.color.rgb, a);
}
