//! Ground mesh (spec §3.3 `terrain.rs`): rebuilt whenever `LatestFields`'s
//! `version` (or the quality tier's subdivision divisor) changes. Also owns
//! replacing `mf_state::HeightAt`'s placeholder flat-ground closure with a
//! real bilinear sampler once fields have arrived, so every other layer
//! (roads/buildings/transit/vehicles/agents/camera) can position things on
//! the ground.
//!
//! Water: on Potato, water stays flat vertex-colored quads in this mesh
//! (status quo). On Low+, pure-water quads are omitted here and drawn by
//! `water.rs`'s stylized `WaterMaterial` overlay instead.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;

use mf_state::{CurrentCity, EffectiveKnobs, HeightAt, LatestFields, Theme};

use crate::atmosphere::CloudShadowParams;
use crate::mesh_utils::MeshBuffers;
use crate::palette;
use crate::roads::{ARTERIAL_WIDTH, COLLECTOR_WIDTH, LOCAL_WIDTH};
use crate::terrain_material::{TerrainExtension, TerrainMaterial};
use crate::water::{
    make_water_material, water_bundle, WaterMaterial, WaterMeshBuffers, WATER_SURFACE_Y,
};

/// Legacy fallback vertical scale for the NORMALIZED (0..1) sim
/// `Fields.terrain` heightfield — the procedural-relief path used only when a
/// city ships no real-elevation channel (msgType=7). Real cities now bake a
/// dedicated DEM heightfield in TRUE METERS (see `StaticElevation` /
/// `GridSpace`), which bypasses this scale entirely.
///
/// Was 300 (spec §3.3: max relief 200-400m): that much artificial relief
/// buried road ribbons between their sparse vertices and sliced terrain
/// through building prisms on slopes. 90m reads as gentle procedural relief.
pub const TERRAIN_Z_SCALE: f32 = 90.0;

/// Vertical exaggeration applied to REAL (meters) elevation for readability.
/// Kept at 1.0 = honest true-meters: at these city-block camera zooms real
/// relief (SF's ~100 m hills over a few km) already reads clearly, and any
/// exaggeration desyncs foundation-clamped buildings / draped roads from
/// their real-world proportions. Exposed as a named constant so it is a
/// single, documented knob rather than a magic literal if a future art pass
/// wants gentle punch-up.
pub const TERRAIN_VERTICAL_EXAGGERATION: f32 = 1.0;
/// Water is rendered dead flat, slightly below the nominal ground plane so
/// shorelines don't z-fight with adjacent land vertices.
pub const WATER_LEVEL_Y: f32 = -0.4;

/// Marker on the terrain ground mesh so other layers (subway view dim) can
/// find its material without reaching into `TerrainState`.
#[derive(Component)]
pub struct TerrainSurface;

pub struct MfTerrainPlugin;

impl Plugin for MfTerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainState>()
            .add_systems(
                Update,
                (
                    build_terrain_system,
                    apply_quality_to_terrain_material_system,
                )
                    .in_set(crate::MfRenderSet::Terrain),
            )
            .add_systems(
                Update,
                apply_cloud_shadow_to_terrain_system
                    .in_set(crate::MfRenderSet::Dynamic)
                    .after(crate::atmosphere::AtmosphereReady),
            );
    }
}

#[derive(Resource, Default)]
struct TerrainState {
    /// `(fields.version, subdiv_divisor, theme, shader_water)` — the
    /// geometry/color-affecting knobs. `Theme` is included so a theme
    /// switch forces the same full rebuild path as a subdivision change,
    /// rather than needing its own in-place vertex-color recolor system
    /// (ground/water/park colors are baked directly into the mesh's
    /// vertex-color attribute at build time, not read from the material
    /// each frame). `shader_water` flips when `water_quality` crosses
    /// zero (Potato flat-in-terrain vs Low+ separate water mesh).
    key: Option<(u32, u32, Theme, bool, u32)>,
    entity: Option<Entity>,
    water_entity: Option<Entity>,
    material: Option<Handle<TerrainMaterial>>,
}

