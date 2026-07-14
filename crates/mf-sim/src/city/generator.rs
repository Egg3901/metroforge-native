//! Procedural city generation. Port of `sim/src/core/city/generator.ts`.
//!
//! Tensor-field street networks (Chen et al. 2008). Order matters:
//! terrain -> river -> CBD/subcenters -> population -> parks -> tensor field ->
//! arterial streamlines (with bridges) -> local streamlines. Population comes
//! BEFORE streets; street density follows people.
//!
//! Determinism: all randomness draws from the seeded [`Rng`]. Idiomatic Rust,
//! NOT bit-parity with the TS f64 math (the new Rust baseline). The OSM
//! real-city path (`osm` bundle) is NOT ported here; P2 generates the fully
//! PROCEDURAL path. Real presets still generate procedurally when no OSM bundle
//! is supplied (which is always, in P2). See `PORT.md`.

use std::collections::HashMap;
use std::f64::consts::PI;

use crate::city::names::{district_name, unique_names};
use crate::city::presets::CityPreset;
use crate::city::streamlines::{trace_streamlines, Separation, TraceOptions};
use crate::city::tensor::{BoundarySample, GridBasis, TensorField};
use crate::fields::{cell_center, cell_index_at, create_field_grid};
use crate::geometry::{clamp, make_polyline, vec, Noise2D, Vec2};
use crate::rng::Rng;
use crate::types::{Difficulty, District, FieldGrid, RoadClass, RoadEdge};

/// The generated city bundle. Mirrors `GeneratedCity`. For real (OSM) cities
/// the `osm_*` channels + `labels` / `poi_anchors` carry the baked static map
/// data; they are all empty/`None` for procedural cities.
pub struct GeneratedCity {
    /// Populated scalar field grid.
    pub fields: FieldGrid,
    /// Road network.
    pub roads: Vec<RoadEdge>,
    /// Districts (named).
    pub districts: Vec<District>,
    /// Central business district anchor.
    pub cbd: Vec2,
    /// High-res OSM water mask (1 = water), for crisp coastline rendering.
    pub water_mask_hi: Option<Vec<u8>>,
    /// High-res OSM park mask.
    pub park_mask_hi: Option<Vec<u8>>,
    /// High-res OSM building-footprint mask.
    pub building_mask_hi: Option<Vec<u8>>,
    /// Mask side length (`mask_res * mask_res`).
    pub mask_res: Option<u32>,
    /// Real-elevation heightfield (meters, `elev_res^2`).
    pub elevation_hi: Option<Vec<i16>>,
    /// Elevation side length.
    pub elev_res: Option<u32>,
    /// Real OSM place-name labels.
    pub labels: Vec<crate::types::MapLabel>,
    /// Named POI anchors from the bundle.
    pub poi_anchors: Vec<crate::types::PoiAnchor>,
}

/// Signed smallest angle from `b` to `a`. Mirrors `angleDelta`.
fn angle_delta(a: f64, b: f64) -> f64 {
    let mut d = a - b;
    while d > PI {
        d -= PI * 2.0;
    }
    while d < -PI {
        d += PI * 2.0;
    }
    d
}

/// Drop collinear-ish points to keep polylines lean. Mirrors `decimate`.
fn decimate(pts: &[Vec2]) -> Vec<Vec2> {
    if pts.len() <= 4 {
        return pts.iter().map(|p| vec(p.x.round(), p.y.round())).collect();
    }
    let mut out: Vec<Vec2> = vec![pts[0]];
    for i in 1..pts.len() - 1 {
        let a = *out.last().unwrap();
        let b = pts[i];
        let c = pts[i + 1];
        let ang_ab = (b.y - a.y).atan2(b.x - a.x);
        let ang_bc = (c.y - b.y).atan2(c.x - b.x);
        let turn = angle_delta(ang_bc, ang_ab).abs();
        let dist = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        if turn > 0.06 || dist > 220.0 {
            out.push(b);
        }
    }
    out.push(pts[pts.len() - 1]);
    out.iter().map(|p| vec(p.x.round(), p.y.round())).collect()
}

