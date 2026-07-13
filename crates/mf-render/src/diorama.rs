//! Diorama slab (v0.8 Underground — Captain Toad art-direction amendment,
//! BINDING: `metroforge-native-art-direction` "Reference 2"). The world square
//! reads as a floating tabletop MODEL: its perimeter is a set of vertical cut
//! sides dropping below the terrain edge (stylized display depth, see
//! `SLAB_DEPTH_M`), and those cut sides ARE the
//! geology display — clean banded flat-color strata (fill/soil, clay/sand,
//! rock, bedrock) with a thin blue water-table line, plus a slight top-edge
//! bevel so the "sliced cake" feel reads at every zoom. A soft contact shadow
//! quad sits under the slab so it floats over the gradient backdrop (the sky
//! dome in `sky.rs`, which is already the "no photo skybox" day/night gradient
//! this amendment calls for).
//!
//! ## Cheap by construction (art direction: "potato-safe")
//! The walls are ONE static mesh with banded vertex colors, rebuilt only when
//! the city changes — the same rebuild-signature pattern as `terrain.rs`, and
//! keyed on the elevation channel's resolution exactly like the buried-roads
//! fix (`terrain.rs` `elev_res`), because the msgType=7 elevation frame lands
//! AFTER `fields` and the slab edge heights (sampled through `HeightAt`) must
//! rebuild once the real DEM has replaced the flat placeholder. The slab runs
//! in `MfRenderSet::Statics`, after `Terrain`, so the `HeightAt` sampler it
//! reads is this frame's freshly-built one.
//!
//! ## Strata source (design choice)
//! The band depths come from `geology.rs`, a client-side MIRROR of the sim's
//! pure O(1) strata function, reconstructed from the seed + city key the
//! client already holds (`GeologyContext`) and the elevation channel — NOT
//! from `strataProbe` round-trips (which would be 64×4 = 256 requests per city
//! load just for the edge). See `geology.rs` for the full rationale.
//!
//! ## Tier gating
//! Slab walls + bevel + baked water-table line render on ALL tiers (one static
//! unlit vertex-colored mesh — cheap). The ANIMATED water-table shimmer and
//! the soft contact shadow are Medium+ only (they add a per-frame system and a
//! translucent draw); lower tiers still show the water line, just not moving.

use bevy::prelude::*;

use mf_state::{CurrentCity, EffectiveKnobs, GeologyContext, HeightAt, LatestFields, Theme};

use crate::geology::{strata_column, StrataColumn};
use crate::mesh_utils::MeshBuffers;
use crate::palette;

/// How far (m) the perimeter cut sides drop below the y=0 ground/water plane.
///
/// STYLIZED, not true meters (owner direction, 2026-07-13): the true ~80 m
/// geology column is an invisible sliver at map zoom on a 12 km world square.
/// The art direction's true-meter rule applies to the CITY; the slab side is
/// presentation — the Captain Toad chunky-diorama read needs the cut sides to
/// occupy a visible fraction of the model's silhouette. Band DEPTHS from the
/// geology model are rescaled proportionally into this display depth (see
/// `DISPLAY_OVERBURDEN_FRAC` below), so fill/clay/rock/bedrock and the water
/// table all keep their relative positions, just chunkier.
pub const SLAB_DEPTH_M: f32 = 520.0;
/// Fraction of the display depth the overburden + rock column (surface to
/// bedrock top) is scaled to fill; the remaining bottom fraction is bedrock.
/// Keeps all four bands individually legible at overview zoom.
const DISPLAY_OVERBURDEN_FRAC: f32 = 0.72;
/// World Y of the flat slab underside (the cut sides bottom out here, and the
/// bedrock band is clamped to it).
const SLAB_BOTTOM_Y: f32 = -SLAB_DEPTH_M;
/// Samples per side for the banded cut face. 64/side (the art-direction figure)
/// resolves the terrain edge relief and the noise-driven band wander smoothly
/// while staying a trivial one-time mesh (≈ 64×4 columns × 4 bands).
const SAMPLES_PER_SIDE: usize = 64;
/// Top-edge bevel size (m): the cut sides start this far outward + down from
/// the terrain edge, and a lighter chamfer lip catches the light there so the
/// tabletop-model "sliced cake" edge reads at every zoom.
const BEVEL_M: f32 = 12.0;
/// Half-thickness (m) of the water-table line drawn across the strata. Sized
/// against the stylized display depth (not true meters) so the line reads at
/// overview zoom without turning into a band of its own.
const WATER_BAND_HALF_M: f32 = 4.5;