/// A regular sample-point grid over world space: point `(i, j)` sits at
/// `(origin_x + i*cell_size, origin_y + j*cell_size)`. Used to describe both
/// the height source (meters) and the water mask, which — since the real
/// elevation channel and the sim water field differ in resolution — are no
/// longer guaranteed to share a grid.
#[derive(Clone, Copy)]
struct GridSpace {
    w: i32,
    h: i32,
    cell_size: f32,
    origin_x: f32,
    origin_y: f32,
}

impl GridSpace {
    #[allow(clippy::type_complexity)]
    fn corners(&self, gx: f32, gy: f32) -> (i32, i32, i32, i32, f32, f32) {
        let x0 = gx.floor().clamp(0.0, (self.w - 1) as f32) as i32;
        let y0 = gy.floor().clamp(0.0, (self.h - 1) as f32) as i32;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let tx = (gx - x0 as f32).clamp(0.0, 1.0);
        let ty = (gy - y0 as f32).clamp(0.0, 1.0);
        (x0, y0, x1, y1, tx, ty)
    }

    fn grid_coords(&self, x: f32, z: f32) -> (f32, f32) {
        (
            (x - self.origin_x) / self.cell_size,
            (z - self.origin_y) / self.cell_size,
        )
    }

    fn bilinear_f32(&self, arr: &[f32], x: f32, z: f32) -> f32 {
        let (gx, gy) = self.grid_coords(x, z);
        let (x0, y0, x1, y1, tx, ty) = self.corners(gx, gy);
        let w = self.w;
        let v00 = arr[(y0 * w + x0) as usize];
        let v10 = arr[(y0 * w + x1) as usize];
        let v01 = arr[(y1 * w + x0) as usize];
        let v11 = arr[(y1 * w + x1) as usize];
        (v00 * (1.0 - tx) + v10 * tx) * (1.0 - ty) + (v01 * (1.0 - tx) + v11 * tx) * ty
    }

    fn bilinear_u8(&self, arr: &[u8], x: f32, z: f32) -> f32 {
        let (gx, gy) = self.grid_coords(x, z);
        let (x0, y0, x1, y1, tx, ty) = self.corners(gx, gy);
        let w = self.w;
        let v00 = arr[(y0 * w + x0) as usize] as f32;
        let v10 = arr[(y0 * w + x1) as usize] as f32;
        let v01 = arr[(y1 * w + x0) as usize] as f32;
        let v11 = arr[(y1 * w + x1) as usize] as f32;
        (v00 * (1.0 - tx) + v10 * tx) * (1.0 - ty) + (v01 * (1.0 - tx) + v11 * tx) * ty
    }
}

/// Real bilinear terrain sampler (replaces `mf_state::HeightAt`'s flat-
/// ground placeholder). Cheap to clone (all backing arrays are `Arc`), so
/// the `HeightAt` closure can hold one directly. `heights` is in TRUE METERS
/// (already graded + exaggerated) on `height_space`; `water` is the sim
/// water field on its own (coarser) `water_space`.
struct TerrainSampleData {
    height_space: GridSpace,
    water_space: GridSpace,
    heights: Arc<Vec<f32>>,
    water: Arc<Vec<u8>>,
}

impl TerrainSampleData {
    /// `(x, z)` here is world X / Bevy Z (coordinate convention: world Y ->
    /// Bevy Z).
    fn sample(&self, x: f32, z: f32) -> f32 {
        if self.height_space.w < 2 || self.height_space.h < 2 {
            return 0.0;
        }
        let land_y = self.height_space.bilinear_f32(&self.heights, x, z);
        // Shoreline height blends across a band instead of a hard `> 0.5`
        // cliff (#112): a binary cut on the bilinearly-interpolated water
        // fraction snapped whole cells between land height and water level,
        // giving a stair-stepped coast. Ease land -> water level across
        // [0.4, 0.6] so the shore descends smoothly (beach/quay) and inland
        // cells stay at true terrain height.
        let water_frac = self.water_space.bilinear_u8(&self.water, x, z);
        if water_frac <= 0.4 {
            return land_y;
        }
        if water_frac >= 0.6 {
            return WATER_LEVEL_Y;
        }
        let t = smoothstep(0.4, 0.6, water_frac);
        land_y + (WATER_LEVEL_Y - land_y) * t
    }
}

