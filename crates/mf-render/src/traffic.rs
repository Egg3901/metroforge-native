//! `traffic.rs` — ambient street traffic: cheap instanced background cars
//! flowing along the road network, driven by the sim's congestion field.
//!
//! Art direction (BINDING §4): transit is the ONLY vivid color. Ambient cars
//! are therefore painted in DESATURATED muted tones (see [`MUTED_CAR_TONES`],
//! mirrored from `tools/blender/mf_bpy.py`'s `car_body_*`) so the player's
//! transit lines stay the only saturated thing on the slab. These are pure
//! background flavour — never route-colored.
//!
//! Cheapness / batching: three low-poly car variants (sedan / hatchback / van,
//! proportions mirrored from `tools/blender/gen_cars.py`) are built once as
//! three shared `Mesh` handles with per-vertex baked colors, drawn through one
//! shared vertex-color material. Every instance of a variant reuses that one
//! mesh + material, so the whole ambient fleet is at most three draw batches.
//! The instance pool is grow-only and reused (shown/hidden), never rebuilt per
//! frame.
//!
//! Tier gating (owner budget): High ~600, Medium ~250, Low ~80, Potato 0.
//!
//! Determinism: NO RNG. Each car's lane, starting offset, travel direction,
//! kerb side and variant are derived from an integer hash of its stable slot
//! index (a position hash), and motion is a continuous phase advanced by
//! delta-time. Congestion drives BOTH how many cars a road gets (lane weight
//! folds in the local density) and how fast they flow (denser roads slow the
//! cars, so congested arterials visibly bunch).

use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;

use mf_state::{CurrentCity, EffectiveKnobs, HeightAt, LatestTraffic, QualityTier};

use crate::RenderCacheStats;

/// Master knob: ambient traffic on at Medium+ by default, off below. Flip to
/// `false` to remove ambient cars entirely.
const AMBIENT_TRAFFIC: bool = true;

/// Muted, DESATURATED car body tones (sRGB) — mirrors `mf_bpy.py`
/// `car_body_a/b/c`. Deliberately low-chroma so transit stays the only vivid
/// color (art-direction §4).
const MUTED_CAR_TONES: [(f32, f32, f32); 3] = [
    (
        0xb8 as f32 / 255.0,
        0xba as f32 / 255.0,
        0xbd as f32 / 255.0,
    ), // cool grey
    (
        0xa9 as f32 / 255.0,
        0xa2 as f32 / 255.0,
        0x99 as f32 / 255.0,
    ), // warm taupe
    (
        0x8f as f32 / 255.0,
        0x96 as f32 / 255.0,
        0x9c as f32 / 255.0,
    ), // slate
];
/// Dark cabin/glass tone (sRGB) — mirrors `mf_bpy.py` `car_glass`.
const CAR_GLASS: (f32, f32, f32) = (
    0x33 as f32 / 255.0,
    0x38 as f32 / 255.0,
    0x40 as f32 / 255.0,
);

/// Small lift so cars sit on the road deck, not buried in the ground shader.
const CAR_LIFT: f32 = 0.35;
/// Base cruise speed (m/s) on a clear road.
const BASE_SPEED: f32 = 9.0;
/// Half the lane offset (m): opposing flows sit this far either side of the
/// polyline centre so they don't overlap.
const LANE_HALF: f32 = 2.4;

/// Per-tier ambient car cap.
fn tier_car_cap(tier: QualityTier) -> usize {
    match tier {
        QualityTier::High => 600,
        QualityTier::Medium => 250,
        QualityTier::Low => 80,
        QualityTier::Potato => 0,
    }
}

/// Integer hash (no RNG): stable per-index scramble for deterministic paths.
fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

pub struct MfTrafficPlugin;