/// Marker on the static slab-wall entity.
#[derive(Component)]
struct DioramaSlab;

/// Marker on the animated water-table overlay strip (Medium+).
#[derive(Component)]
struct DioramaWaterBand;

/// Marker on the soft contact-shadow quad (Medium+).
#[derive(Component)]
struct DioramaContactShadow;

pub struct MfDioramaPlugin;

impl Plugin for MfDioramaPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DioramaState>()
            .add_systems(
                Update,
                build_diorama_system.in_set(crate::MfRenderSet::Statics),
            )
            .add_systems(
                Update,
                animate_water_band_system.in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

#[derive(Resource, Default)]
struct DioramaState {
    /// `(fields.version, elev_res, theme, seed, city_key_hash, medium_plus)` —
    /// the rebuild signature. `elev_res` keys on the elevation channel exactly
    /// like `terrain.rs`, so the slab rebuilds when the DEM arrives after
    /// `fields` (and the edge heights from `HeightAt` become real);
    /// `seed`/`city_key_hash` capture the geology inputs; `medium_plus` flips
    /// the animated-water / contact-shadow gate.
    key: Option<(u32, u32, Theme, u64, u64, bool)>,
    slab: Option<Entity>,
    water_band: Option<Entity>,
    contact_shadow: Option<Entity>,
    water_material: Option<Handle<StandardMaterial>>,
}

fn city_key_hash(key: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

#[allow(clippy::too_many_arguments)]
fn build_diorama_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<LatestFields>,
    geology: Res<GeologyContext>,
    effective: Res<EffectiveKnobs>,
    theme: Res<Theme>,
    height_at: Res<HeightAt>,
    mut state: ResMut<DioramaState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    let Some(f) = &fields.0 else {
        return;
    };
    let elev_res = city.elevation.as_ref().map(|e| e.res).unwrap_or(0);
    // Medium+ = lit materials tier (unlit_material is false only on Medium/High).
    let medium_plus = !effective.0.unlit_material;
    let key = (
        f.version,
        elev_res,
        *theme,
        geology.seed,
        city_key_hash(geology.city_key.as_deref()),
        medium_plus,
    );
    if state.key == Some(key) {
        return;
    }
    state.key = Some(key);

    for e in [
        state.slab.take(),
        state.water_band.take(),
        state.contact_shadow.take(),
    ]
    .into_iter()
    .flatten()
    {
        commands.entity(e).despawn();
    }

    let world_size = city_json.world_size as f32;
    let half = world_size / 2.0;
    if half < 1.0 {
        return;
    }

    let heights = city.elevation.as_ref().map(|e| e.heights.as_slice());
    let sample = |x: f32, z: f32| -> (f32, StrataColumn) {
        let surf_y = (height_at.0)(x, z);
        let col = strata_column(
            geology.seed,
            geology.city_key.as_deref(),
            heights,
            elev_res,
            world_size as f64,
            x as f64,
            z as f64,
        );
        (surf_y, col)
    };

    // Four perimeter edges (outward normal, and a parametric position along
    // the edge as `t` in 0..1). All go corner-to-corner so adjacent sides
    // meet at a shared column.
    type EdgeFn = Box<dyn Fn(f32) -> Vec2>;
    let edges: [(Vec3, EdgeFn); 4] = [
        // West (x = -half), normal -X, sweep z.
        (
            Vec3::NEG_X,
            Box::new(move |t: f32| Vec2::new(-half, -half + t * world_size)),
        ),
        // East (x = +half), normal +X.
        (
            Vec3::X,
            Box::new(move |t: f32| Vec2::new(half, -half + t * world_size)),
        ),
        // North (z = -half), normal -Z, sweep x.
        (
            Vec3::NEG_Z,
            Box::new(move |t: f32| Vec2::new(-half + t * world_size, -half)),
        ),
        // South (z = +half), normal +Z.
        (
            Vec3::Z,
            Box::new(move |t: f32| Vec2::new(-half + t * world_size, half)),
        ),
    ];

    let mut wall = MeshBuffers::new();
    let mut water = MeshBuffers::new();
    let bevel_color = palette::strata_fill().mix(&Color::WHITE, 0.35);

    for (normal, edge) in &edges {
        let n2 = Vec2::new(normal.x, normal.z);
        // Precompute the per-sample columns for this edge.
        let cols: Vec<(Vec2, f32, StrataColumn)> = (0..=SAMPLES_PER_SIDE)
            .map(|i| {
                let t = i as f32 / SAMPLES_PER_SIDE as f32;
                let p = edge(t);
                let (surf_y, col) = sample(p.x, p.y);
                (p, surf_y, col)
            })
            .collect();

        for w in cols.windows(2) {
            let (pa, surf_a, col_a) = &w[0];
            let (pb, surf_b, col_b) = &w[1];
            // Wall-top points: pushed outward + down by the bevel so a lighter
            // chamfer lip sits above the vertical banded face.
            let wa_top = *surf_a - BEVEL_M;
            let wb_top = *surf_b - BEVEL_M;
            let a_out = *pa + n2 * BEVEL_M;
            let b_out = *pb + n2 * BEVEL_M;

            // Bevel chamfer: terrain edge (surf) -> wall top (out+down).
            let g_a = Vec3::new(pa.x, *surf_a, pa.y);
            let g_b = Vec3::new(pb.x, *surf_b, pb.y);
            let t_a = Vec3::new(a_out.x, wa_top, a_out.y);
            let t_b = Vec3::new(b_out.x, wb_top, b_out.y);
            wall.push_flat_quad(g_a, g_b, t_b, t_a, *normal, bevel_color);

            // Banded vertical cut face below the wall top. Each band is one
            // quad. Band depths from the geology model (true metres) are
            // rescaled into the STYLIZED display depth: the overburden+rock
            // column (surface -> bedrock top) fills `DISPLAY_OVERBURDEN_FRAC`
            // of each column's wall height, bedrock the remainder — every
            // band keeps its relative thickness, just chunky enough to read
            // at overview zoom (see `SLAB_DEPTH_M`'s doc).
            let scale_for = |wall_top: f32, col: &StrataColumn| -> f32 {
                let bedrock_top = col.bands[3].top.max(1.0) as f32;
                (wall_top - SLAB_BOTTOM_Y) * DISPLAY_OVERBURDEN_FRAC / bedrock_top
            };
            let k_a = scale_for(wa_top, col_a);
            let k_b = scale_for(wb_top, col_b);
            let y_at = |top: f32, k: f32, depth: f64| -> f32 {
                (top - depth as f32 * k).max(SLAB_BOTTOM_Y)
            };
            for bi in 0..4 {
                let ba = &col_a.bands[bi];
                let bb = &col_b.bands[bi];
                let color = palette::strata_color(ba.kind);
                let ya_top = y_at(wa_top, k_a, ba.top);
                let yb_top = y_at(wb_top, k_b, bb.top);
                let ya_bot = y_at(wa_top, k_a, ba.bottom);
                let yb_bot = y_at(wb_top, k_b, bb.bottom);
                let top_a = Vec3::new(a_out.x, ya_top, a_out.y);
                let top_b = Vec3::new(b_out.x, yb_top, b_out.y);
                let bot_b = Vec3::new(b_out.x, yb_bot, b_out.y);
                let bot_a = Vec3::new(a_out.x, ya_bot, a_out.y);
                wall.push_flat_quad(top_a, top_b, bot_b, bot_a, *normal, color);
            }

            // Water-table line: a thin strip across the face at the table
            // depth. Baked into the static wall (present on all tiers) AND,
            // on Medium+, duplicated into the animated overlay strip below.
            let wta = y_at(wa_top, k_a, col_a.water_table_depth);
            let wtb = y_at(wb_top, k_b, col_b.water_table_depth);
            if wta > SLAB_BOTTOM_Y + WATER_BAND_HALF_M {
                let wcolor = palette::strata_water_table();
                let sa_hi = Vec3::new(a_out.x, wta + WATER_BAND_HALF_M, a_out.y);
                let sb_hi = Vec3::new(b_out.x, wtb + WATER_BAND_HALF_M, b_out.y);
                let sb_lo = Vec3::new(b_out.x, wtb - WATER_BAND_HALF_M, b_out.y);
                let sa_lo = Vec3::new(a_out.x, wta - WATER_BAND_HALF_M, a_out.y);
                wall.push_flat_quad(sa_hi, sb_hi, sb_lo, sa_lo, *normal, wcolor);
                if medium_plus {
                    // Overlay strip sits a hair proud of the wall so it never
                    // z-fights; its material's emissive is pulsed each frame.
                    let out = n2 * 0.4;
                    let oa_hi =
                        Vec3::new(a_out.x + out.x, wta + WATER_BAND_HALF_M, a_out.y + out.y);
                    let ob_hi =
                        Vec3::new(b_out.x + out.x, wtb + WATER_BAND_HALF_M, b_out.y + out.y);
                    let ob_lo =
                        Vec3::new(b_out.x + out.x, wtb - WATER_BAND_HALF_M, b_out.y + out.y);
                    let oa_lo =
                        Vec3::new(a_out.x + out.x, wta - WATER_BAND_HALF_M, a_out.y + out.y);
                    water.push_flat_quad(oa_hi, ob_hi, ob_lo, oa_lo, *normal, wcolor);
                }
            }
        }
    }

    // Flat underside cap (bedrock), so low camera angles see a solid model
    // base rather than through the hollow slab.
    let bedrock = palette::strata_bedrock();
    wall.push_flat_quad(
        Vec3::new(-half, SLAB_BOTTOM_Y, -half),
        Vec3::new(half, SLAB_BOTTOM_Y, -half),
        Vec3::new(half, SLAB_BOTTOM_Y, half),
        Vec3::new(-half, SLAB_BOTTOM_Y, half),
        Vec3::NEG_Y,
        bedrock,
    );

    if wall.is_empty() {
        return;
    }
    let wall_mesh = meshes.add(wall.build());
    // Unlit + double-sided: the banded strata are flat authored colors (art
    // direction: "clean banded flat-color"), identical on every tier, and
    // double_sided/no-cull means the per-edge quad winding never matters.
    let wall_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        perceptual_roughness: 1.0,
        reflectance: 0.0,
        ..default()
    });
    let slab = commands
        .spawn((
            Mesh3d(wall_mesh),
            MeshMaterial3d(wall_mat),
            Transform::IDENTITY,
            Visibility::default(),
            DioramaSlab,
            Name::new("diorama-slab"),
        ))
        .id();
    state.slab = Some(slab);

    if medium_plus && !water.is_empty() {
        let water_mesh = meshes.add(water.build());
        let water_mat = materials.add(StandardMaterial {
            base_color: palette::strata_water_table(),
            unlit: true,
            double_sided: true,
            cull_mode: None,
            emissive: palette::emissive(palette::strata_water_table(), 0.6),
            ..default()
        });
        state.water_material = Some(water_mat.clone());
        let wb = commands
            .spawn((
                Mesh3d(water_mesh),
                MeshMaterial3d(water_mat),
                Transform::IDENTITY,
                Visibility::default(),
                DioramaWaterBand,
                Name::new("diorama-water-band"),
            ))
            .id();
        state.water_band = Some(wb);

        // Soft contact shadow: a radial-fade disc just under the slab so the
        // model reads as floating over the backdrop void. Per-vertex alpha
        // fades to zero at the rim — a hard-edged quad read as a "box" band
        // when seen edge-on (owner feedback), a gradient disc reads as a
        // soft studio shadow from every angle.
        // (Vertex-alpha on a Blend StandardMaterial is a known Bevy 0.16 trap
        // — vertex colors are ignored in the transparent pass — so the fade
        // comes from a tiny procedural radial texture, the same pattern as
        // the subway vignette.)
        let shadow_mesh = meshes.add(
            Plane3d::default()
                .mesh()
                .size(world_size * 1.55, world_size * 1.55)
                .build(),
        );
        let shadow_image = images.add(build_contact_shadow_image());
        let shadow_mat = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(shadow_image),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            double_sided: true,
            cull_mode: None,
            ..default()
        });
        let cs = commands
            .spawn((
                Mesh3d(shadow_mesh),
                MeshMaterial3d(shadow_mat),
                Transform::from_xyz(0.0, SLAB_BOTTOM_Y - 6.0, 0.0),
                Visibility::default(),
                DioramaContactShadow,
                Name::new("diorama-contact-shadow"),
            ))
            .id();
        state.contact_shadow = Some(cs);
    } else {
        state.water_material = None;
    }
}