#[allow(clippy::too_many_arguments)]
fn build_terrain_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<LatestFields>,
    effective: Res<EffectiveKnobs>,
    theme: Res<Theme>,
    mut state: ResMut<TerrainState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut water_materials: ResMut<Assets<WaterMaterial>>,
    cloud_shadows: Res<CloudShadowParams>,
    mut height_at: ResMut<HeightAt>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    let Some(f) = &fields.0 else {
        return;
    };
    let divisor = effective.0.terrain_subdiv_divisor.max(1);
    let shader_water = effective.0.water_quality > 0;
    // Fold the elevation channel's resolution (0 = none) into the rebuild key
    // so the mesh rebuilds when the one-shot msgType=7 frame arrives after
    // `fields` (elevation is static, so its res alone captures presence).
    let elev_res = city.elevation.as_ref().map(|e| e.res).unwrap_or(0);
    let key = (f.version, divisor, *theme, shader_water, elev_res);
    if state.key == Some(key) {
        return;
    }
    state.key = Some(key);

    if let Some(e) = state.entity.take() {
        commands.entity(e).despawn();
    }
    if let Some(e) = state.water_entity.take() {
        commands.entity(e).despawn();
    }

    let field_w = city_json.field_w;
    let field_h = city_json.field_h;
    if field_w < 2 || field_h < 2 || f.terrain.len() != (field_w * field_h) as usize {
        return;
    }
    let field_cell = city_json.cell_size as f32;
    let field_origin_x = city_json.origin_x as f32;
    let field_origin_y = city_json.origin_y as f32;

    // The water/park sim fields keep their own (coarse, 96²) grid space.
    // `GridSpace::grid_coords` treats the origin as the world position of grid
    // sample (0,0) with NO half-cell term — i.e. the origin must be the first
    // cell CENTRE (exactly how `height_space` bakes it below:
    // `-HALF + 0.5·cell`). The sim field values ARE cell-centre samples
    // (`fields.ts` `cellCenter`/`sampleField`, which subtracts 0.5), but
    // `city_json.origin_x/y` is the field CORNER (`-(w·cell)/2 = -HALF`). Using
    // the raw corner origin for bilinear sampling therefore displaced the whole
    // rendered shoreline half a field cell (≈62.5 m) south-east of the true
    // coastline, so shoreline-hugging roads ran into water ("land masses don't
    // line up with roads", issue #141). Shift to the cell centre to match the
    // sampler's convention and `height_space`. (Nearest-cell lookups like
    // `traffic.rs` keep the corner origin — correct for their floor(), so
    // `city_json.origin_x` is left untouched.)
    let water_space = GridSpace {
        w: field_w as i32,
        h: field_h as i32,
        cell_size: field_cell,
        origin_x: field_origin_x + field_cell * 0.5,
        origin_y: field_origin_y + field_cell * 0.5,
    };

    // Height source: prefer the real-elevation channel (msgType=7) in TRUE
    // METERS at its own (finer) resolution; fall back to the normalized sim
    // `f.terrain` scaled by TERRAIN_Z_SCALE for cities that ship no DEM.
    // `world_size` is the full square edge (both channels cover it), so the
    // elevation sample-point grid places point (0,0) at the first cell CENTER
    // (`-HALF + 0.5*cell`, matching build-cities.ts's bake) with `cell =
    // world_size/res`.
    let (height_space, raw_heights) = match city.elevation.as_ref() {
        Some(elev) if elev.res >= 2 && elev.heights.len() == (elev.res * elev.res) as usize => {
            let res = elev.res;
            let world_size = city_json.world_size as f32;
            let cell = world_size / res as f32;
            let origin = -world_size / 2.0 + cell * 0.5;
            // Re-base to the city's lowest sample so inland cities (Cleveland
            // ~172m ASL, Atlanta ~156m) sit on the y=0 water/ground plane like
            // the coastal ones, instead of floating on an absolute-ASL plateau.
            let base_m = elev.heights.iter().copied().min().unwrap_or(0) as f32;
            let heights: Vec<f32> = elev
                .heights
                .iter()
                .map(|&m| (m as f32 - base_m) * TERRAIN_VERTICAL_EXAGGERATION)
                .collect();
            (
                GridSpace {
                    w: res as i32,
                    h: res as i32,
                    cell_size: cell,
                    origin_x: origin,
                    origin_y: origin,
                },
                heights,
            )
        }
        _ => {
            let heights: Vec<f32> = f.terrain.iter().map(|&t| t * TERRAIN_Z_SCALE).collect();
            (water_space, heights)
        }
    };

    // Grade (flatten) the height source in a corridor under each road so
    // `roads.rs`'s ribbons — which sample the SAME graded heights via
    // `HeightAt` — agree with the ground mesh instead of visually slicing a
    // stripe across a downhill building's lower wall on slopes (issue #33).
    // Grading in the height grid's own space keeps roads/terrain/buildings
    // (via `HeightAt.sample`, which buildings' footprint-min and stations
    // both go through) all reading one graded ground with no extra plumbing.
    let graded_heights = grade_terrain(
        &raw_heights,
        height_space.w as u32,
        height_space.h as u32,
        height_space.cell_size,
        height_space.origin_x,
        height_space.origin_y,
        &city_json.roads,
        city_json.road_scale as f32,
    );

    // Stepped grid indices for this tier's subdivision divisor, always
    // including the far edge so the mesh reaches the city's full extent. The
    // mesh now walks the HEIGHT grid (256² for DEM cities), so higher tiers
    // resolve real relief crisply while the divisor still coarsens it on
    // weaker GPUs.
    let hf_w = height_space.w as u32;
    let hf_h = height_space.h as u32;
    let stepped = |n: u32, step: u32| -> Vec<u32> {
        let mut v: Vec<u32> = (0..n).step_by(step as usize).collect();
        if *v.last().unwrap_or(&0) != n - 1 {
            v.push(n - 1);
        }
        v
    };
    let xs = stepped(hf_w, divisor);
    let ys = stepped(hf_h, divisor);

    let mut land_buf = MeshBuffers::new();
    let mut water_buf = WaterMeshBuffers::new();
    let ground = palette::ground();
    let water = palette::water();
    let park = palette::park();

    let vertex_at = |ix: usize, iy: usize| -> (Vec3, Color, f32) {
        let gx = xs[ix];
        let gy = ys[iy];
        let idx = (gy * hf_w + gx) as usize;
        // World position of this height-grid sample point.
        let x = height_space.origin_x + gx as f32 * height_space.cell_size;
        let z = height_space.origin_y + gy as f32 * height_space.cell_size;
        // Water/park come from the (independently-resolved) sim fields,
        // bilinearly sampled at this vertex's world position — a naturally
        // soft shoreline that no longer depends on the height grid matching
        // the field grid cell-for-cell (they differ for DEM cities).
        let water_frac = water_space.bilinear_u8(&f.water, x, z).clamp(0.0, 1.0);
        let is_park = water_space.bilinear_u8(&f.parks, x, z) > 0.5;
        let y = if water_frac > 0.5 {
            WATER_LEVEL_Y
        } else {
            graded_heights.get(idx).copied().unwrap_or(0.0)
        };
        let land = if is_park { park } else { ground };
        // Potato: bake water into vertex color (status quo). Low+: land
        // mesh stays land-colored; the water overlay carries the water look.
        let color = if !shader_water {
            if water_frac <= 0.0 {
                land
            } else if water_frac >= 1.0 {
                water
            } else {
                land.mix(&water, water_frac)
            }
        } else {
            land
        };
        (Vec3::new(x, y, z), color, water_frac)
    };

    for iy in 0..ys.len().saturating_sub(1) {
        for ix in 0..xs.len().saturating_sub(1) {
            let (p00, c00, f00) = vertex_at(ix, iy);
            let (p10, c10, f10) = vertex_at(ix + 1, iy);
            let (p11, c11, f11) = vertex_at(ix + 1, iy + 1);
            let (p01, c01, f01) = vertex_at(ix, iy + 1);
            let max_f = f00.max(f10).max(f11).max(f01);
            let min_f = f00.min(f10).min(f11).min(f01);

            // Land: skip pure-water quads when the water shader owns them
            // (avoids double-drawing NYC harbors). Potato keeps everything.
            let emit_land = !shader_water || min_f < 0.99;
            if emit_land {
                // Winding vs the declared `+Y` normal: `ix` walks `+X`, `iy`
                // walks `+Z` (`vertex_at`'s `x`/`z` both increase with their
                // index). Taking `p00` as the origin, `v1 = p10-p00 ~= (dx,0,0)`
                // and `v2 = p11-p00 ~= (dx,0,dz)` (dx,dz > 0); the right-hand
                // cross product `v1 x v2 = (0, -dx*dz, 0)` — i.e. `(p00,p10,p11)`
                // winds CCW as seen from *below* (`-Y`), not from above where
                // the camera and the declared normal both are. `push_quad`
                // needs (p0,p1,p2) CCW from `normal` (Bevy/wgpu front-face =
                // CCW), so the naive `(p00,p10,p11,p01)` order is backwards;
                // swapping the middle two args to `(p00,p01,p11,p10)` reverses
                // the same quad and flips the cross product to `+Y`.
                land_buf.push_quad(p00, p01, p11, p10, Vec3::Y, c00, c01, c11, c10);
            }

            // Water overlay: any quad that touches water. Flat at
            // WATER_SURFACE_Y with water_frac in UV0.x for shoreline foam.
            if shader_water && max_f > 0.05 {
                let y = WATER_SURFACE_Y;
                let w00 = Vec3::new(p00.x, y, p00.z);
                let w10 = Vec3::new(p10.x, y, p10.z);
                let w11 = Vec3::new(p11.x, y, p11.z);
                let w01 = Vec3::new(p01.x, y, p01.z);
                water_buf.push_quad(w00, w01, w11, w10, f00, f01, f11, f10);
            }
        }
    }
    if land_buf.is_empty() {
        return;
    }
    let mesh = meshes.add(land_buf.build());

    let unlit = effective.0.unlit_material;
    // Grid quads verified CCW-from-+Y below (fixed to match) — single-sided,
    // back-face-culled is correct for a ground plane only ever seen from
    // above. (An A/B-diffed brightness regression in the subway+Potato
    // combination initially looked like it implicated this material too,
    // but root-caused to roads.rs's `unlit` flag going stale on a runtime
    // quality change — see the comment on `apply_quality_to_roads_material_
    // system` there. This material's own `unlit` already updates reactively
    // via `apply_quality_to_terrain_material_system` below, so it was never
    // actually the source.)
    let material = materials.add(TerrainMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            unlit,
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            ..default()
        },
        extension: TerrainExtension {
            cloud: Vec4::new(
                cloud_shadows.offset.x,
                cloud_shadows.offset.y,
                cloud_shadows.strength,
                cloud_shadows.inv_scale,
            ),
            weather: Vec4::ZERO,
            cloud_noise: Some(cloud_shadows.texture.clone()),
        },
    });
    state.material = Some(material.clone());

    let entity = commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::default(),
            TerrainSurface,
        ))
        .id();
    state.entity = Some(entity);

    if shader_water && !water_buf.is_empty() {
        let water_mesh = meshes.add(water_buf.build());
        let water_mat = water_materials.add(make_water_material(effective.0.water_quality));
        let water_entity = commands.spawn(water_bundle(water_mesh, water_mat)).id();
        state.water_entity = Some(water_entity);
    }

    let data = Arc::new(TerrainSampleData {
        height_space,
        water_space,
        heights: Arc::new(graded_heights),
        water: Arc::new(f.water.clone()),
    });
    height_at.0 = Box::new(move |x, z| data.sample(x, z));
}

