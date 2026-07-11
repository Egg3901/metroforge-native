// Procedural building facade detail (main opaque pass). Replaces the former
// reveal-only fragment entry: keeps the dithered reveal discard, then tints
// wall fragments with a world-position window grid. Zero texture assets —
// floors, columns, and night lit/unlit pattern are all hash-parametric.
//
// LOD (camera distance to fragment, no popping — smoothstep fades):
//   < ~800m   full windows (day insets / night glow)
//   < ~2.5km  grid-only (floor lines + column hints)
//   beyond    flat cel shading (identity)
// Tier gate: `facade.y` (enabled) is 0 on Potato/Low; Medium/High set 1.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    mesh_view_bindings::view,
}

// Packed into ONE bind-group-100 uniform buffer (AsBindGroup merges same-
// binding Vec4 fields — see RevealExtension in reveal.rs):
//   reveal = (center_x, center_z, inner_radius, outer_radius) world space
//   params = (reveal_strength 0..1, reserved, reserved, reserved)
//   facade = (night_factor 0..1, enabled 0/1, reserved, reserved)
struct RevealUniform {
    reveal: vec4<f32>,
    params: vec4<f32>,
    facade: vec4<f32>,
}

@group(2) @binding(100)
var<uniform> reveal_uniform: RevealUniform;

const BAYER_4X4: array<f32, 16> = array(
    0.0 / 16.0, 8.0 / 16.0, 2.0 / 16.0, 10.0 / 16.0,
    12.0 / 16.0, 4.0 / 16.0, 14.0 / 16.0, 6.0 / 16.0,
    3.0 / 16.0, 11.0 / 16.0, 1.0 / 16.0, 9.0 / 16.0,
    15.0 / 16.0, 7.0 / 16.0, 13.0 / 16.0, 5.0 / 16.0,
);

const LOD_FULL_M: f32 = 800.0;
const LOD_GRID_M: f32 = 2500.0;
const LOD_FULL_FADE_M: f32 = 180.0;
const LOD_GRID_FADE_M: f32 = 350.0;

// Warm night window glow — transit stays vivid; buildings get a soft amber.
const WINDOW_LIT: vec3<f32> = vec3(1.0, 0.82, 0.55);
// Daytime inset: subtle darker rectangle on the white massing.
const WINDOW_DAY_DARKEN: f32 = 0.11;

fn reveal_bayer_threshold(frag_xy: vec2<f32>) -> f32 {
    let ix = u32(frag_xy.x) % 4u;
    let iy = u32(frag_xy.y) % 4u;
    return BAYER_4X4[iy * 4u + ix];
}

fn reveal_should_discard(world_xz: vec2<f32>, frag_xy: vec2<f32>) -> bool {
    let dist = distance(world_xz, reveal_uniform.reveal.xy);
    let t_geom = smoothstep(reveal_uniform.reveal.z, reveal_uniform.reveal.w, dist);
    let t = mix(1.0, t_geom, reveal_uniform.params.x);
    return reveal_bayer_threshold(frag_xy) >= t;
}

fn hash11(n: f32) -> f32 {
    return fract(sin(n * 127.1) * 43758.5453);
}

fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

/// Stable per-building seed: step inward from the facade, quantize to a
/// coarse cell so every wall of the same mass shares one hash without a
/// per-vertex building-id attribute (merged chunk meshes).
fn building_seed(pos: vec3<f32>, normal: vec3<f32>) -> f32 {
    var n_xz = normal.xz;
    let nlen = length(n_xz);
    if nlen < 1e-4 {
        n_xz = vec2(1.0, 0.0);
    } else {
        n_xz = n_xz / nlen;
    }
    let inward = pos.xz - n_xz * 3.0;
    return hash21(floor(inward * 0.25));
}

/// Apply procedural facade tint. `grid_w` / `full_w` are 0..1 LOD weights.
fn apply_facade(
    base_rgb: vec3<f32>,
    pos: vec3<f32>,
    normal: vec3<f32>,
    night: f32,
    grid_w: f32,
    full_w: f32,
) -> vec3<f32> {
    // Roofs / near-horizontal faces stay flat (parapet/AC are mesh detail).
    if abs(normal.y) > 0.55 {
        return base_rgb;
    }

    let seed = building_seed(pos, normal);
    // Slight per-building variation so the skyline isn't a uniform grid.
    let floor_h = mix(3.25, 3.75, hash11(seed * 17.3));
    let col_w = mix(2.6, 4.0, hash11(seed * 29.1 + 3.7));
    let win_inset_h = mix(0.18, 0.28, hash11(seed * 41.0 + 1.1));
    let win_inset_v = mix(0.14, 0.22, hash11(seed * 53.0 + 2.3));

    var n_xz = normal.xz;
    let nlen = length(n_xz);
    if nlen < 1e-4 {
        return base_rgb;
    }
    n_xz = n_xz / nlen;
    let tangent = vec2(-n_xz.y, n_xz.x);
    let along = dot(pos.xz, tangent);

    let fy = fract(pos.y / floor_h);
    let fx = fract(along / col_w);

    // Floor lines + column gutters (grid-only LOD band).
    let floor_line = step(fy, 0.045) + step(0.955, fy);
    let col_line = step(fx, 0.06) + step(0.94, fx);
    let grid_mask = max(floor_line, col_line * 0.65);
    let grid_rgb = base_rgb * (1.0 - 0.07 * grid_mask * grid_w);

    // Window pane (full LOD): inset rectangle inside each cell.
    let in_win = step(win_inset_h, fx) * step(fx, 1.0 - win_inset_h)
        * step(win_inset_v, fy) * step(fy, 1.0 - win_inset_v * 1.15);

    let cell_id = vec2(floor(along / col_w), floor(pos.y / floor_h));
    // Deterministic ~30% lit at night.
    let lit = step(0.70, hash21(cell_id + vec2(seed * 64.0, seed * 13.0)));

    let day_win = base_rgb * (1.0 - WINDOW_DAY_DARKEN);
    let night_win = mix(base_rgb * 0.55, WINDOW_LIT, lit);
    let win_rgb = mix(day_win, night_win, night);

    var out_rgb = mix(grid_rgb, mix(grid_rgb, win_rgb, in_win), full_w);
    // When full fades out, keep grid contribution from grid_w.
    out_rgb = mix(base_rgb, out_rgb, max(grid_w, full_w));
    return out_rgb;
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    if reveal_should_discard(in.world_position.xz, in.position.xy) {
        discard;
    }

    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let facade_enabled = reveal_uniform.facade.y;
    if facade_enabled > 0.5 {
        let cam = view.world_position;
        let dist = distance(in.world_position.xyz, cam);
        let full_w = 1.0 - smoothstep(
            LOD_FULL_M - LOD_FULL_FADE_M,
            LOD_FULL_M + LOD_FULL_FADE_M,
            dist,
        );
        let grid_w = 1.0 - smoothstep(
            LOD_GRID_M - LOD_GRID_FADE_M,
            LOD_GRID_M + LOD_GRID_FADE_M,
            dist,
        );
        let night = reveal_uniform.facade.x;
        let shaded = apply_facade(
            pbr_input.material.base_color.rgb,
            in.world_position.xyz,
            in.world_normal,
            night,
            grid_w,
            full_w,
        );
        pbr_input.material.base_color = vec4(shaded, pbr_input.material.base_color.a);
    }

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