#[inline]
fn is_water_at(fields: &FieldGrid, p: Vec2) -> bool {
    fields.water[cell_index_at(fields, p)] == 1
}

/// Population+jobs density (relative to mean) at `p`; -1 outside/on water/park.
fn density_at(fields: &FieldGrid, half: f64, mean_cell_pop: f64, p: Vec2) -> f64 {
    if p.x.abs() > half || p.y.abs() > half {
        return -1.0;
    }
    let i = cell_index_at(fields, p);
    if fields.water[i] == 1 || fields.parks[i] == 1 {
        return -1.0;
    }
    (fields.population[i] as f64 + fields.jobs[i] as f64) / mean_cell_pop
}

/// Spatial hash over polyline segments for nearest-segment snap queries.
/// Mirrors `SegmentGrid`.
struct SegmentGrid<'a> {
    cell: f64,
    lines: &'a [Vec<Vec2>],
    map: HashMap<i64, Vec<i64>>, // cellKey -> encoded refs (line*1e6 + seg)
}

impl<'a> SegmentGrid<'a> {
    fn new(lines: &'a [Vec<Vec2>], cell: f64) -> Self {
        let mut g = SegmentGrid {
            cell,
            lines,
            map: HashMap::new(),
        };
        for (li, line) in lines.iter().enumerate() {
            for si in 0..line.len().saturating_sub(1) {
                g.rasterize(line[si], line[si + 1], li as i64 * 1_000_000 + si as i64);
            }
        }
        g
    }

    #[inline]
    fn key(cx: i64, cy: i64) -> i64 {
        cx * 73_856_093 + cy * 19_349_663
    }

    fn rasterize(&mut self, a: Vec2, b: Vec2, r#ref: i64) {
        let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let steps = ((len / self.cell * 2.0).ceil() as i64).max(1);
        let mut last_key: Option<i64> = None;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let cx = ((a.x + (b.x - a.x) * t) / self.cell).floor() as i64;
            let cy = ((a.y + (b.y - a.y) * t) / self.cell).floor() as i64;
            let k = Self::key(cx, cy);
            if Some(k) == last_key {
                continue;
            }
            last_key = Some(k);
            self.map.entry(k).or_default().push(r#ref);
        }
    }