/// One road-corridor segment used by [`grade_terrain`]: a straight span
/// between two consecutive road-polyline points, carrying the RAW (pre-grade,
/// pre-`TERRAIN_Z_SCALE`) terrain height sampled at each endpoint so the
/// corridor's target elevation interpolates smoothly along the road instead
/// of pinning to one endpoint.
struct GradeSeg {
    a: Vec2,
    b: Vec2,
    height_a: f32,
    height_b: f32,
    /// Half the ribbon width (`roads.rs`'s per-class width, already
    /// `road_scale`-multiplied) — vertices inside this are graded flush to
    /// the road profile height.
    half_width: f32,
    /// Extra falloff distance past `half_width` over which the blend
    /// smoothsteps back to the raw terrain (the "shoulder").
    shoulder: f32,
}

/// Bilinear-sample a raw (un-graded, un-scaled) heightfield at a world `(x,
/// z)` position. Standalone from `TerrainSampleData::bilinear_f32` because
/// this runs BEFORE that sampler exists this frame (grading is a
/// precondition of building it) and only ever needs the terrain channel.
#[allow(clippy::too_many_arguments)]
fn sample_raw_bilinear(
    raw: &[f32],
    field_w: u32,
    field_h: u32,
    cell_size: f32,
    origin_x: f32,
    origin_y: f32,
    x: f32,
    z: f32,
) -> f32 {
    if field_w < 2 || field_h < 2 {
        return 0.0;
    }
    let gx = ((x - origin_x) / cell_size).clamp(0.0, (field_w - 1) as f32);
    let gy = ((z - origin_y) / cell_size).clamp(0.0, (field_h - 1) as f32);
    let x0 = gx.floor() as u32;
    let y0 = gy.floor() as u32;
    let x1 = (x0 + 1).min(field_w - 1);
    let y1 = (y0 + 1).min(field_h - 1);
    let tx = gx - x0 as f32;
    let ty = gy - y0 as f32;
    let at = |xi: u32, yi: u32| raw[(yi * field_w + xi) as usize];
    let v00 = at(x0, y0);
    let v10 = at(x1, y0);
    let v01 = at(x0, y1);
    let v11 = at(x1, y1);
    (v00 * (1.0 - tx) + v10 * tx) * (1.0 - ty) + (v01 * (1.0 - tx) + v11 * tx) * ty
}

