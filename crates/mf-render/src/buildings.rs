//! Building fabric (spec §3.3 `buildings.rs`): merged per-chunk meshes (8x8
//! world chunks), white cel-shaded masses with TOP/SIDE/BASE vertex colors +
//! ±3% per-building brightness jitter (art-direction §1). Rebuilt whenever
//! `LatestFields.version` changes, or when `CurrentCity.buildings` arrives
//! or its content changes (see `BuildingsState::rebuild_key`) — mirrors
//! `renderer.ts`'s `setFields -> drawBuildings` gate, extended for the
//! native-only vector footprint data `renderer.ts` has no equivalent of.
//!
//! Procedural facade detail (Medium/High): `facade.wgsl` paints window grids
//! from world-position; this module generates parapet + occasional AC-box
//! rooftop massing at mesh-build time and pushes `DayNightState.night_factor`
//! into the shared material uniform. Potato/Low keep flat cel masses.
//!
//! Three paths, tried in this priority order:
//! - **Real-footprint path** (`CurrentCity.buildings` present and
//!   non-empty): extrudes the actual per-building polygon (`BuildingFootprint`,
//!   spec §1.2 msgType=5) into a prism via `mesh_utils::append_prism` — real
//!   OSM building shapes, not axis-aligned boxes. This is the owner's north
//!   star (real Google-Maps-3D-massing-style per-building geometry) and is
//!   used whenever the sidecar has sent it, superseding both paths below.
//! - **Mask path** (`buildingMask` present, no vector data): samples the
//!   mask exactly per `renderer.ts`'s `drawIsoBuildings`: `step =
//!   max(2, floor(res/96))`, a ≥5/9 neighbor filter, footprint half-extent
//!   `cell*step*0.42`, decomposed into greedy rectangles. Kept as a
//!   fallback for cities the sidecar hasn't sent vector footprints for yet.
//! - **Procedural path** (neither): walks local-road polylines, porting the
//!   typology thresholds from `renderer.ts`'s `drawBuildings` (tower/
//!   apartment/rowhouse/house by jobs/population density).
//!
//! **Deviation from `renderer.ts` (documented):** the web renderer is a 2D
//! isometric fake-extrusion (fixed `ISO_EXTRUDE = 140`) — it has no literal
//! per-building 3D height formula. Since this *is* a real 3D renderer,
//! heights are derived here from the same jobs/population density signals
//! `renderer.ts` uses for typology selection (`towerness = jobs/60`,
//! `resDensity = pop/55`), via `height = BASE + jobs*JOBS_WEIGHT +
//! pop*POP_WEIGHT` clamped to `[MIN_HEIGHT, MAX_HEIGHT]` for the real-mask
//! path, and fixed per-typology ranges for the procedural path. The
//! real-footprint path uses `BuildingFootprint.height_dm` verbatim when
//! present (`> 0`), falling back to this same density formula (clamped to
//! its own, wider range — see `FOOTPRINT_MIN_HEIGHT`/`FOOTPRINT_MAX_HEIGHT`)
//! for buildings the sidecar didn't have a real height for (`height_dm == 0`).

use bevy::math::Vec3A;
use bevy::prelude::*;
use bevy::render::primitives::Aabb;

use mf_protocol::BuildingFootprint;
use mf_state::{CurrentCity, EffectiveKnobs, HeightAt, LatestFields, RevealState, Theme};

use crate::atmosphere::CloudShadowParams;
use crate::mesh_utils::{
    append_cuboid_cel, append_prism, append_rooftop_detail, hash01, polygon_area, MeshBuffers,
};
use crate::palette;
use crate::reveal::{BuildingMaterial, RevealExtension};
use crate::RenderCacheStats;

const CHUNKS_PER_SIDE: usize = 8;

// Real-city mask-driven height formula (see module doc for why this exists
// — `renderer.ts` has no literal equivalent).
const BASE_HEIGHT: f32 = 8.0;
const JOBS_WEIGHT: f32 = 1.6;
const POP_WEIGHT: f32 = 0.9;
const MIN_HEIGHT: f32 = 6.0;
const MAX_HEIGHT: f32 = 220.0;

// Real-footprint path height clamp — wider than the mask path's
// `MIN_HEIGHT`/`MAX_HEIGHT` because real vector footprints carry actual
// building geometry (down to small rowhouses, up past supertalls) instead of
// a coarse rasterized mask cell, so the plausible range is wider in both
// directions.
const FOOTPRINT_MIN_HEIGHT: f32 = 3.0;
const FOOTPRINT_MAX_HEIGHT: f32 = 500.0;

// Wall cel-shading tint amounts relative to plain `building_side` (art
// direction: a flat, quantized three-tone read, no shader work). See
// `mesh_utils::append_prism`'s doc for the sun-direction dot-product
// thresholds that pick between these and plain `side`.
const WALL_SUNLIT_BRIGHTEN: f32 = 0.04;
const WALL_SHADED_DARKEN: f32 = -0.07;

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
                    apply_facade_uniforms_system.in_set(crate::MfRenderSet::Dynamic),
                    apply_reveal_system.in_set(crate::MfRenderSet::Dynamic),
                    apply_cloud_shadow_to_buildings_system
                        .in_set(crate::MfRenderSet::Dynamic)
                        .after(crate::atmosphere::AtmosphereReady),
                ),
            );
    }
}

