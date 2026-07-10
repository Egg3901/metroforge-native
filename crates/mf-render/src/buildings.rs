//! Building fabric (spec §3.3 `buildings.rs`): merged per-chunk meshes (8x8
//! world chunks), white cuboids with TOP/SIDE/BASE vertex colors + ±3%
//! per-building brightness jitter (art-direction §1). Rebuilt whenever
//! `LatestFields.version` changes (mirrors `renderer.ts`'s
//! `setFields -> drawBuildings` gate).
//!
//! Real cities (a `buildingMask` present) sample the mask exactly per
//! `renderer.ts`'s `drawIsoBuildings`: `step = max(2, floor(res/96))`, a
//! ≥5/9 neighbor filter, footprint half-extent `cell*step*0.42`. Procedural
//! cities (no mask) walk local-road polylines, porting the typology
//! thresholds from `renderer.ts`'s `drawBuildings` (tower/apartment/
//! rowhouse/house by jobs/population density).
//!
//! **Deviation from `renderer.ts` (documented):** the web renderer is a 2D
//! isometric fake-extrusion (fixed `ISO_EXTRUDE = 140`) — it has no literal
//! per-building 3D height formula. Since this *is* a real 3D renderer,
//! heights are derived here from the same jobs/population density signals
//! `renderer.ts` uses for typology selection (`towerness = jobs/60`,
//! `resDensity = pop/55`), via `height = BASE + jobs*JOBS_WEIGHT +
//! pop*POP_WEIGHT` clamped to `[MIN_HEIGHT, MAX_HEIGHT]` for the real-mask
//! path, and fixed per-typology ranges for the procedural path.

use bevy::prelude::*;

use mf_state::{CurrentCity, HeightAt, LatestFields, QualityTier};

use crate::mesh_utils::{append_cuboid, hash01, MeshBuffers};
use crate::palette;

const CHUNKS_PER_SIDE: usize = 8;

// Real-city mask-driven height formula (see module doc for why this exists
// — `renderer.ts` has no literal equivalent).
const BASE_HEIGHT: f32 = 8.0;
const JOBS_WEIGHT: f32 = 1.6;
const POP_WEIGHT: f32 = 0.9;
const MIN_HEIGHT: f32 = 6.0;
const MAX_HEIGHT: f32 = 220.0;

/// Marker on each chunk entity so `subway.rs` can find them all to animate
/// the Y-scale squash.
#[derive(Component)]
pub struct BuildingChunk {
    /// World-space chunk center (X, Z) — used for draw-distance culling.
    pub center: Vec2,
}

pub struct MfBuildingsPlugin;

impl Plugin for MfBuildingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingsState>()
            .init_resource::<BuildingsDenseCenter>()
            .add_systems(
                Update,
                (
                    build_buildings_system.in_set(crate::MfRenderSet::Statics),
                    draw_distance_system.in_set(crate::MfRenderSet::Dynamic),
                    apply_quality_to_buildings_material_system.in_set(crate::MfRenderSet::Dynamic),
                    apply_night_dim_system.in_set(crate::MfRenderSet::Dynamic),
                ),
            );
    }
}

#[derive(Resource, Default)]
struct BuildingsState {
    version: Option<u32>,
    chunks: Vec<Entity>,
    material: Option<Handle<StandardMaterial>>,
    /// Night-dim factor (quantized, see `quantize_night_factor`) already
    /// baked into `material`'s `base_color`. Reset whenever `material` is
    /// (re)created so a rebuild is guaranteed one fresh dim application even
    /// if the ambient `night_factor` hasn't itself moved since the last
    /// rebuild — the new material always starts back at flat white.
    applied_night_factor_bucket: Option<i32>,
}

/// World-space (X, Z) center of the densest building chunk (most lots
/// placed in it this rebuild). Not part of the original spec — added so
/// `mf-game`'s camera (and this crate's own verification screenshots) can
/// frame "the interesting part of the city" instead of the city origin,
/// which for real-city data is frequently open water/parkland rather than
/// the built-up core (e.g. NYC's origin sits mid-harbor).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct BuildingsDenseCenter(pub Vec2);