    /// Nearest projected point on any segment within `max_dist`. Mirrors
    /// `nearest`.
    fn nearest(&self, p: Vec2, max_dist: f64) -> Option<Vec2> {
        let cx = (p.x / self.cell).floor() as i64;
        let cy = (p.y / self.cell).floor() as i64;
        let mut refs: Vec<i64> = Vec::new();
        for oy in -2..=2 {
            for ox in -2..=2 {
                if let Some(arr) = self.map.get(&Self::key(cx + ox, cy + oy)) {
                    refs.extend_from_slice(arr);
                }
            }
        }
        if refs.is_empty() {
            return None;
        }
        refs.sort_unstable();
        let mut best = max_dist * max_dist;
        let mut best_p: Option<Vec2> = None;
        let mut prev: Option<i64> = None;
        for r#ref in refs {
            if Some(r#ref) == prev {
                continue;
            }
            prev = Some(r#ref);
            let line = &self.lines[(r#ref / 1_000_000) as usize];
            let si = (r#ref % 1_000_000) as usize;
            let a = line[si];
            let b = line[si + 1];
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let l2 = {
                let v = dx * dx + dy * dy;
                if v == 0.0 {
                    1.0
                } else {
                    v
                }
            };
            let t = clamp(((p.x - a.x) * dx + (p.y - a.y) * dy) / l2, 0.0, 1.0);
            let qx = a.x + dx * t;
            let qy = a.y + dy * t;
            let d2 = (qx - p.x).powi(2) + (qy - p.y).powi(2);
            if d2 < best {
                best = d2;
                best_p = Some(vec(qx, qy));
            }
        }
        best_p
    }
}

/// Project `p` onto the nearest segment across `lines` within `max_dist`,
/// skipping `skip` (by index). Mirrors the inner `projectOnto`.
fn project_onto(lines: &[Vec<Vec2>], p: Vec2, max_dist: f64, skip: Option<usize>) -> Option<Vec2> {
    let mut best = max_dist * max_dist;
    let mut best_p: Option<Vec2> = None;
    for (li, line) in lines.iter().enumerate() {
        if Some(li) == skip {
            continue;
        }
        for i in 0..line.len().saturating_sub(1) {
            let a = line[i];
            let b = line[i + 1];
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let l2 = {
                let v = dx * dx + dy * dy;
                if v == 0.0 {
                    1.0
                } else {
                    v
                }
            };
            let t = clamp(((p.x - a.x) * dx + (p.y - a.y) * dy) / l2, 0.0, 1.0);
            let qx = a.x + dx * t;
            let qy = a.y + dy * t;
            let d2 = (qx - p.x).powi(2) + (qy - p.y).powi(2);
            if d2 < best {
                best = d2;
                best_p = Some(vec(qx, qy));
            }
        }
    }
    best_p
}

/// Generate a city. Mirrors `generateCity`. When `osm` is `Some`, real
/// land/water/parks + the real road network replace procgen (the shared
/// population / land-value / district stages still run on top); when `None`,
/// the fully procedural path runs.
pub fn generate_city(
    seed: u32,
    difficulty: Difficulty,
    world_size: Option<f64>,
    preset: CityPreset,
    osm: Option<&crate::city::osm::OsmCityData>,
) -> GeneratedCity {
    let mut rng = Rng::from_seed(seed);
    let terrain_noise = Noise2D::new(&mut rng);
    let detail_noise = Noise2D::new(&mut rng);
    // Real cities size the world from the bundle; procedural uses the option.
    let world_size_opt = osm.map(|o| o.world_size).or(world_size);
    let mut fields = create_field_grid(world_size_opt);
    let world_size = fields.w as f64 * fields.cell_size;
    let half = world_size / 2.0;
    let w = fields.w as usize;
    let n = (fields.w * fields.h) as usize;

    // ── Terrain + water ──
    let water = preset.water;
    let water_angle = match water.coast_angle_deg {
        Some(deg) => deg * PI / 180.0,
        None => rng.range(0.0, PI * 2.0),
    };
    let water_dir = vec(water_angle.cos(), water_angle.sin());
    let water_offset = water.coast_inset * half;

    // Decoded OSM channels (real cities only), threaded onto GeneratedCity.
    let osm_channels = if let Some(osm) = osm {
        // real land/water/parks + shore-faded relief straight from the masks
        Some(crate::city::osm::apply_osm_terrain(
            &mut fields,
            osm,
            &terrain_noise,
        ))
    } else {
        for cy in 0..fields.h as usize {
            for cx in 0..w {
                let i = cy * w + cx;
                let p = cell_center(&fields, i);
                let nx = p.x / world_size;
                let ny = p.y / world_size;
                let mut elev = terrain_noise.fbm(nx * 4.0 + 10.0, ny * 4.0 + 10.0, 4, 2.0, 0.5);
                if !water.coast {
                    // landlocked: keep the noise but never dip below the waterline
                    fields.terrain[i] = clamp(0.35 + elev * 0.5, 0.0, 1.0) as f32;
                    fields.water[i] = 0;
                    continue;
                }
                let coast_dist = p.x * water_dir.x + p.y * water_dir.y - water_offset;
                if coast_dist > 0.0 {
                    elev -= (coast_dist / half) * 0.9;
                }
                fields.terrain[i] = clamp(elev, 0.0, 1.0) as f32;
                fields.water[i] = if elev < 0.22 { 1 } else { 0 };
            }
        }
        None
    };

    // ── River: meanders from an inland edge downhill to the sea (procgen only) ──
    if water.river && osm.is_none() {
        let start_angle = water_angle + PI + rng.range(-0.5, 0.5);
        let mut px = start_angle.cos() * half * 0.95;
        let mut py = start_angle.sin() * half * 0.95;
        let mut dir_angle = (-py).atan2(-px);
        let meander = rng.range(2.0, 5.0);
        let mut reached_sea = false;
        let mut step = 0i32;
        while step < 400 {
            let ci = cell_index_at(&fields, vec(px, py));
            let cx0 = (ci % w) as i64;
            let cy0 = (ci / w) as i64;
            for oy in -1i64..=1 {
                for ox in -1i64..=1 {
                    if ox.abs() + oy.abs() > 1 && rng.next_f64() >= 0.4 {
                        continue;
                    }
                    let nx = cx0 + ox;
                    let ny = cy0 + oy;
                    if nx >= 0 && ny >= 0 && nx < fields.w as i64 && ny < fields.h as i64 {
                        let idx = (ny * fields.w as i64 + nx) as usize;
                        fields.water[idx] = 1;
                        fields.terrain[idx] = fields.terrain[idx].min(0.2);
                    }
                }
            }
            if fields.water[ci] == 1 && step > 30 {
                let coast_dist = px * water_dir.x + py * water_dir.y - water_offset;
                if coast_dist > -600.0 {
                    reached_sea = true;
                    break;
                }
            }
            let to_coast = water_dir.y.atan2(water_dir.x);
            let wiggle = (step as f64 / 14.0).sin() * 0.5 * (meander + step as f64 / 40.0).sin();
            dir_angle +=
                angle_delta(to_coast, dir_angle) * 0.035 + wiggle * 0.14 + rng.range(-0.08, 0.08);
            px += dir_angle.cos() * 95.0;
            py += dir_angle.sin() * 95.0;
            if px.abs() > half || py.abs() > half {
                reached_sea = true;
                break;
            }
            step += 1;
        }
        if !reached_sea {
            // rivers should end somewhere: stamp a terminal lake
            let lake_r = rng.range(350.0, 600.0);
            for i in 0..n {
                let c = cell_center(&fields, i);
                let d = ((c.x - px).powi(2) + (c.y - py).powi(2)).sqrt();
                if d < lake_r
                    * (0.75 + 0.25 * detail_noise.at(c.x / 900.0 + 77.0, c.y / 900.0 + 77.0))
                {
                    fields.water[i] = 1;
                    fields.terrain[i] = fields.terrain[i].min(0.18);
                }
            }
        }
    }

    // ── CBD: on land, biased toward the water (port cities) ──
    let mut cbd = vec(0.0, 0.0);
    {
        let mut best = f64::NEG_INFINITY;
        for _ in 0..60 {
            let cand = vec(
                rng.range(-half * 0.35, half * 0.35),
                rng.range(-half * 0.35, half * 0.35),
            );
            if is_water_at(&fields, cand) {
                continue;
            }
            let coast_dist = (cand.x * water_dir.x + cand.y * water_dir.y - water_offset).abs();
            let score = -coast_dist / half - (cand.x.powi(2) + cand.y.powi(2)).sqrt() / half
                + rng.range(0.0, 0.3);
            if score > best {
                best = score;
                cbd = cand;
            }
        }
    }

    // ── Employment subcenters (edge-city anchors) ──
    // NB: TS re-evaluates `rng.int(3, 5)` each loop test; replicated here.
    let mut subcenters: Vec<Vec2> = Vec::new();
    {
        let mut k = 0i64;
        while k < rng.int(3, 5) {
            for _ in 0..20 {
                let ang = rng.range(0.0, PI * 2.0);
                let cand = vec(
                    cbd.x + ang.cos() * rng.range(2000.0, 4200.0),
                    cbd.y + ang.sin() * rng.range(2000.0, 4200.0),
                );
                if cand.x.abs() > half * 0.9
                    || cand.y.abs() > half * 0.9
                    || is_water_at(&fields, cand)
                {
                    continue;
                }
                if subcenters
                    .iter()
                    .all(|s| ((s.x - cand.x).powi(2) + (s.y - cand.y).powi(2)).sqrt() > 1800.0)
                {
                    subcenters.push(cand);
                    break;
                }
            }
            k += 1;
        }
    }

    // ── Population & jobs (BEFORE streets) ──
    let target = match difficulty {
        Difficulty::Easy => 220000.0,
        Difficulty::Normal => 160000.0,
        Difficulty::Hard => 110000.0,
    };
    let mut raw_pop = vec![0f32; n];
    let mut raw_jobs = vec![0f32; n];
    let mut raw_pop_sum = 0f64;
    let mut raw_jobs_sum = 0f64;
    let sp = preset.sprawl;
    for i in 0..n {
        if fields.water[i] == 1 {
            continue;
        }
        let c = cell_center(&fields, i);
        let d_cbd = ((c.x - cbd.x).powi(2) + (c.y - cbd.y).powi(2)).sqrt();
        let noise = detail_noise.fbm(c.x / 3000.0 + 50.0, c.y / 3000.0 + 50.0, 3, 2.0, 0.5);
        let mut pop = (-d_cbd / (2600.0 * sp)).exp();
        for s in &subcenters {
            let d_s = ((c.x - s.x).powi(2) + (c.y - s.y).powi(2)).sqrt();
            pop += 0.45 * (-d_s / (1400.0 * sp)).exp();
        }
        pop *= 0.45 + noise;
        raw_pop[i] = pop as f32;
        raw_pop_sum += pop;
        let mut jobs = (-d_cbd / (1100.0 * sp)).exp() * 3.0;
        for s in &subcenters {
            let d_s = ((c.x - s.x).powi(2) + (c.y - s.y).powi(2)).sqrt();
            jobs += (-d_s / (800.0 * sp)).exp() * 0.8;
        }
        jobs *= 0.6 + noise;
        raw_jobs[i] = jobs as f32;
        raw_jobs_sum += jobs;
    }
    let jobs_target = target * 0.45;
    for i in 0..n {
        fields.population[i] = ((raw_pop[i] as f64 / raw_pop_sum) * target) as f32;
        fields.jobs[i] = ((raw_jobs[i] as f64 / raw_jobs_sum) * jobs_target) as f32;
    }

    // ── Parks: real parks (OSM) already stamped in apply_osm_terrain; else
    //    noise pockets + signature parks. Either way, parks displace residents. ──
    if osm.is_none() {
        for i in 0..n {
            if fields.water[i] == 1 {
                continue;
            }
            let c = cell_center(&fields, i);
            let nz = detail_noise.fbm(c.x / 1400.0 + 300.0, c.y / 1400.0 + 300.0, 3, 2.0, 0.5);
            let d_cbd = ((c.x - cbd.x).powi(2) + (c.y - cbd.y).powi(2)).sqrt();
            if nz > 0.66 && d_cbd > 700.0 {
                fields.parks[i] = 1;
            }
        }
        let big_parks = rng.int(1, 2);
        for _ in 0..big_parks {
            let ang = rng.range(0.0, PI * 2.0);
            let cx0 = cbd.x + ang.cos() * rng.range(1200.0, 2400.0);
            let cy0 = cbd.y + ang.sin() * rng.range(1200.0, 2400.0);
            let pw = rng.range(500.0, 900.0);
            let ph = rng.range(350.0, 650.0);
            for i in 0..n {
                let c = cell_center(&fields, i);
                if (c.x - cx0).abs() < pw / 2.0
                    && (c.y - cy0).abs() < ph / 2.0
                    && fields.water[i] == 0
                {
                    fields.parks[i] = 1;
                }
            }
        }
    }
    for i in 0..n {
        if fields.parks[i] == 1 {
            fields.population[i] = 0.0;
            fields.jobs[i] = 0.0;
        }
    }

    let mean_cell_pop = target / n as f64;

    // ── Roads ──
    let mut roads: Vec<RoadEdge> = Vec::new();
    let mut road_id = 1u32;
    if let Some(osm) = osm {
        // real street network straight from the OSM bundle (no tensor field /
        // streamlines / RNG draws — the geometry is static input)
        for r in &osm.roads {
            if r.pts.len() < 4 {
                continue;
            }
            let mut pl: Vec<Vec2> = Vec::with_capacity(r.pts.len() / 2);
            let mut i = 0;
            while i + 1 < r.pts.len() {
                pl.push(vec(r.pts[i], r.pts[i + 1]));
                i += 2;
            }
            roads.push(RoadEdge {
                id: road_id,
                cls: crate::city::osm::road_class_of(&r.cls),
                polyline: make_polyline(pl),
                grade_level: if r.g != 0 { Some(r.g) } else { None },
                is_bridge: if r.br { Some(true) } else { None },
                is_tunnel: if r.tn { Some(true) } else { None },
            });
            road_id += 1;
        }
    } else {
        // ── Tensor field ──
        let base_angle = preset.grid.angle_deg * PI / 180.0;
        let mut grids: Vec<GridBasis> = Vec::new();
        let mut grid_seeds: Vec<Vec2> = subcenters.clone();
        for _ in 0..6 {
            grid_seeds.push(vec(
                rng.range(-half * 0.8, half * 0.8),
                rng.range(-half * 0.8, half * 0.8),
            ));
        }
        for gcenter in &grid_seeds {
            let raw = detail_noise.at(gcenter.x / 4200.0 + 200.0, gcenter.y / 4200.0 + 200.0) * PI;
            let theta = if preset.grid.rigid {
                base_angle
            } else {
                base_angle + (raw / (PI / 12.0)).round() * (PI / 12.0)
            };
            grids.push(GridBasis {
                center: *gcenter,
                theta,
                sigma: rng.range(1600.0, 2600.0),
                weight: preset.grid.weight,
            });
        }
        // boundary samples: shoreline cells with tangent from the water gradient
        let mut boundaries: Vec<BoundarySample> = Vec::new();
        for cy in 1..fields.h as usize - 1 {
            for cx in 1..w - 1 {
                let i = cy * w + cx;
                if fields.water[i] == 1 {
                    continue;
                }
                let w_r = fields.water[cy * w + cx + 1] as i32;
                let w_l = fields.water[cy * w + cx - 1] as i32;
                let w_d = fields.water[(cy + 1) * w + cx] as i32;
                let w_u = fields.water[(cy - 1) * w + cx] as i32;
                if w_r + w_l + w_d + w_u == 0 {
                    continue;
                }
                if (cx + cy) % 2 != 0 {
                    continue;
                }
                let gx = (w_r - w_l) as f64;
                let gy = (w_d - w_u) as f64;
                boundaries.push(BoundarySample {
                    pos: cell_center(&fields, i),
                    theta: gy.atan2(gx) + PI / 2.0,
                });
            }
        }

        let global_grid = if preset.grid.rigid {
            Some((base_angle, 3.2))
        } else {
            None
        };
        let field = TensorField {
            grids,
            global_grid,
            radial_center: cbd,
            radial_weight: preset.radial_weight,
            radial_sigma: 2600.0,
            boundaries,
            boundary_sigma: 550.0,
            boundary_weight: 1.6,
            noise: Box::new(|x: f64, y: f64| {
                detail_noise.at(x / 5200.0 + 400.0, y / 5200.0 + 400.0)
            }),
            noise_weight: preset.grid.noise_weight,
        };

        // ── Roads (procedural streamlines) ──
        // Arterials
        let mut arterial_seeds: Vec<Vec2> = vec![cbd];
        arterial_seeds.extend_from_slice(&subcenters);
        for _ in 0..18 {
            let cand = vec(
                rng.range(-half * 0.85, half * 0.85),
                rng.range(-half * 0.85, half * 0.85),
            );
            if density_at(&fields, half, mean_cell_pop, cand) > 0.4 {
                arterial_seeds.push(cand);
            }
        }
        let arterials = {
            let fields_ref = &fields;
            let cbd_c = cbd;
            let opts = TraceOptions {
                separation: Separation::Const(620.0),
                in_domain: Box::new(move |p: Vec2| {
                    p.x.abs() < half * 0.97
                        && p.y.abs() < half * 0.97
                        && (density_at(fields_ref, half, mean_cell_pop, p) > 0.12
                            || ((p.x - cbd_c.x).powi(2) + (p.y - cbd_c.y).powi(2)).sqrt() < 2800.0)
                }),
                bridge_max_steps: 9,
                blocked: Box::new(move |p: Vec2| is_water_at(fields_ref, p)),
                max_length: 11000.0,
                min_length: 900.0,
                seeds: arterial_seeds,
                snap_targets: Vec::new(),
                spawn_seeds: true,
                eigen_dirs: vec![0, 1],
            };
            trace_streamlines(&field, rng.fork(11), opts)
        };
        for line in &arterials {
            roads.push(RoadEdge {
                id: road_id,
                cls: RoadClass::Arterial,
                polyline: make_polyline(decimate(line)),
                grade_level: None,
                is_bridge: None,
                is_tunnel: None,
            });
            road_id += 1;
        }

        // Locals
        let mut local_seeds: Vec<Vec2> = vec![cbd];
        local_seeds.extend_from_slice(&subcenters);
        for _ in 0..140 {
            let cand = vec(
                rng.range(-half * 0.9, half * 0.9),
                rng.range(-half * 0.9, half * 0.9),
            );
            if density_at(&fields, half, mean_cell_pop, cand) > 0.35 {
                local_seeds.push(cand);
            }
        }
        let mut arterial_samples: Vec<Vec2> = Vec::new();
        for line in &arterials {
            arterial_samples.extend_from_slice(line);
        }
        let mut locals = {
            let fields_ref = &fields;
            let opts = TraceOptions {
                separation: Separation::Varying(Box::new(move |p: Vec2| {
                    let d = density_at(fields_ref, half, mean_cell_pop, p);
                    if d > 2.5 {
                        70.0
                    } else if d > 1.2 {
                        95.0
                    } else {
                        130.0
                    }
                })),
                in_domain: Box::new(move |p: Vec2| {
                    density_at(fields_ref, half, mean_cell_pop, p) > 0.22
                }),
                bridge_max_steps: 0,
                blocked: Box::new(move |p: Vec2| is_water_at(fields_ref, p)),
                max_length: 2600.0,
                min_length: 330.0,
                seeds: local_seeds,
                snap_targets: arterial_samples,
                spawn_seeds: true,
                eigen_dirs: vec![0, 1],
            };
            trace_streamlines(&field, rng.fork(13), opts)
        };

        // Junction snap: pull each local street's dangling ends onto the nearest
        // arterial (or a nearby local) so streets actually meet.
        const ARTERIAL_SNAP: f64 = 150.0;
        const LOCAL_SNAP: f64 = 80.0;
        let arterial_grid = SegmentGrid::new(&arterials, ARTERIAL_SNAP);
        for li in 0..locals.len() {
            if locals[li].len() >= 2 {
                let last = locals[li].len() - 1;
                for end_idx in [0usize, last] {
                    let end = locals[li][end_idx];
                    let q = arterial_grid
                        .nearest(end, ARTERIAL_SNAP)
                        .or_else(|| project_onto(&locals, end, LOCAL_SNAP, Some(li)));
                    if let Some(q) = q {
                        if ((q.x - end.x).powi(2) + (q.y - end.y).powi(2)).sqrt() > 12.0
                            && !is_water_at(&fields, q)
                        {
                            if end_idx == 0 {
                                locals[li].insert(0, q);
                            } else {
                                locals[li].push(q);
                            }
                        }
                    }
                }
            }
            roads.push(RoadEdge {
                id: road_id,
                cls: RoadClass::Local,
                polyline: make_polyline(decimate(&locals[li])),
                grade_level: None,
                is_bridge: None,
                is_tunnel: None,
            });
            road_id += 1;
        }
    } // end procedural road generation

    // ── Land value + NIMBY ──
    for i in 0..n {
        if fields.water[i] == 1 {
            continue;
        }
        let c = cell_center(&fields, i);
        let d_cbd = ((c.x - cbd.x).powi(2) + (c.y - cbd.y).powi(2)).sqrt();
        let mut near_water = 0.0;
        let probe = 2i64;
        let cx = (i % w) as i64;
        let cy = (i / w) as i64;
        'outer: for oy in -probe..=probe {
            for ox in -probe..=probe {
                let nx2 = cx + ox;
                let ny2 = cy + oy;
                if nx2 < 0 || ny2 < 0 || nx2 >= fields.w as i64 || ny2 >= fields.h as i64 {
                    continue;
                }
                if fields.water[(ny2 * fields.w as i64 + nx2) as usize] == 1 {
                    near_water = 1.0;
                    break 'outer;
                }
            }
        }
        let lv = (-d_cbd / 3500.0).exp() * 1.2
            + near_water * 0.6
            + detail_noise.fbm(c.x / 2500.0 + 90.0, c.y / 2500.0 + 90.0, 3, 2.0, 0.5) * 0.5;
        fields.land_value[i] = lv as f32;
        let pop_norm = fields.population[i] as f64 / mean_cell_pop;
        fields.nimby[i] = if lv > 1.1 && pop_norm < 1.2 {
            clamp((lv - 1.0) * 55.0, 0.0, 90.0) as f32
        } else {
            0.0
        };
    }

    // ── Districts: 4x4-cell blocks ──
    let mut districts: Vec<District> = Vec::new();
    const BLOCK: usize = 4;
    let mut district_id = 0u32;
    let mut by = 0usize;
    while by < fields.h as usize {
        let mut bx = 0usize;
        while bx < w {
            let mut pop = 0.0;
            let mut jobs = 0.0;
            let mut lv_sum = 0.0;
            let mut land_cells = 0u32;
            let mut cell_indices: Vec<u32> = Vec::new();
            let mut wx = 0.0;
            let mut wy = 0.0;
            let mut w_sum = 0.0;
            let mut oy = 0usize;
            while oy < BLOCK && by + oy < fields.h as usize {
                let mut ox = 0usize;
                while ox < BLOCK && bx + ox < w {
                    let i = (by + oy) * w + (bx + ox);
                    cell_indices.push(i as u32);
                    let cp = fields.population[i] as f64;
                    let cj = fields.jobs[i] as f64;
                    pop += cp;
                    jobs += cj;
                    if fields.water[i] == 0 {
                        lv_sum += fields.land_value[i] as f64;
                        land_cells += 1;
                    }
                    let wt = cp + cj;
                    if wt > 0.0 {
                        let c = cell_center(&fields, i);
                        wx += c.x * wt;
                        wy += c.y * wt;
                        w_sum += wt;
                    }
                    ox += 1;
                }
                oy += 1;
            }
            if pop + jobs < 50.0 {
                bx += BLOCK;
                continue;
            }
            let centroid = if w_sum > 0.0 {
                vec(wx / w_sum, wy / w_sum)
            } else {
                cell_center(&fields, cell_indices[cell_indices.len() / 2] as usize)
            };
            districts.push(District {
                id: district_id,
                name: String::new(),
                centroid,
                cell_indices,
                population: pop,
                jobs,
                land_value: if land_cells > 0 {
                    lv_sum / land_cells as f64
                } else {
                    0.0
                },
                last_growth_delta: None,
            });
            district_id += 1;
            bx += BLOCK;
        }
        by += BLOCK;
    }

    // ── Name the neighborhoods (unique, seed-stable) ──
    let mut name_rng = rng.fork(0x0d15);
    let names = unique_names(&mut name_rng, districts.len(), district_name);
    for (i, d) in districts.iter_mut().enumerate() {
        d.name = names[i].clone();
    }

    let ch = osm_channels.unwrap_or_default();
    GeneratedCity {
        fields,
        roads,
        districts,
        cbd,
        water_mask_hi: ch.water_mask,
        park_mask_hi: ch.park_mask,
        building_mask_hi: ch.building_mask,
        mask_res: ch.mask_res,
        elevation_hi: ch.elevation,
        elev_res: ch.elev_res,
        labels: osm.map(|o| o.labels.clone()).unwrap_or_default(),
        poi_anchors: osm.map(|o| o.poi_anchors.clone()).unwrap_or_default(),
    }
}