#[derive(Resource, Default)]
struct BuildingsState {
    version: Option<u32>,
    /// The theme baked into `chunks`' vertex colors last time they were
    /// built. Building TOP/SIDE/BASE colors are baked directly into each
    /// chunk mesh's vertex-color attribute (art-direction §1), not read
    /// from the material each frame, so a theme switch needs to force the
    /// same full rebuild path as a `version`/`buildings_count` change.
    theme: Option<Theme>,
    /// Building count of the last-seen `CurrentCity.buildings` (`None` if it
    /// hadn't arrived yet, `Some(0)` is never observed since the real-
    /// footprint path only exists when the vec is non-empty — see
    /// `rebuild_key`). `StaticBuildings` (spec §1.2 msgType=5) arrives once,
    /// shortly after `ready`, and can land AFTER the first `Fields`-version-
    /// triggered rebuild already ran with the mask/procedural fallback.
    /// Tracking this alongside `version` means that late arrival flips this
    /// field and forces exactly one more rebuild onto the real-footprint
    /// path, instead of being silently missed until the next unrelated
    /// `Fields.version` bump (which may be minutes away, or never, on a
    /// paused city).
    buildings_count: Option<usize>,
    chunks: Vec<Entity>,
    material: Option<Handle<BuildingMaterial>>,
    /// Night-dim factor (quantized, see `quantize_night_factor`) already
    /// baked into `material.base`'s `base_color`. Reset whenever `material`
    /// is (re)created so a rebuild is guaranteed one fresh dim application
    /// even if the ambient `night_factor` hasn't itself moved since the last
    /// rebuild — the new material always starts back at flat white.
    applied_night_factor_bucket: Option<i32>,
    /// Quantized `RevealState` last written into `material.extension` (see
    /// `apply_reveal_system`). Reset whenever `material` is (re)created so a
    /// fresh material picks up the current reveal state on the next tick
    /// instead of relying on `RevealState` itself having moved since the
    /// last rebuild.
    applied_reveal_bucket: Option<(i32, i32, i32, i32, i32)>,
    /// Last `(night_factor_bucket, facade_enabled)` written into
    /// `material.extension.facade` — same no-churn discipline as the reveal
    /// / night-dim buckets.
    applied_facade_bucket: Option<(i32, bool)>,
}

impl BuildingsState {
    /// `(fields.version, buildings-present key)` — the rebuild gate. Bundled
    /// as one method so the "when do we redo the geometry" decision has one
    /// definition instead of two fields compared ad hoc at each call site.
    fn rebuild_key(city: &CurrentCity, fields_version: u32) -> (u32, Option<usize>) {
        let buildings_count = city
            .buildings
            .as_ref()
            .map(|b| b.buildings.len())
            .filter(|&n| n > 0);
        (fields_version, buildings_count)
    }
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

/// Why `resolve_part_extent` skipped a building:part instead of resolving a
/// height to draw. Kept as a distinct value (rather than a plain `None`) so
/// `build_buildings_system` can count the two cases separately for its
/// debug log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartSkipReason {
    /// `height_dm == 0` (unknown) AND `min_height_dm > 0`: there is no
    /// density formula for "how tall is the segment that starts partway up
    /// a building," unlike the ground-based (`min == 0`) fallback case
    /// below, so there is nothing sane to guess.
    UnknownHeightWithMin,
    /// The resolved `min` is at or above the resolved `height`: no
    /// positive-thickness prism to draw (degenerate or corrupt wire data).
    MinAtOrAboveHeight,
}

/// Resolve one building:part's `(base_offset, height)` in meters from its
/// wire `height_dm`/`min_height_dm`. `land_use_at` is only invoked when the
/// ground-based density-formula fallback actually applies (`height_dm == 0`
/// and `min_height_dm == 0`), so callers can pass a closure that samples the
/// jobs/population fields lazily. Factored out of `build_buildings_system`'s
/// per-part loop for the same reason as `emit_building_prism`: it is
/// unit-testable without a Bevy `App`.
/// Flag building:parts that are exact-footprint duplicates of another part
/// with an overlapping vertical extent, so the mesh builder can skip them
/// (issue #141).
///
/// Real NYC OSM data ships the same `building:part` ring twice when a way is
/// both a standalone element and a member of a relation the extractor also
/// walked (e.g. Central Park Tower arrives as two byte-identical 8-vertex
/// rings, one 466 m and one 472 m tall). Extruding both produces two
/// coincident prisms whose walls z-fight in the depth buffer AND
/// self-shadow each other in the shadow map — the tower renders solid black
/// with stippled edges at every hour of the day. Dropping the shorter twin
/// fixes the render with zero visual loss: the kept prism covers the whole
/// shared extent.
///
/// Rules, per group of byte-identical vertex rings (non-duplicates never
/// enter a group and are never skipped):
/// - Parts are considered tallest-first (`height_dm` descending, unknown
///   `height_dm == 0` last); the first part of a group is always kept.
/// - A part is skipped iff its wire vertical extent `[min_height_dm,
///   height_dm)` overlaps an extent already kept in its group. Legitimate
///   same-footprint vertical stacking (e.g. a spire segment `mh=420 h=500`
///   above a `0..417` tower body) has disjoint extents and keeps both.
/// - `height_dm == 0` (unknown height, resolved later by the density
///   formula) is treated as covering everything: two unknown-height twins
///   would resolve to the same formula height and be exactly coincident.
fn duplicate_part_skips(parts: &[BuildingFootprint]) -> Vec<bool> {
    use std::collections::HashMap;

    let mut skip = vec![false; parts.len()];
    // Group by bit-exact ring bytes; verts come off the wire as i16
    // half-meter fixed point, so identical source rings decode bit-identical.
    let mut groups: HashMap<Vec<(u32, u32)>, Vec<usize>> = HashMap::new();
    for (i, bd) in parts.iter().enumerate() {
        if bd.verts.len() < 3 {
            continue;
        }
        let key: Vec<(u32, u32)> = bd
            .verts
            .iter()
            .map(|v| (v[0].to_bits(), v[1].to_bits()))
            .collect();
        groups.entry(key).or_default().push(i);
    }

    let extent_of = |i: usize| -> (u32, u32) {
        let bd = &parts[i];
        if bd.height_dm == 0 {
            (0, u32::MAX) // unknown: resolved later, assume full coverage
        } else {
            (bd.min_height_dm as u32, bd.height_dm as u32)
        }
    };

    for mut members in groups.into_values() {
        if members.len() < 2 {
            continue;
        }
        // Tallest first so the twin that covers the most extent is kept.
        members.sort_by_key(|&i| {
            let h = parts[i].height_dm;
            core::cmp::Reverse(if h == 0 { u32::MAX } else { h as u32 })
        });
        let mut kept: Vec<(u32, u32)> = Vec::with_capacity(members.len());
        for i in members {
            let (lo, hi) = extent_of(i);
            if kept.iter().any(|&(klo, khi)| lo < khi && klo < hi) {
                skip[i] = true;
            } else {
                kept.push((lo, hi));
            }
        }
    }
    skip
}