/// Classic Hermite smoothstep, clamped: 0 at/before `edge0`, 1 at/after
/// `edge1`, smooth (zero-derivative-at-both-ends) in between.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Closest point on segment `a..b` to `p`: returns `(distance, t)` where `t
/// in [0,1]` is the interpolation parameter of the closest point (used to
/// blend the segment's two endpoint heights).
fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> (f32, f32) {
    let ab = b - a;
    let len_sq = ab.length_squared();
    let t = if len_sq > 1e-6 {
        ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let closest = a + ab * t;
    ((p - closest).length(), t)
}

/// Grade (flatten) `raw` in a corridor under every road segment so the
/// ground mesh and `HeightAt` (which roads/buildings/stations all sample)
/// agree with the road ribbons instead of the terrain slicing through them
/// on slopes (issue #33). Each vertex within `half_width` of the nearest
/// road segment is pulled flush to that segment's interpolated profile
/// height; vertices out to `half_width + shoulder` blend back to the raw
/// terrain via [`smoothstep`]; further away is untouched.
///
/// Only grades the road network baked into `static_city` at load time —
/// this crate rebuilds the ground mesh keyed on `LatestFields`'s
/// `version`/subdivision-tier only (see `TerrainState::key` / the doc
/// comment atop this file), and player-placed transit stations arrive later
/// via the separate, purely-dynamic `LatestUi` resource with no terrain
/// rebuild trigger of their own. Grading around stations too would need
/// either (a) folding station positions into the same rebuild key so a
/// terrain rebuild fires when stations change, or (b) a dedicated
/// remesh-on-edit system — neither exists yet, so this is road-corridor-only
/// for now (see PR description / issue #33 for the follow-up note).
///
/// Uses a spatial grid (`bucket_size`-keyed) over the road segments so each
/// vertex only tests nearby segments instead of the full network —
/// necessary since this runs over every heightfield vertex, not just the
/// (coarser, quality-tiered) mesh subdivision.
#[allow(clippy::too_many_arguments)]
fn grade_terrain(
    raw: &[f32],
    field_w: u32,
    field_h: u32,
    cell_size: f32,
    origin_x: f32,
    origin_y: f32,
    roads: &[mf_protocol::RoadDto],
    road_scale: f32,
) -> Vec<f32> {
    let mut out = raw.to_vec();
    if roads.is_empty() || field_w < 2 || field_h < 2 {
        return out;
    }

    let mut segs: Vec<GradeSeg> = Vec::new();
    for road in roads {
        let width = match road.cls.as_str() {
            "arterial" => ARTERIAL_WIDTH as f32,
            "collector" => COLLECTOR_WIDTH as f32,
            _ => LOCAL_WIDTH as f32,
        } * road_scale;
        let half_width = width / 2.0;
        // Shoulder scales with the road's own half-width (wider roads get a
        // proportionally wider grade-out) but is clamped to a sane 6-10m
        // band so a local road's shoulder isn't imperceptibly thin and an
        // arterial's isn't absurdly wide.
        let shoulder = (half_width * 0.3).clamp(6.0, 10.0);
        let pts: Vec<Vec2> = road
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let height_a = sample_raw_bilinear(
                raw, field_w, field_h, cell_size, origin_x, origin_y, a.x, a.y,
            );
            let height_b = sample_raw_bilinear(
                raw, field_w, field_h, cell_size, origin_x, origin_y, b.x, b.y,
            );
            segs.push(GradeSeg {
                a,
                b,
                height_a,
                height_b,
                half_width,
                shoulder,
            });
        }
    }
    if segs.is_empty() {
        return out;
    }

    // Bucket size just needs to comfortably exceed the widest corridor
    // (arterial: half_width 30 + shoulder ~9 = ~39m) so a 1-ring neighbor
    // search around each vertex's own bucket never misses a segment.
    const BUCKET_SIZE: f32 = 50.0;
    let bucket_key = |p: Vec2| -> (i32, i32) {
        (
            (p.x / BUCKET_SIZE).floor() as i32,
            (p.y / BUCKET_SIZE).floor() as i32,
        )
    };
    let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        let reach = s.half_width + s.shoulder;
        let min_p = Vec2::new(s.a.x.min(s.b.x) - reach, s.a.y.min(s.b.y) - reach);
        let max_p = Vec2::new(s.a.x.max(s.b.x) + reach, s.a.y.max(s.b.y) + reach);
        let (kx0, ky0) = bucket_key(min_p);
        let (kx1, ky1) = bucket_key(max_p);
        for kx in kx0..=kx1 {
            for ky in ky0..=ky1 {
                buckets.entry((kx, ky)).or_default().push(i);
            }
        }
    }

    for iy in 0..field_h {
        for ix in 0..field_w {
            let idx = (iy * field_w + ix) as usize;
            let x = origin_x + ix as f32 * cell_size;
            let z = origin_y + iy as f32 * cell_size;
            let (kx, ky) = bucket_key(Vec2::new(x, z));
            let mut best: Option<(f32, f32, f32, f32)> = None; // (dist, target_height, half_width, shoulder)
            for dkx in -1..=1 {
                for dky in -1..=1 {
                    let Some(list) = buckets.get(&(kx + dkx, ky + dky)) else {
                        continue;
                    };
                    for &si in list {
                        let s = &segs[si];
                        let (dist, t) = point_segment_distance(Vec2::new(x, z), s.a, s.b);
                        if best.is_none_or(|(best_dist, ..)| dist < best_dist) {
                            let target_height = s.height_a + (s.height_b - s.height_a) * t;
                            best = Some((dist, target_height, s.half_width, s.shoulder));
                        }
                    }
                }
            }
            let Some((dist, target_height, half_width, shoulder)) = best else {
                continue;
            };
            if dist >= half_width + shoulder {
                continue;
            }
            // 0 at dist<=half_width (fully graded to the road profile), 1 at
            // dist>=half_width+shoulder (untouched raw terrain).
            let raw_weight = smoothstep(half_width, half_width + shoulder, dist);
            out[idx] = target_height * (1.0 - raw_weight) + raw[idx] * raw_weight;
        }
    }

    out
}