/// Procedural radial-gradient shadow sprite (transparent rim -> soft dark
/// centre): the contact shadow under the floating slab. Small (64px) and
/// generated once per city rebuild; alpha peaks at the `palette::
/// contact_shadow` opacity in the centre and eases to zero by the rim.
fn build_contact_shadow_image() -> Image {
    use bevy::render::render_asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    const SIZE: u32 = 64;
    let centre = SIZE as f32 / 2.0 - 0.5;
    let peak = palette::contact_shadow().to_srgba();
    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = (x as f32 - centre) / centre;
            let dy = (y as f32 - centre) / centre;
            let dist = (dx * dx + dy * dy).sqrt().min(1.0);
            // Smooth ease-out: full strength in the middle, zero at the rim.
            let t = 1.0 - dist;
            let fall = t * t * (3.0 - 2.0 * t);
            let idx = ((y * SIZE + x) * 4) as usize;
            data[idx] = (peak.red * 255.0) as u8;
            data[idx + 1] = (peak.green * 255.0) as u8;
            data[idx + 2] = (peak.blue * 255.0) as u8;
            data[idx + 3] = (peak.alpha * fall * 255.0) as u8;
        }
    }
    Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// Gently pulse the water-table overlay's emissive so the groundwater line
/// shimmers (Medium+ only — the static baked line covers the lower tiers).
/// Also nudges the base color between two close blues. Cheap: one material.
fn animate_water_band_system(
    time: Res<Time>,
    state: Res<DioramaState>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(handle) = &state.water_material else {
        return;
    };
    let Some(mat) = materials.get_mut(handle) else {
        return;
    };
    let pulse = 0.5 + 0.5 * (time.elapsed_secs() * 1.6).sin();
    let strength = 0.35 + 0.55 * pulse;
    mat.emissive = palette::emissive(palette::strata_water_table(), strength);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::assertions_on_constants)] // documents a compile-time invariant
    fn slab_bottom_is_below_ground_plane() {
        assert!(SLAB_BOTTOM_Y < 0.0);
        assert_eq!(SLAB_BOTTOM_Y, -SLAB_DEPTH_M);
    }

    #[test]
    fn city_key_hash_distinguishes_profiles() {
        assert_ne!(city_key_hash(Some("nyc")), city_key_hash(Some("boston")));
        assert_eq!(city_key_hash(Some("nyc")), city_key_hash(Some("nyc")));
        assert_ne!(city_key_hash(None), city_key_hash(Some("nyc")));
    }
}