fn resolve_part_extent(
    bd: &BuildingFootprint,
    jitter_key: (i32, i32),
    land_use_at: impl FnOnce() -> (f32, f32),
) -> Result<(f32, f32), PartSkipReason> {
    let min_m = bd.min_height_dm as f32 / 10.0;
    let height = if bd.height_dm > 0 {
        bd.height_dm as f32 / 10.0
    } else if min_m == 0.0 {
        // Unknown real height on a ground-based part: fall back to the same
        // density formula (and hvar jitter) the mask path uses, so
        // buildings the sidecar didn't have height data for still read as
        // varied masses instead of a flat plateau.
        let (jobs, pop) = land_use_at();
        let hvar = 0.8
            + hash01(
                jitter_key.0.wrapping_mul(7) + 3,
                jitter_key.1.wrapping_mul(13) + 1,
            ) * 0.5;
        // Mega-footprints with unknown height are stations, terminals, and
        // convention halls, not towers: untamed, the Midtown density
        // formula extruded Penn Station's 148k m2 outline into a block-wide
        // monolith (owner: "what is the giant box building"). Cap the
        // FALLBACK (never real tag data) so allowed height shrinks as
        // footprint area grows past a city-block 5k m2.
        let area = polygon_area(
            &bd.verts
                .iter()
                .map(|v| Vec2::new(v[0], v[1]))
                .collect::<Vec<_>>(),
        );
        let area_cap = if area > 5_000.0 {
            (BASE_HEIGHT + 120_000_000.0 / (area * area.sqrt())).max(12.0)
        } else {
            f32::MAX
        };
        ((BASE_HEIGHT + jobs * JOBS_WEIGHT + pop * POP_WEIGHT) * hvar).min(area_cap)
    } else {
        return Err(PartSkipReason::UnknownHeightWithMin);
    }
    .clamp(FOOTPRINT_MIN_HEIGHT, FOOTPRINT_MAX_HEIGHT);

    if min_m >= height {
        return Err(PartSkipReason::MinAtOrAboveHeight);
    }
    Ok((min_m, height))
}