fn chunk_index(pos: Vec2, world_size: f32) -> (usize, usize) {
    let half = world_size * 0.5;
    let cx = (((pos.x + half) / world_size) * CHUNKS_PER_SIDE as f32)
        .floor()
        .clamp(0.0, (CHUNKS_PER_SIDE - 1) as f32) as usize;
    let cz = (((pos.y + half) / world_size) * CHUNKS_PER_SIDE as f32)
        .floor()
        .clamp(0.0, (CHUNKS_PER_SIDE - 1) as f32) as usize;
    (cx, cz)
}

#[allow(clippy::too_many_arguments)]
fn build_buildings_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<LatestFields>,
    height_at: Res<HeightAt>,
    quality: Res<QualityTier>,
    day_night: Res<crate::daynight::DayNightState>,
    mut state: ResMut<BuildingsState>,
    mut dense_center: ResMut<BuildingsDenseCenter>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    let Some(f) = &fields.0 else {
        return;
    };
    if state.version == Some(f.version) {
        return;
    }
    state.version = Some(f.version);

    for e in state.chunks.drain(..) {
        commands.entity(e).despawn();
    }

    let world_size = city_json.world_size as f32;
    let chunk_size = world_size / CHUNKS_PER_SIDE as f32;
    let mut chunk_bufs: Vec<MeshBuffers> = (0..CHUNKS_PER_SIDE * CHUNKS_PER_SIDE)
        .map(|_| MeshBuffers::new())
        .collect();
    // Built VOLUME per chunk (footprint × height), purely to find the urban
    // core for `BuildingsDenseCenter` — not used for geometry. Lot COUNT
    // pointed at noise; footprint AREA pointed at warehouse superblocks;
    // volume is what makes Midtown the densest place in NYC.
    let mut chunk_lot_counts = vec![0f32; CHUNKS_PER_SIDE * CHUNKS_PER_SIDE];

    let top = palette::building_top();
    let side = palette::building_side();
    let base = palette::building_base();

    let field_w = city_json.field_w;
    let field_h = city_json.field_h;
    let cell_size = city_json.cell_size as f32;
    let origin_x = city_json.origin_x as f32;
    let origin_y = city_json.origin_y as f32;
    let land_use_at = |x: f32, z: f32| -> (f32, f32) {
        if field_w == 0 || field_h == 0 {
            return (0.0, 0.0);
        }
        let cx = (((x - origin_x) / cell_size) as i32).clamp(0, field_w as i32 - 1) as usize;
        let cz = (((z - origin_y) / cell_size) as i32).clamp(0, field_h as i32 - 1) as usize;
        let idx = cz * field_w as usize + cx;
        (
            f.jobs.get(idx).copied().unwrap_or(0.0),
            f.population.get(idx).copied().unwrap_or(0.0),
        )
    };

    let res = city_json.mask_res.unwrap_or(0);
    if let (Some(mask), true) = (&city.building_mask, res > 0) {
        // Real-city path — exact port of renderer.ts `drawIsoBuildings`'s
        // mask sampling rules.
        let mut place_lot = |x: f32, z: f32, half_x: f32, half_z: f32, jitter_key: (i32, i32)| {
            let (jobs, pop) = land_use_at(x, z);
            // Per-lot height variance so equal-density areas still read as
            // distinct building masses, not an extruded plateau.
            let hvar = 0.8
                + hash01(
                    jitter_key.0.wrapping_mul(7) + 3,
                    jitter_key.1.wrapping_mul(13) + 1,
                ) * 0.5;
            let height = ((BASE_HEIGHT + jobs * JOBS_WEIGHT + pop * POP_WEIGHT) * hvar)
                .clamp(MIN_HEIGHT, MAX_HEIGHT);
            let jitter = 1.0 + (hash01(jitter_key.0, jitter_key.1) - 0.5) * 0.06;
            let tint = |c: Color| -> Color {
                let s = c.to_srgba();
                Color::srgba(
                    (s.red * jitter).clamp(0.0, 1.0),
                    (s.green * jitter).clamp(0.0, 1.0),
                    (s.blue * jitter).clamp(0.0, 1.0),
                    s.alpha,
                )
            };
            let ground_y = height_at.sample(x, z);
            let (cx, cz) = chunk_index(Vec2::new(x, z), world_size);
            chunk_lot_counts[cz * CHUNKS_PER_SIDE + cx] += (half_x * half_z).max(1.0) * height;
            append_cuboid(
                &mut chunk_bufs[cz * CHUNKS_PER_SIDE + cx],
                Vec2::new(x, z),
                ground_y,
                half_x,
                half_z,
                height,
                tint(top),
                tint(side),
                tint(base),
            );
        };

        // Greedy rectangle decomposition of the FULL-RES footprint mask.
        // The web renderer's coarse `step` sampling reads as a fake uniform
        // grid of identical cubes in true 3D (owner feedback). Merging set
        // cells into variable-sized rectangles instead follows the real OSM
        // building fabric — actual blocks, voids, and shapes — from the same
        // shipped mask data. The wire `StaticMask` (spec §1.2 msgType=4) is a
        // strict 0/1 mask, so the threshold is `>= 1` (the web mask's 0..255
        // grading never crosses the wire).
        let res = res as i32;
        let half = world_size * 0.5;
        let cell = world_size / res as f32;
        // Cap merged extents (~90 m) so a solid city block reads as several
        // building masses rather than one monolithic slab.
        let max_cells = ((90.0 / cell).round() as i32).clamp(2, 8);
        let at = |gx: i32, gy: i32| -> bool { mask[(gy * res + gx) as usize] >= 1 };
        let mut consumed = vec![false; (res * res) as usize];
        let mut lots_emitted = 0u32;
        for gy in 0..res {
            for gx in 0..res {
                if consumed[(gy * res + gx) as usize] || !at(gx, gy) {
                    continue;
                }
                let mut w = 1;
                while gx + w < res
                    && w < max_cells
                    && !consumed[(gy * res + gx + w) as usize]
                    && at(gx + w, gy)
                {
                    w += 1;
                }
                let mut h = 1;
                'rows: while gy + h < res && h < max_cells {
                    for ox in 0..w {
                        let j = ((gy + h) * res + gx + ox) as usize;
                        if consumed[j] || !at(gx + ox, gy + h) {
                            break 'rows;
                        }
                    }
                    h += 1;
                }
                for oy in 0..h {
                    for ox in 0..w {
                        consumed[((gy + oy) * res + gx + ox) as usize] = true;
                    }
                }
                // Lone 1x1 cells with no set neighbor are mask noise; 1x1
                // cells attached to other fabric are real small buildings.
                if w == 1 && h == 1 {
                    let neighbors = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .filter(|(ox, oy)| {
                            let (nx, ny) = (gx + ox, gy + oy);
                            nx >= 0 && ny >= 0 && nx < res && ny < res && at(nx, ny)
                        })
                        .count();
                    if neighbors == 0 {
                        continue;
                    }
                }
                let x = -half + (gx as f32 + w as f32 * 0.5) * cell;
                let z = -half + (gy as f32 + h as f32 * 0.5) * cell;
                // Inset each mass so adjacent rectangles show seams and read
                // as separate buildings instead of fusing.
                let hx = (w as f32 * cell * 0.5 - 1.5).max(cell * 0.35);
                let hz = (h as f32 * cell * 0.5 - 1.5).max(cell * 0.35);
                lots_emitted += 1;
                place_lot(x, z, hx, hz, (gx, gy));
            }
        }
        let set_cells = mask.iter().filter(|&&v| v >= 1).count();
        info!("buildings: mask res={res} set_cells={set_cells} lots_emitted={lots_emitted}");
    } else {
        // Procedural path — port of renderer.ts `drawBuildings`'s
        // local-road walk + typology thresholds. Axis-aligned footprints
        // (a documented simplification vs. the web version's
        // road-tangent-oriented quads — see module doc).
        for road in &city_json.roads {
            if road.cls != "local" {
                continue;
            }
            let pts: Vec<Vec2> = road
                .points
                .chunks_exact(2)
                .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
                .collect();
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                let len = a.distance(b);
                if len < 40.0 {
                    continue;
                }
                let dir = (b - a) / len;
                let normal = Vec2::new(-dir.y, dir.x);
                let mut d = 22.0;
                while d < len - 22.0 {
                    let p = a + dir * d;
                    let (jobs, pop) = land_use_at(p.x, p.y);
                    let towerness = (jobs / 60.0).min(1.0);
                    let res_density = (pop / 55.0).min(1.0);
                    for side_sign in [-1.0f32, 1.0] {
                        let r = hash01((p.x * side_sign) as i32, (p.y + side_sign) as i32);
                        let (half_extent, height) = if towerness > 0.45 {
                            if r < 0.12 {
                                continue;
                            }
                            (12.0 + r * 8.0, 70.0 + r * 130.0)
                        } else if res_density > 0.55 {
                            if r < 0.15 {
                                continue;
                            }
                            (11.0 + r * 5.0, 28.0 + r * 30.0)
                        } else if res_density > 0.25 {
                            if r < 0.28 {
                                continue;
                            }
                            (7.5 + r * 3.5, 12.0 + r * 10.0)
                        } else {
                            if r < 0.5 {
                                continue;
                            }
                            (4.0 + r * 2.5, 6.0 + r * 5.0)
                        };
                        let setback = 20.0 + r * 8.0;
                        let c = p + normal * side_sign * (setback + half_extent);
                        let jitter = 1.0 + (r - 0.5) * 0.06;
                        let tint = |col: Color| -> Color {
                            let s = col.to_srgba();
                            Color::srgba(
                                (s.red * jitter).clamp(0.0, 1.0),
                                (s.green * jitter).clamp(0.0, 1.0),
                                (s.blue * jitter).clamp(0.0, 1.0),
                                s.alpha,
                            )
                        };
                        let ground_y = height_at.sample(c.x, c.y);
                        let (cx, cz) = chunk_index(c, world_size);
                        chunk_lot_counts[cz * CHUNKS_PER_SIDE + cx] += 1.0;
                        append_cuboid(
                            &mut chunk_bufs[cz * CHUNKS_PER_SIDE + cx],
                            c,
                            ground_y,
                            half_extent,
                            half_extent,
                            height,
                            tint(top),
                            tint(side),
                            tint(base),
                        );
                    }
                    d += 34.0;
                }
            }
        }
    }

    if let Some((densest_idx, &count)) = chunk_lot_counts
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
    {
        if count > 0.0 {
            let cx = densest_idx % CHUNKS_PER_SIDE;
            let cz = densest_idx / CHUNKS_PER_SIDE;
            let half = world_size * 0.5;
            dense_center.0 = Vec2::new(
                -half + (cx as f32 + 0.5) * chunk_size,
                -half + (cz as f32 + 0.5) * chunk_size,
            );
        }
    }

    let unlit = quality.knobs().unlit_material;
    // Night-dim only applies on unlit tiers (see `apply_night_dim_system`);
    // bake the *current* dim in at creation time so a mid-night rebuild
    // doesn't flash back to flat white until the next dim pass happens to
    // run — the material starts already correct.
    let base_color = if unlit {
        Color::WHITE.mix(&palette::building_night(), day_night.night_factor)
    } else {
        Color::WHITE
    };
    let material = materials.add(StandardMaterial {
        double_sided: true,
        cull_mode: None,
        base_color,
        unlit,
        ..default()
    });
    state.material = Some(material.clone());
    state.applied_night_factor_bucket = if unlit {
        Some(quantize_night_factor(day_night.night_factor))
    } else {
        None
    };

    let non_empty_chunks = chunk_bufs.iter().filter(|b| !b.is_empty()).count();
    info!(
        "mf-render buildings: has_building_mask={} mask_present={} mask_res={:?} non_empty_chunks={}/{}",
        city_json.has_building_mask,
        city.building_mask.is_some(),
        city_json.mask_res,
        non_empty_chunks,
        CHUNKS_PER_SIDE * CHUNKS_PER_SIDE
    );

    for (i, buf) in chunk_bufs.into_iter().enumerate() {
        if buf.is_empty() {
            continue;
        }
        let cx = i % CHUNKS_PER_SIDE;
        let cz = i / CHUNKS_PER_SIDE;
        let half = world_size * 0.5;
        let center = Vec2::new(
            -half + (cx as f32 + 0.5) * chunk_size,
            -half + (cz as f32 + 0.5) * chunk_size,
        );
        let mesh = meshes.add(buf.build());
        let entity = commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                Transform::IDENTITY,
                Visibility::default(),
                BuildingChunk { center },
                Name::new(format!("buildings-chunk-{cx}-{cz}")),
            ))
            .id();
        state.chunks.push(entity);
    }
}