/// If only the quality tier changed (not the fields version/subdivision —
/// that's the full-rebuild path above), just flip the existing material's
/// `unlit` flag rather than rebuilding the whole ground mesh.
fn apply_quality_to_terrain_material_system(
    effective: Res<EffectiveKnobs>,
    state: Res<TerrainState>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    if !effective.is_changed() {
        return;
    }
    let Some(handle) = &state.material else {
        return;
    };
    if let Some(mat) = materials.get_mut(handle) {
        mat.base.unlit = effective.0.unlit_material;
    }
}

fn apply_cloud_shadow_to_terrain_system(
    shadows: Res<CloudShadowParams>,
    state: Res<TerrainState>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    let Some(handle) = &state.material else {
        return;
    };
    if let Some(mat) = materials.get_mut(handle) {
        mat.extension.cloud = Vec4::new(
            shadows.offset.x,
            shadows.offset.y,
            shadows.strength,
            shadows.inv_scale,
        );
        if mat.extension.cloud_noise.is_none() && shadows.texture != Handle::default() {
            mat.extension.cloud_noise = Some(shadows.texture.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GridSpace;

    /// Locks the `GridSpace` bilinear convention that the `water_space` fix
    /// depends on: the origin is the world position of grid sample (0,0) with
    /// NO half-cell term, so a `GridSpace` built at the first cell CENTRE must
    /// map that centre to grid coord 0.0 and return the cell's exact value.
    /// A regression to a CORNER origin (the half-cell bug behind roads running
    /// into water, issue #141) shifts this and fails the exact-value asserts.
    #[test]
    fn bilinear_u8_hits_cell_centres_under_centre_origin() {
        // Mirror how build_terrain_system now builds water_space: field of
        // side `n`, cell `world/n`, origin at the first cell CENTRE.
        let n: i32 = 4;
        let world = 12000.0_f32;
        let cell = world / n as f32;
        let corner = -world / 2.0;
        let centre_origin = corner + cell * 0.5;
        let space = GridSpace {
            w: n,
            h: n,
            cell_size: cell,
            origin_x: centre_origin,
            origin_y: centre_origin,
        };
        // A distinctive per-cell pattern so an off-by-one/half-cell shift shows.
        let vals: Vec<u8> = (0..(n * n)).map(|i| (i * 17 % 251) as u8).collect();
        for gy in 0..n {
            for gx in 0..n {
                // World position of this cell's centre.
                let x = corner + (gx as f32 + 0.5) * cell;
                let z = corner + (gy as f32 + 0.5) * cell;
                let got = space.bilinear_u8(&vals, x, z);
                let want = vals[(gy * n + gx) as usize] as f32;
                assert!(
                    (got - want).abs() < 1e-3,
                    "cell ({gx},{gy}) at world ({x},{z}): got {got}, want {want}",
                );
            }
        }
    }
}