/// Emit one real building:part's prism (walls + roof cap, plus a bottom cap
/// when `base_offset > 0`) into `buf`, given the already-resolved
/// `ground_y`/`base_offset`/`height`/colors. Factored out of
/// `build_buildings_system`'s per-building loop so it's unit-testable
/// without a Bevy `App`: it takes the wire type directly and touches nothing
/// but `mesh_utils`. Returns `(vertices_added, indices_added)` straight from
/// `append_prism`. A footprint with fewer than 3 verts is a no-op (decode
/// already enforces the wire's 3..=64 range, but this never trusts that
/// twice).
#[allow(clippy::too_many_arguments)]
fn emit_building_prism(
    buf: &mut MeshBuffers,
    building: &BuildingFootprint,
    ground_y: f32,
    base_offset: f32,
    height: f32,
    top: Color,
    side_plain: Color,
    side_sunlit: Color,
    side_shaded: Color,
    base: Color,
) -> (usize, usize) {
    if building.verts.len() < 3 {
        return (0, 0);
    }
    let ring: Vec<Vec2> = building
        .verts
        .iter()
        .map(|v| Vec2::new(v[0], v[1]))
        .collect();
    append_prism(
        buf,
        &ring,
        ground_y,
        base_offset,
        height,
        top,
        side_plain,
        side_sunlit,
        side_shaded,
        base,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_buildings_system(
    mut commands: Commands,
    city: Res<CurrentCity>,
    fields: Res<LatestFields>,
    height_at: Res<HeightAt>,
    effective: Res<EffectiveKnobs>,
    theme: Res<Theme>,
    day_night: Res<crate::daynight::DayNightState>,
    cloud_shadows: Res<CloudShadowParams>,
    mut state: ResMut<BuildingsState>,
    mut dense_center: ResMut<BuildingsDenseCenter>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<BuildingMaterial>>,
    mut stats: ResMut<RenderCacheStats>,
    counters: Res<crate::perf::PerfCounters>,
) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    let Some(f) = &fields.0 else {
        return;
    };
    let (new_version, new_buildings_count) = BuildingsState::rebuild_key(&city, f.version);
    if state.version == Some(new_version)
        && state.buildings_count == new_buildings_count
        && state.theme == Some(*theme)
    {
        return;
    }
    let _span = tracing::info_span!("buildings_rebuild").entered();
    let _timer = crate::perf::PerfSpan::start(&counters.buildings_rebuild_us);
    state.version = Some(new_version);
    state.buildings_count = new_buildings_count;
    state.theme = Some(*theme);

    for e in state.chunks.drain(..) {
        commands.entity(e).despawn();
    }

    let world_size = city_json.world_size as f32;
    let chunk_size = world_size / CHUNKS_PER_SIDE as f32;
    let real_footprints = city.buildings.as_ref().filter(|b| !b.buildings.is_empty());
    // Real building data ships with a per-city vertex total; estimate each
    // building's final contribution as `4*vc` wall vertices (one quad per
    // ring edge) + `3*(vc-2)` cap-triangle vertices, spread evenly across
    // the 64 chunks (a uniform-density approximation — cheap and good
    // enough to avoid most of `Vec`'s repeated-doubling reallocs across the
    // ~2M-vertex NYC case; it's fine if a few dense chunks still grow past
    // it, this is a perf hint, not a correctness bound).
    let prealloc = real_footprints.map(|b| {
        let total: usize = b
            .buildings
            .iter()
            .map(|bd| {
                let vc = bd.verts.len();
                if vc >= 3 {
                    // Walls + roof cap + parapet (one quad per edge).
                    4 * vc + 3 * (vc - 2) + 4 * vc
                } else {
                    0
                }
            })
            .sum();
        let per_chunk_v = total / (CHUNKS_PER_SIDE * CHUNKS_PER_SIDE) + 64;
        let per_chunk_i = per_chunk_v * 3 / 2 + 96;
        (per_chunk_v, per_chunk_i)
    });
    let mut chunk_bufs: Vec<MeshBuffers> = (0..CHUNKS_PER_SIDE * CHUNKS_PER_SIDE)
        .map(|_| match prealloc {
            Some((v, i)) => MeshBuffers::with_capacity(v, i),
            None => MeshBuffers::new(),
        })
        .collect();
    // Built VOLUME per chunk (footprint × height), purely to find the urban
    // core for `BuildingsDenseCenter` — not used for geometry. Lot COUNT
    // pointed at noise; footprint AREA pointed at warehouse superblocks;
    // volume is what makes Midtown the densest place in NYC.
    let mut chunk_lot_counts = vec![0f32; CHUNKS_PER_SIDE * CHUNKS_PER_SIDE];

    let top = palette::building_top();
    let side = palette::building_side();
    let base = palette::building_base();
    let side_sunlit = palette::brighten(side, WALL_SUNLIT_BRIGHTEN);
    let side_shaded = palette::brighten(side, WALL_SHADED_DARKEN);

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
    if let Some(buildings) = real_footprints {
        // Real-footprint path (owner's north star): extrude the actual
        // per-building polygon instead of any rectangle approximation. Takes
        // priority over both fallbacks below whenever the sidecar has sent
        // vector data for this city.
        //
        // One real-world OSM building can arrive as several stacked
        // `building:part` footprints (a ground podium, a tower set back on
        // top of it, a spire on top of that): each part's own
        // `min_height_dm` says where ITS prism starts, so this loop treats
        // every entry in `buildings.buildings` as one independent part to
        // extrude, not one whole building.
        let mut prism_vertex_total = 0usize;
        let mut skipped_unknown_height_with_min = 0usize;
        let mut skipped_min_at_or_above_height = 0usize;
        let duplicate_skips = duplicate_part_skips(&buildings.buildings);
        let skipped_duplicate_footprint = duplicate_skips.iter().filter(|s| **s).count();
        for (part_idx, bd) in buildings.buildings.iter().enumerate() {
            if duplicate_skips[part_idx] {
                continue; // coincident duplicate of a kept part (issue #141)
            }
            if bd.verts.len() < 3 {
                continue; // decode already enforces 3..=64; never trust twice
            }
            let ring: Vec<Vec2> = bd.verts.iter().map(|v| Vec2::new(v[0], v[1])).collect();
            let centroid = ring.iter().fold(Vec2::ZERO, |acc, p| acc + *p) / ring.len() as f32;
            let jitter_key = (centroid.x as i32, centroid.y as i32);

            let (min_m, height) =
                match resolve_part_extent(bd, jitter_key, || land_use_at(centroid.x, centroid.y)) {
                    Ok(extent) => extent,
                    Err(PartSkipReason::UnknownHeightWithMin) => {
                        skipped_unknown_height_with_min += 1;
                        continue;
                    }
                    Err(PartSkipReason::MinAtOrAboveHeight) => {
                        skipped_min_at_or_above_height += 1;
                        continue;
                    }
                };

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

            // Ground the flat prism bottom on sloped terrain (#113). Sample
            // the footprint's ground at every ring corner, every edge
            // midpoint (a long wall can cross a dip lower than either corner),
            // and the centroid; take the LOWEST. Then sink the base under it
            // by a foundation skirt that grows with the footprint's own relief
            // span (`hi - lo`), so the downhill edge stays buried instead of
            // floating even between samples. Flat ground keeps the old 3m
            // skirt. This robustness is what lets the relief cap rise later
            // without prisms slicing through hillsides.
            let mut lo = height_at.sample(centroid.x, centroid.y);
            let mut hi = lo;
            let n = ring.len();
            for i in 0..n {
                let a = ring[i];
                let b = ring[(i + 1) % n];
                for p in [a, (a + b) * 0.5] {
                    let g = height_at.sample(p.x, p.y);
                    lo = lo.min(g);
                    hi = hi.max(g);
                }
            }
            let ground_y = lo - (3.0 + (hi - lo) * 0.5);
            let (cx, cz) = chunk_index(centroid, world_size);
            // Same volume-argmax semantics as the mask path above (footprint
            // AREA x height, not lot count), real polygon area this time
            // instead of a rectangle's half-extent product. Uses the part's
            // own built slab (height - min), not its absolute top height, so
            // a tall building's upper stacked parts don't get double-counted
            // as if each also included the podium below it.
            let area = polygon_area(&ring).max(1.0);
            chunk_lot_counts[cz * CHUNKS_PER_SIDE + cx] += area * (height - min_m);

            let (v, _i) = emit_building_prism(
                &mut chunk_bufs[cz * CHUNKS_PER_SIDE + cx],
                bd,
                ground_y,
                min_m,
                height,
                tint(top),
                tint(side),
                tint(side_sunlit),
                tint(side_shaded),
                tint(base),
            );
            // Parapet + occasional AC boxes on the roof deck (mesh-time
            // detail; shader facades handle the walls).
            append_rooftop_detail(
                &mut chunk_bufs[cz * CHUNKS_PER_SIDE + cx],
                &ring,
                ground_y + height,
                area,
                jitter_key,
                tint(top),
                tint(side),
            );
            prism_vertex_total += v;
        }
        if skipped_unknown_height_with_min > 0
            || skipped_min_at_or_above_height > 0
            || skipped_duplicate_footprint > 0
        {
            debug!(
                "buildings: real-footprint path skipped {} parts (unknown height + nonzero min), {} parts (min >= height), {} parts (duplicate coincident footprint)",
                skipped_unknown_height_with_min,
                skipped_min_at_or_above_height,
                skipped_duplicate_footprint
            );
        }
        let chunks_used = chunk_bufs.iter().filter(|b| !b.is_empty()).count();
        info!(
            "buildings: real-footprint path building_count={} prism_vertex_total={} chunks_used={}/{}",
            buildings.buildings.len(),
            prism_vertex_total,
            chunks_used,
            CHUNKS_PER_SIDE * CHUNKS_PER_SIDE
        );
    } else if let (Some(mask), true) = (&city.building_mask, res > 0) {
        // Mask path — exact port of renderer.ts `drawIsoBuildings`'s
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
            let area = (half_x * 2.0) * (half_z * 2.0);
            chunk_lot_counts[cz * CHUNKS_PER_SIDE + cx] += (half_x * half_z).max(1.0) * height;
            let buf = &mut chunk_bufs[cz * CHUNKS_PER_SIDE + cx];
            append_cuboid_cel(
                buf,
                Vec2::new(x, z),
                ground_y,
                half_x,
                half_z,
                height,
                tint(top),
                tint(side),
                tint(side_sunlit),
                tint(side_shaded),
                tint(base),
            );
            let ring = [
                Vec2::new(x - half_x, z - half_z),
                Vec2::new(x - half_x, z + half_z),
                Vec2::new(x + half_x, z + half_z),
                Vec2::new(x + half_x, z - half_z),
            ];
            append_rooftop_detail(
                buf,
                &ring,
                ground_y + height,
                area,
                jitter_key,
                tint(top),
                tint(side),
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
                        let buf = &mut chunk_bufs[cz * CHUNKS_PER_SIDE + cx];
                        append_cuboid_cel(
                            buf,
                            c,
                            ground_y,
                            half_extent,
                            half_extent,
                            height,
                            tint(top),
                            tint(side),
                            tint(side_sunlit),
                            tint(side_shaded),
                            tint(base),
                        );
                        let ring = [
                            Vec2::new(c.x - half_extent, c.y - half_extent),
                            Vec2::new(c.x - half_extent, c.y + half_extent),
                            Vec2::new(c.x + half_extent, c.y + half_extent),
                            Vec2::new(c.x + half_extent, c.y - half_extent),
                        ];
                        append_rooftop_detail(
                            buf,
                            &ring,
                            ground_y + height,
                            (half_extent * 2.0).powi(2),
                            ((p.x * side_sign) as i32, (p.y + side_sign) as i32),
                            tint(top),
                            tint(side),
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

    // Facade window grids stay Medium/High only (`!unlit_material` is exactly
    // those two tiers). Building cel-lighting is a *separate* knob so the low
    // tiers can light their masses without lighting the black road material.
    let facade_enabled = !effective.0.unlit_material;
    let building_unlit = !effective.0.building_lit;
    // Night-dim only applies to unlit buildings (see `apply_night_dim_system`);
    // bake the *current* dim in at creation time so a mid-night rebuild
    // doesn't flash back to flat white until the next dim pass happens to
    // run — the material starts already correct. Lit buildings darken via the
    // directional light instead, so they start plain white here.
    let base_color = if building_unlit {
        Color::WHITE.mix(&palette::building_night(), day_night.night_factor)
    } else {
        Color::WHITE
    };
    // Both `append_cuboid` (mask/procedural paths) and `append_prism`
    // (real-footprint path) are individually verified CCW-from-declared-
    // normal (see mesh_utils.rs) — single-sided, back-face-culled is
    // correct for all three. (Same note as terrain.rs: an initially-
    // suspected brightness regression here root-caused to roads.rs's stale
    // `unlit` flag, not to this material — this one's `unlit` already
    // updates reactively via `apply_quality_to_buildings_material_system`
    // below.)
    let material = materials.add(BuildingMaterial {
        base: StandardMaterial {
            base_color,
            unlit: building_unlit,
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            ..default()
        },
        // Fresh material starts with reveal off; facade night/enable are
        // baked from the live day-night + tier so a mid-night Medium rebuild
        // doesn't flash unlit windows for a frame. Cloud-shadow texture is
        // wired from `CloudShadowParams` (may still be default Handle at first
        // build); reveal is picked up by `apply_reveal_system` next tick since
        // `applied_reveal_bucket` is reset below.
        extension: RevealExtension {
            cloud_noise: Some(cloud_shadows.texture.clone()),
            facade: Vec4::new(
                day_night.night_factor,
                if facade_enabled { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ),

            ..default()
        },
    });
    state.material = Some(material.clone());
    state.applied_night_factor_bucket = if building_unlit {
        Some(quantize_night_factor(day_night.night_factor))
    } else {
        None
    };
    state.applied_reveal_bucket = None;
    state.applied_facade_bucket = Some((
        quantize_night_factor(day_night.night_factor),
        facade_enabled,
    ));

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
        // Chunk-aligned AABB (not a full vertex scan): frustum-cull friendly
        // and O(1) at spawn. Y half-extent covers water-level basements up
        // through FOOTPRINT_MAX_HEIGHT skyscrapers so culling stays correct
        // without walking millions of verts on NYC.
        let half_xz = chunk_size * 0.5;
        let aabb = Aabb {
            center: Vec3A::new(center.x, 200.0, center.y),
            half_extents: Vec3A::new(half_xz, 400.0, half_xz),
        };
        let entity = commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                Transform::IDENTITY,
                Visibility::default(),
                aabb,
                BuildingChunk { center },
                Name::new(format!("buildings-chunk-{cx}-{cz}")),
            ))
            .id();
        state.chunks.push(entity);
    }
    stats.building_chunks = state.chunks.len();
}

/// Per-tier building draw distance (spec §4: 3/6/12km/unlimited).
fn draw_distance_system(
    effective: Res<EffectiveKnobs>,
    chunks: Query<(Entity, &BuildingChunk)>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut visibility: Query<&mut Visibility>,
    counters: Res<crate::perf::PerfCounters>,
) {
    let _span = tracing::info_span!("building_draw_distance").entered();
    let _timer = crate::perf::PerfSpan::start(&counters.building_draw_distance_us);
    let Ok(cam) = cameras.single() else {
        return;
    };
    let cam_xz = Vec2::new(cam.translation.x, cam.translation.z);
    let max_dist = effective.0.building_draw_distance_m;
    for (entity, chunk) in &chunks {
        let Ok(mut vis) = visibility.get_mut(entity) else {
            continue;
        };
        let visible = match max_dist {
            None => true,
            Some(limit) => cam_xz.distance(chunk.center) <= limit,
        };
        let next = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        crate::perf::set_visibility_if_changed(&mut vis, next, Some(&counters));
    }
}

fn apply_quality_to_buildings_material_system(
    effective: Res<EffectiveKnobs>,
    day_night: Res<crate::daynight::DayNightState>,
    mut state: ResMut<BuildingsState>,
    mut materials: ResMut<Assets<BuildingMaterial>>,
) {
    if !effective.is_changed() {
        return;
    }
    let Some(handle) = &state.material else {
        return;
    };
    let facade_enabled = !effective.0.unlit_material;
    if let Some(mat) = materials.get_mut(handle) {
        mat.base.unlit = !effective.0.building_lit;
        mat.extension.facade = Vec4::new(
            day_night.night_factor,
            if facade_enabled { 1.0 } else { 0.0 },
            0.0,
            0.0,
        );
    }
    state.applied_facade_bucket = Some((
        quantize_night_factor(day_night.night_factor),
        facade_enabled,
    ));
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
/// for unlit buildings — lit buildings (every tier once `building_lit` is on)
/// darken naturally as the directional light's illuminance drops
/// (`daynight.rs`), and stacking both would over-darken.
fn apply_night_dim_system(
    effective: Res<EffectiveKnobs>,
    day_night: Res<crate::daynight::DayNightState>,
    mut state: ResMut<BuildingsState>,
    mut materials: ResMut<Assets<BuildingMaterial>>,
) {
    if effective.0.building_lit {
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
        mat.base.base_color = Color::WHITE.mix(&palette::building_night(), day_night.night_factor);
    }
    state.applied_night_factor_bucket = Some(bucket);
}

/// Pushes `DayNightState.night_factor` + Medium/High facade enable into the
/// shared building material's `facade` uniform (read by `facade.wgsl`).
/// Does not touch `daynight.rs`. Quantized so dusk/dawn drift doesn't
/// re-upload the uniform every frame.
fn apply_facade_uniforms_system(
    effective: Res<EffectiveKnobs>,
    day_night: Res<crate::daynight::DayNightState>,
    mut state: ResMut<BuildingsState>,
    mut materials: ResMut<Assets<BuildingMaterial>>,
) {
    let facade_enabled = !effective.0.unlit_material;
    let bucket = (
        quantize_night_factor(day_night.night_factor),
        facade_enabled,
    );
    if state.applied_facade_bucket == Some(bucket) {
        return;
    }
    let Some(handle) = state.material.clone() else {
        return;
    };
    if let Some(mat) = materials.get_mut(&handle) {
        mat.extension.facade = Vec4::new(
            day_night.night_factor,
            if facade_enabled { 1.0 } else { 0.0 },
            0.0,
            0.0,
        );
    }
    state.applied_facade_bucket = Some(bucket);
}

/// Quantization steps for `RevealState` so a merely-jittering cursor or
/// eased-strength value doesn't dirty (and re-upload) the shared buildings
/// material's uniform buffer every single frame — same no-churn discipline
/// as `quantize_night_factor` above. ~0.5m spatially is well under a visible
/// dither-cell width at any zoom this effect is active at; 1/64 on strength
/// is finer than the ease curve's own visible steps.
const REVEAL_POSITION_QUANTUM_M: f32 = 0.5;
const REVEAL_STRENGTH_BUCKETS: f32 = 64.0;

pub(crate) fn quantize_reveal_position(v: f32) -> i32 {
    (v / REVEAL_POSITION_QUANTUM_M).round() as i32
}

pub(crate) fn quantize_reveal_strength(v: f32) -> i32 {
    (v * REVEAL_STRENGTH_BUCKETS).round() as i32
}

/// One quantized `RevealState` snapshot — the shared change-detection bucket
/// both `apply_reveal_system` (buildings material) and `outline.rs`'s
/// `apply_reveal_to_outline_system` (inverted-hull material, issue #141) use
/// to skip redundant uniform re-uploads.
pub(crate) fn reveal_bucket(reveal_state: &RevealState) -> (i32, i32, i32, i32, i32) {
    (
        quantize_reveal_position(reveal_state.center.x),
        quantize_reveal_position(reveal_state.center.y),
        quantize_reveal_position(reveal_state.inner),
        quantize_reveal_position(reveal_state.outer),
        quantize_reveal_strength(reveal_state.strength),
    )
}

/// Copies `mf_state::RevealState` into the shared buildings material's
/// `RevealExtension` uniform (issue #18) — this is the system that lets
/// `mf-game`'s `reveal_input.rs` (which owns *where* the hole is) actually
/// reach the shader (which owns *drawing* the hole). Runs unconditionally
/// (no quality-tier gate): the reveal effect is meant to work identically on
/// every tier, "potato" included.
fn apply_reveal_system(
    reveal_state: Res<RevealState>,
    mut state: ResMut<BuildingsState>,
    mut materials: ResMut<Assets<BuildingMaterial>>,
) {
    let Some(handle) = state.material.clone() else {
        return;
    };
    let bucket = reveal_bucket(&reveal_state);
    if state.applied_reveal_bucket == Some(bucket) {
        return;
    }
    if let Some(mat) = materials.get_mut(&handle) {
        mat.extension.reveal = Vec4::new(
            reveal_state.center.x,
            reveal_state.center.y,
            reveal_state.inner,
            reveal_state.outer,
        );
        mat.extension.params = Vec4::new(reveal_state.strength, 0.0, 0.0, 0.0);
    }
    state.applied_reveal_bucket = Some(bucket);
}

fn apply_cloud_shadow_to_buildings_system(
    shadows: Res<CloudShadowParams>,
    state: Res<BuildingsState>,
    mut materials: ResMut<Assets<BuildingMaterial>>,
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
    use super::*;

    /// Concave hexagon: 2x2 square with a 1x1 corner notch removed
    /// (area = 3), same shape as `mesh_utils`'s ear-clip `l_shape` test,
    /// reused here to check the per-building wire-type path end to end.
    fn l_shape_footprint(height_dm: u16, min_height_dm: u16) -> BuildingFootprint {
        BuildingFootprint {
            height_dm,
            min_height_dm,
            verts: vec![
                [0.0, 0.0],
                [2.0, 0.0],
                [2.0, 1.0],
                [1.0, 1.0],
                [1.0, 2.0],
                [0.0, 2.0],
            ],
        }
    }

    #[test]
    fn emit_building_prism_l_shape_wall_and_cap_counts() {
        let building = l_shape_footprint(300, 0);
        let mut buf = MeshBuffers::new();
        let white = Color::WHITE;
        let (v, i) = emit_building_prism(
            &mut buf, &building, 0.0, 0.0, 30.0, white, white, white, white, white,
        );
        let n = building.verts.len();
        // One wall quad per ring edge (4 verts / 6 indices each) plus one
        // roof-cap triangle per `n - 2` (3 verts / 3 indices each), the same
        // invariant `ear_clip_indices` guarantees in mesh_utils.rs. No
        // bottom cap since base_offset=0.
        assert_eq!(n, 6, "fixture should be the 6-vertex L-shape");
        assert_eq!(v, n * 4 + (n - 2) * 3);
        assert_eq!(i, n * 6 + (n - 2) * 3);
        assert_eq!(buf.vertex_count(), v);
        assert_eq!(buf.index_count(), i);
    }

    #[test]
    fn emit_building_prism_with_base_offset_emits_bottom_cap() {
        // Same L-shape, but as a stacked part with a nonzero base_offset (a
        // tower part set back on top of a podium): must gain a bottom cap
        // on top of the walls + roof cap the base_offset=0 case above has.
        let building = l_shape_footprint(300, 80);
        let base_offset = building.min_height_dm as f32 / 10.0;
        let mut buf = MeshBuffers::new();
        let white = Color::WHITE;
        let (v, i) = emit_building_prism(
            &mut buf,
            &building,
            0.0,
            base_offset,
            30.0,
            white,
            white,
            white,
            white,
            white,
        );
        let n = building.verts.len();
        assert_eq!(v, n * 4 + (n - 2) * 3 + (n - 2) * 3);
        assert_eq!(i, n * 6 + (n - 2) * 3 + (n - 2) * 3);
        assert_eq!(buf.vertex_count(), v);
        assert_eq!(buf.index_count(), i);
    }

    #[test]
    fn resolve_part_extent_skips_unknown_height_with_nonzero_min() {
        // height_dm=0 (unknown) with min_height_dm>0: no density formula
        // exists for an elevated segment's height, so this must be skipped
        // rather than guessed.
        let bd = l_shape_footprint(0, 80);
        let result = resolve_part_extent(&bd, (0, 0), || (0.0, 0.0));
        assert_eq!(result, Err(PartSkipReason::UnknownHeightWithMin));
    }

    #[test]
    fn resolve_part_extent_skips_min_at_or_above_height() {
        // min_height_dm (30.0m) >= height_dm (25.0m): degenerate/corrupt
        // wire data, no positive-thickness prism to draw.
        let bd = l_shape_footprint(250, 300);
        let result = resolve_part_extent(&bd, (0, 0), || (0.0, 0.0));
        assert_eq!(result, Err(PartSkipReason::MinAtOrAboveHeight));
    }

    #[test]
    fn resolve_part_extent_min_exactly_at_height_is_also_skipped() {
        // Boundary case: min == height exactly (not just >) must still be
        // treated as degenerate, since a zero-thickness prism is as
        // meaningless as a negative one.
        let bd = l_shape_footprint(250, 250);
        let result = resolve_part_extent(&bd, (0, 0), || (0.0, 0.0));
        assert_eq!(result, Err(PartSkipReason::MinAtOrAboveHeight));
    }

    #[test]
    fn resolve_part_extent_uses_explicit_height_and_min_verbatim() {
        let bd = l_shape_footprint(300, 80);
        let (min_m, height) = resolve_part_extent(&bd, (0, 0), || {
            panic!("land_use_at must not be called when height_dm > 0")
        })
        .expect("should resolve");
        assert_eq!(min_m, 8.0);
        assert_eq!(height, 30.0);
    }

    #[test]
    fn resolve_part_extent_falls_back_to_density_formula_only_when_min_is_zero() {
        // height_dm=0 AND min_height_dm=0: the ground-based fallback case,
        // the only one allowed to invoke the density formula.
        let bd = l_shape_footprint(0, 0);
        let (min_m, height) =
            resolve_part_extent(&bd, (1, 2), || (10.0, 20.0)).expect("should resolve");
        assert_eq!(min_m, 0.0);
        assert!((FOOTPRINT_MIN_HEIGHT..=FOOTPRINT_MAX_HEIGHT).contains(&height));
    }

    #[test]
    fn emit_building_prism_degenerate_ring_is_a_noop() {
        let building = BuildingFootprint {
            height_dm: 100,
            min_height_dm: 0,
            verts: vec![[0.0, 0.0], [1.0, 0.0]],
        };
        let mut buf = MeshBuffers::new();
        let white = Color::WHITE;
        let (v, i) = emit_building_prism(
            &mut buf, &building, 0.0, 0.0, 10.0, white, white, white, white, white,
        );
        assert_eq!((v, i), (0, 0));
        assert!(buf.is_empty());
    }

    #[test]
    fn rebuild_key_forces_rebuild_when_buildings_arrive_after_fields() {
        // Reproduces the "StaticBuildings lands after the first
        // Fields-triggered build" race the rebuild key exists to catch:
        // `fields.version` alone would miss this since it never changes.
        let mut city = CurrentCity::default();
        let key_before = BuildingsState::rebuild_key(&city, 5);
        assert_eq!(key_before, (5, None));

        city.apply_buildings(mf_protocol::StaticBuildings {
            buildings: vec![l_shape_footprint(0, 0)],
        });
        let key_after = BuildingsState::rebuild_key(&city, 5);
        assert_ne!(
            key_before, key_after,
            "buildings arriving with unchanged fields.version must still change the rebuild key"
        );
    }

    #[test]
    fn rebuild_key_ignores_an_empty_buildings_list() {
        // An explicitly-empty `StaticBuildings` (valid but pointless) must
        // not be treated as "real footprint data present" — the real-
        // footprint branch itself guards on non-empty (`real_footprints`),
        // so the rebuild key must agree or it would force a no-op rebuild
        // forever whenever `fields.version` is otherwise stable.
        let mut city = CurrentCity::default();
        city.apply_buildings(mf_protocol::StaticBuildings { buildings: vec![] });
        let key = BuildingsState::rebuild_key(&city, 5);
        assert_eq!(key, (5, None));
    }

    #[test]
    fn duplicate_part_skips_drops_shorter_coincident_twin() {
        // The literal issue-#141 shape: two byte-identical rings, 472 dm-ish
        // heights apart, both ground based. The shorter must be skipped.
        let parts = vec![l_shape_footprint(4660, 0), l_shape_footprint(4720, 0)];
        assert_eq!(duplicate_part_skips(&parts), vec![true, false]);
    }

    #[test]
    fn duplicate_part_skips_keeps_disjoint_vertical_stack() {
        // Same footprint but stacked extents (tower body 0..4170, spire
        // 4200..5000): legitimate building:part stacking, keep both.
        let parts = vec![l_shape_footprint(4170, 0), l_shape_footprint(5000, 4200)];
        assert_eq!(duplicate_part_skips(&parts), vec![false, false]);
    }

    #[test]
    fn duplicate_part_skips_treats_unknown_height_twins_as_coincident() {
        // Two unknown-height twins resolve to the same density-formula
        // height later — exactly coincident, so one must be skipped.
        let parts = vec![l_shape_footprint(0, 0), l_shape_footprint(0, 0)];
        assert_eq!(
            duplicate_part_skips(&parts).iter().filter(|s| **s).count(),
            1
        );
    }

    #[test]
    fn duplicate_part_skips_ignores_distinct_footprints() {
        let mut other = l_shape_footprint(4660, 0);
        other.verts[0] = [0.5, 0.0];
        let parts = vec![l_shape_footprint(4660, 0), other];
        assert_eq!(duplicate_part_skips(&parts), vec![false, false]);
    }

    #[test]
    fn duplicate_part_skips_partial_overlap_is_still_a_duplicate() {
        // Overlapping (not nested) extents on an identical ring still means
        // coincident walls over the shared band — skip the shorter.
        let parts = vec![l_shape_footprint(3000, 0), l_shape_footprint(4720, 890)];
        assert_eq!(duplicate_part_skips(&parts), vec![true, false]);
    }
}