impl Plugin for MfTrafficPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TrafficLanes>()
            .init_resource::<TrafficPool>()
            .add_systems(
                Update,
                build_traffic_lanes_system.in_set(crate::MfRenderSet::Statics),
            )
            .add_systems(
                Update,
                update_traffic_system.in_set(crate::MfRenderSet::Dynamic),
            );
    }
}

/// One drivable lane: a densified road polyline plus its cumulative arclength
/// table, in world (x, z) meters.
struct Lane {
    pts: Vec<Vec2>,
    cum: Vec<f32>,
    total_len: f32,
}

impl Lane {
    /// Position + forward unit vector at arclength `s` (wrapped into range).
    fn sample(&self, s: f32) -> (Vec2, Vec2) {
        if self.pts.len() < 2 || self.total_len <= 0.0 {
            return (self.pts.first().copied().unwrap_or(Vec2::ZERO), Vec2::X);
        }
        let s = s.rem_euclid(self.total_len);
        // Binary search the cumulative table for the segment containing s.
        let mut lo = 0usize;
        let mut hi = self.cum.len() - 1;
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.cum[mid] <= s {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let seg_len = (self.cum[hi] - self.cum[lo]).max(1e-4);
        let t = ((s - self.cum[lo]) / seg_len).clamp(0.0, 1.0);
        let a = self.pts[lo];
        let b = self.pts[hi];
        let pos = a.lerp(b, t);
        let fwd = (b - a).normalize_or_zero();
        (pos, fwd)
    }
}

#[derive(Resource, Default)]
struct TrafficLanes {
    lanes: Vec<Lane>,
    /// Rebuild signature: (road count, total point count). Lanes are static
    /// geometry, so this only changes on a city load.
    signature: Option<(usize, usize)>,
}

/// A single ambient car's fixed identity, derived once from its slot hash.
/// The car's visual variant is fixed by its pooled entity's mesh (`index % 3`),
/// so it isn't stored here.
struct CarSlot {
    lane: usize,
    dir: f32,   // +1 / -1 travel direction along the lane
    side: f32,  // +1 / -1 kerb side (perpendicular offset)
    phase: f32, // current arclength position (advanced by dt)
}

#[derive(Resource, Default)]
struct TrafficPool {
    entities: Vec<Entity>,
    slots: Vec<CarSlot>,
    /// Three variant meshes (sedan / hatch / van) with baked vertex colors.
    car_meshes: [Option<Handle<Mesh>>; 3],
    /// One shared vertex-color material; rebuilt only when `unlit` flips.
    material: Option<Handle<StandardMaterial>>,
    material_unlit: bool,
    /// Live (visible) count this frame, and the cap/lane signature the slot
    /// assignment table was last built for.
    visible: usize,
    assign_sig: Option<(usize, usize, i32)>,
}

#[derive(Component)]
struct AmbientCar;

/// Append an axis-aligned box (per-face normals, one flat color) to a mesh
/// buffer set. Center `c`, half-extents `h`.
#[allow(clippy::too_many_arguments)]
fn push_box(
    pos: &mut Vec<[f32; 3]>,
    nor: &mut Vec<[f32; 3]>,
    col: &mut Vec<[f32; 4]>,
    idx: &mut Vec<u32>,
    c: Vec3,
    h: Vec3,
    color: [f32; 4],
) {
    // 6 faces, each 4 verts with a shared normal.
    let faces: [([f32; 3], [Vec3; 4]); 6] = [
        (
            [0.0, 0.0, 1.0],
            [
                Vec3::new(-h.x, -h.y, h.z),
                Vec3::new(h.x, -h.y, h.z),
                Vec3::new(h.x, h.y, h.z),
                Vec3::new(-h.x, h.y, h.z),
            ],
        ),
        (
            [0.0, 0.0, -1.0],
            [
                Vec3::new(h.x, -h.y, -h.z),
                Vec3::new(-h.x, -h.y, -h.z),
                Vec3::new(-h.x, h.y, -h.z),
                Vec3::new(h.x, h.y, -h.z),
            ],
        ),
        (
            [1.0, 0.0, 0.0],
            [
                Vec3::new(h.x, -h.y, h.z),
                Vec3::new(h.x, -h.y, -h.z),
                Vec3::new(h.x, h.y, -h.z),
                Vec3::new(h.x, h.y, h.z),
            ],
        ),
        (
            [-1.0, 0.0, 0.0],
            [
                Vec3::new(-h.x, -h.y, -h.z),
                Vec3::new(-h.x, -h.y, h.z),
                Vec3::new(-h.x, h.y, h.z),
                Vec3::new(-h.x, h.y, -h.z),
            ],
        ),
        (
            [0.0, 1.0, 0.0],
            [
                Vec3::new(-h.x, h.y, h.z),
                Vec3::new(h.x, h.y, h.z),
                Vec3::new(h.x, h.y, -h.z),
                Vec3::new(-h.x, h.y, -h.z),
            ],
        ),
        (
            [0.0, -1.0, 0.0],
            [
                Vec3::new(-h.x, -h.y, -h.z),
                Vec3::new(h.x, -h.y, -h.z),
                Vec3::new(h.x, -h.y, h.z),
                Vec3::new(-h.x, -h.y, h.z),
            ],
        ),
    ];
    for (n, quad) in faces {
        let base = pos.len() as u32;
        for v in quad {
            let p = c + v;
            pos.push([p.x, p.y, p.z]);
            nor.push(n);
            col.push(color);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Build one car variant mesh (body + cabin + wheel skirt), proportions
/// mirrored from `gen_cars.py`. Body sits with wheels hinted below `floor`.
fn build_car_mesh(variant: u8) -> Mesh {
    let (len, wid, hgt, floor, cab_l, cab_h, cab_x) = match variant {
        0 => (4.6, 1.9, 1.15, 0.35, 4.6 * 0.42, 0.72, -4.6 * 0.05), // sedan
        1 => (3.9, 1.85, 1.2, 0.35, 3.9 * 0.5, 0.82, -3.9 * 0.10),  // hatch
        _ => (4.9, 2.0, 1.55, 0.4, 4.9 * 0.34, 0.55, 4.9 * 0.28),   // van
    };
    let body = MUTED_CAR_TONES[variant as usize % 3];
    let body_c = [body.0, body.1, body.2, 1.0];
    let glass_c = [CAR_GLASS.0, CAR_GLASS.1, CAR_GLASS.2, 1.0];

    let mut pos = Vec::new();
    let mut nor = Vec::new();
    let mut col = Vec::new();
    let mut idx = Vec::new();
    // body (car length along +X, width along Z)
    push_box(
        &mut pos,
        &mut nor,
        &mut col,
        &mut idx,
        Vec3::new(0.0, floor + hgt / 2.0, 0.0),
        Vec3::new(len / 2.0, hgt / 2.0, wid / 2.0),
        body_c,
    );
    // cabin / greenhouse (slightly narrower, on top)
    push_box(
        &mut pos,
        &mut nor,
        &mut col,
        &mut idx,
        Vec3::new(cab_x, floor + hgt + cab_h / 2.0, 0.0),
        Vec3::new(cab_l / 2.0, cab_h / 2.0, wid * 0.86 / 2.0),
        glass_c,
    );
    // wheel skirt (dark, below the body)
    push_box(
        &mut pos,
        &mut nor,
        &mut col,
        &mut idx,
        Vec3::new(0.0, floor * 0.5, 0.0),
        Vec3::new(len * 0.86 / 2.0, floor * 0.6 / 2.0, wid * 1.02 / 2.0),
        glass_c,
    );

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    mesh.insert_indices(Indices::U32(idx));
    mesh
}

/// Rebuild the drivable-lane set from the city road network when it changes.
/// Only ground-level, non-tunnel arterials and collectors carry ambient cars
/// (tunnels are underground; locals are too fine to bother instancing).
fn build_traffic_lanes_system(city: Res<CurrentCity>, mut lanes: ResMut<TrafficLanes>) {
    let Some(city_json) = &city.static_city else {
        return;
    };
    let sig = (
        city_json.roads.len(),
        city_json
            .roads
            .iter()
            .map(|r| r.points.len())
            .sum::<usize>(),
    );
    if lanes.signature == Some(sig) {
        return;
    }
    lanes.signature = Some(sig);
    lanes.lanes.clear();
    for road in &city_json.roads {
        if road.is_tunnel || road.grade_level < 0 {
            continue;
        }
        let cls = road.cls.as_str();
        if cls != "arterial" && cls != "collector" {
            continue;
        }
        let pts: Vec<Vec2> = road
            .points
            .chunks_exact(2)
            .map(|c| Vec2::new(c[0] as f32, c[1] as f32))
            .collect();
        if pts.len() < 2 {
            continue;
        }
        let mut cum = Vec::with_capacity(pts.len());
        let mut acc = 0.0f32;
        cum.push(0.0);
        for w in pts.windows(2) {
            acc += w[0].distance(w[1]);
            cum.push(acc);
        }
        if acc < 20.0 {
            continue; // too short to host flowing traffic
        }
        lanes.lanes.push(Lane {
            pts,
            cum,
            total_len: acc,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn update_traffic_system(
    time: Res<Time>,
    quality: Res<QualityTier>,
    effective: Res<EffectiveKnobs>,
    height_at: Res<HeightAt>,
    traffic: Res<LatestTraffic>,
    lanes: Res<TrafficLanes>,
    mut pool: ResMut<TrafficPool>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cars: Query<(&mut Transform, &mut Visibility), With<AmbientCar>>,
    mut stats: ResMut<RenderCacheStats>,
    mut off_cache: Local<Option<bool>>,
) {
    // Runtime kill switch (`MF_NO_TRAFFIC=1`): an ops/benchmark toggle to A/B
    // the ambient fleet's perf cost without a rebuild. Read once and cached.
    let off = *off_cache.get_or_insert_with(|| std::env::var_os("MF_NO_TRAFFIC").is_some());
    let cap = if AMBIENT_TRAFFIC && !off {
        tier_car_cap(*quality)
    } else {
        0
    };

    // Nothing to show: hide any existing instances and bail.
    if cap == 0 || lanes.lanes.is_empty() {
        if pool.visible != 0 {
            for &e in &pool.entities {
                if let Ok((_, mut vis)) = cars.get_mut(e) {
                    *vis = Visibility::Hidden;
                }
            }
            pool.visible = 0;
            stats.ambient_traffic_cars = 0;
        }
        return;
    }

    // Lazily build the three variant meshes + the shared material.
    for v in 0..3usize {
        if pool.car_meshes[v].is_none() {
            pool.car_meshes[v] = Some(meshes.add(build_car_mesh(v as u8)));
        }
    }
    let unlit = effective.0.unlit_material;
    if pool.material.is_none() || pool.material_unlit != unlit {
        pool.material = Some(materials.add(StandardMaterial {
            base_color: Color::WHITE, // vertex colors carry the tone
            perceptual_roughness: 1.0,
            metallic: 0.0,
            unlit,
            ..default()
        }));
        pool.material_unlit = unlit;
    }
    let material = pool.material.clone().unwrap();

    let max_density = traffic.max_density().max(1e-3);
    let density_bucket = (max_density * 8.0) as i32;
    let assign_sig = (lanes.lanes.len(), cap, density_bucket);

    // (Re)build the deterministic slot-assignment table when the lane set, the
    // cap, or the congestion level materially changed. Lane weight folds in the
    // local density so congested arterials are handed proportionally more cars.
    if pool.assign_sig != Some(assign_sig) {
        pool.assign_sig = Some(assign_sig);
        // Weight per lane = length * (base + gain * density_norm@midpoint).
        let mut weights = Vec::with_capacity(lanes.lanes.len());
        let mut total_w = 0.0f32;
        for lane in &lanes.lanes {
            let (mid, _) = lane.sample(lane.total_len * 0.5);
            let dens = (traffic.density_at(mid.x, mid.y) / max_density).clamp(0.0, 1.0);
            let w = lane.total_len * (0.4 + 1.6 * dens);
            weights.push(w);
            total_w += w;
        }
        let total_w = total_w.max(1e-3);
        pool.slots.clear();
        pool.slots.reserve(cap);
        // Cumulative-weight walk assigns each car index to a lane; hash gives a
        // deterministic offset / direction / side / variant.
        let mut cum_w = 0.0f32;
        let mut lane_i = 0usize;
        let mut acc = 0.0f32;
        for i in 0..cap {
            // target cumulative weight for car i
            let target = (i as f32 + 0.5) / cap as f32 * total_w;
            while lane_i + 1 < lanes.lanes.len() && cum_w + weights[lane_i] < target {
                cum_w += weights[lane_i];
                lane_i += 1;
                acc = 0.0;
            }
            let h = hash_u32(i as u32 ^ 0x9e37_79b9);
            let lane = &lanes.lanes[lane_i];
            // Spread cars along the lane deterministically.
            acc += 1.0;
            let base_frac = ((h >> 8) & 0xffff) as f32 / 65535.0;
            let phase = (base_frac + acc * 0.137).fract() * lane.total_len;
            pool.slots.push(CarSlot {
                lane: lane_i,
                dir: if h & 1 == 0 { 1.0 } else { -1.0 },
                side: if h & 2 == 0 { 1.0 } else { -1.0 },
                phase,
            });
        }
    }

    // Grow the entity pool to the cap (rare; only on a new high-water cap).
    while pool.entities.len() < cap {
        let v = pool.entities.len() % 3;
        let mesh = pool.car_meshes[v].clone().unwrap();
        let e = commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                Transform::IDENTITY,
                Visibility::Hidden,
                AmbientCar,
            ))
            .id();
        pool.entities.push(e);
    }

    let dt = time.delta_secs();
    let mut visible = 0usize;
    for i in 0..pool.entities.len() {
        let e = pool.entities[i];
        let Ok((mut tf, mut vis)) = cars.get_mut(e) else {
            continue;
        };
        if i >= cap || i >= pool.slots.len() {
            *vis = Visibility::Hidden;
            continue;
        }
        let (lane_idx, dir, side, phase) = {
            let s = &pool.slots[i];
            (s.lane, s.dir, s.side, s.phase)
        };
        let lane = &lanes.lanes[lane_idx];
        // Congestion at the car's position slows it (denser -> slower flow).
        let (pos, fwd) = lane.sample(phase);
        let dens = (traffic.density_at(pos.x, pos.y) / max_density).clamp(0.0, 1.0);
        let speed = BASE_SPEED * (1.0 - 0.7 * dens);
        let new_phase = phase + dir * speed * dt;
        pool.slots[i].phase = new_phase;

        // Perpendicular kerb offset so opposing flows separate.
        let perp = Vec2::new(-fwd.y, fwd.x) * (LANE_HALF * side);
        let wx = pos.x + perp.x;
        let wz = pos.y + perp.y;
        let ground = height_at.sample(wx, wz);
        tf.translation = Vec3::new(wx, ground + CAR_LIFT, wz);
        // Align car +X with travel direction (matching vehicles.rs yaw form).
        let travel = fwd * dir;
        let yaw = (-travel.y).atan2(travel.x);
        tf.rotation = Quat::from_rotation_y(yaw);

        *vis = Visibility::Visible;
        visible += 1;
    }
    pool.visible = visible;
    stats.ambient_traffic_cars = visible;
}