/// Per-tier building draw distance (spec §4: 3/6/12km/unlimited).
fn draw_distance_system(
    quality: Res<QualityTier>,
    chunks: Query<(Entity, &BuildingChunk)>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut visibility: Query<&mut Visibility>,
) {
    let Ok(cam) = cameras.single() else {
        return;
    };
    let cam_xz = Vec2::new(cam.translation.x, cam.translation.z);
    let max_dist = quality.knobs().building_draw_distance_m;
    for (entity, chunk) in &chunks {
        let Ok(mut vis) = visibility.get_mut(entity) else {
            continue;
        };
        let visible = match max_dist {
            None => true,
            Some(limit) => cam_xz.distance(chunk.center) <= limit,
        };
        *vis = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

fn apply_quality_to_buildings_material_system(
    quality: Res<QualityTier>,
    state: Res<BuildingsState>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !quality.is_changed() {
        return;
    }
    let Some(handle) = &state.material else {
        return;
    };
    if let Some(mat) = materials.get_mut(handle) {
        mat.unlit = quality.knobs().unlit_material;
    }
}

/// Quantize `night_factor` to 1/256 steps: continuous drift during dusk/dawn
/// shouldn't defeat the steady-state cache below over a shade of dimming
/// finer than anyone could see, and tiers where day/night is disabled
/// (`night_factor` pinned at one value forever) need this to bucket to a
/// single, stable value.
fn quantize_night_factor(v: f32) -> i32 {
    (v * 256.0).round() as i32
}

/// Night-dims the shared buildings material toward `palette::building_night`
/// (art-direction §6: "night = ... buildings dim to #b9bec4"). Only needed
/// on unlit tiers — lit tiers already darken naturally as the directional
/// light's illuminance drops (`daynight.rs`), and stacking both would
/// over-darken.
fn apply_night_dim_system(
    quality: Res<QualityTier>,
    day_night: Res<crate::daynight::DayNightState>,
    mut state: ResMut<BuildingsState>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !quality.knobs().unlit_material {
        return;
    }
    let Some(handle) = state.material.clone() else {
        return;
    };
    // `materials.get_mut` unconditionally marks the shared building material
    // dirty for GPU re-upload; on potato/low, day/night is disabled entirely
    // so `night_factor` never moves and this would otherwise repaint the
    // whole city's material every frame forever for no visual change.
    let bucket = quantize_night_factor(day_night.night_factor);
    if state.applied_night_factor_bucket == Some(bucket) {
        return;
    }
    if let Some(mat) = materials.get_mut(&handle) {
        mat.base_color = Color::WHITE.mix(&palette::building_night(), day_night.night_factor);
    }
    state.applied_night_factor_bucket = Some(bucket);
}
