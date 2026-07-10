//! Vehicles (spec §3.3 `vehicles.rs`): a grow-only pool of plain entities
//! (never merged — hundreds at most, no instancing needed), positioned from
//! `LatestFrame` every frame. Steady-state (pool already large enough for
//! the busiest tick seen so far) does **zero per-frame heap allocation**:
//! no `Vec`s built, no strings formatted, no new mesh/material assets — only
//! `Transform`/`Mesh3d`/material-field writes on entities the pool already
//! owns. The only allocations are the rare one-offs when the pool grows to
//! a new high-water mark of simultaneous vehicles.
//!
//! Color: per art-direction §4, the wire's `colorTable` is IGNORED — each
//! vehicle's own material is repainted every frame from the client's vivid
//! table (`palette::vivid_route_color`) indexed by `routeColorIdx`. Each
//! vehicle gets its own material handle (created once, at pool-growth time)
//! rather than sharing one handle per color index, so per-vehicle occupancy
//! brightness doesn't fight between vehicles that happen to share a route
//! color.
//!
//! **Mode (bus/tram/metro/rail), documented gap:** `FrameSnapshot.vehicles`
//! carries no mode field. `sim.worker.ts`'s `sendFrame` sets
//! `routeColorIdx = routeIndex.get(v.routeId)` — the vehicle's *positional*
//! index into that tick's `s.routes` array — and `buildUi`'s `UiState.routes`
//! is built by iterating the same `s.routes` array, so `routeColorIdx` and
//! `LatestUi.routes`'s index line up positionally. This module uses that
//! (undocumented-on-the-wire, but structurally guaranteed) equivalence to
//! look up `ui.routes[idx].mode` for tram elongation (art-direction §4:
//! "trams 1.6x longer, 0.85x width"); an out-of-range index (e.g. one frame
//! of skew right after a route is deleted) falls back to the standard box.

use bevy::prelude::*;

use mf_protocol::TransitMode;
use mf_state::{HeightAt, LatestFrame, LatestUi, QualityTier};

use crate::palette;

const VEHICLE_BASE_LENGTH: f32 = 10.0;
const VEHICLE_WIDTH: f32 = 4.5;
const VEHICLE_HEIGHT: f32 = 3.5;
const TRAM_LENGTH_MULT: f32 = 1.6;
const TRAM_WIDTH_MULT: f32 = 0.85;

pub struct MfVehiclesPlugin;

impl Plugin for MfVehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VehiclePool>().add_systems(
            Update,
            update_vehicles_system.in_set(crate::MfRenderSet::Dynamic),
        );
    }
}

#[derive(Component)]
struct VehicleSlot;

#[derive(Resource, Default)]
struct VehiclePool {
    /// Grow-only: entities are spawned once and reused; excess slots beyond
    /// the current frame's `vehicle_count` are hidden, not despawned.
    entities: Vec<Entity>,
    box_mesh: Option<Handle<Mesh>>,
    tram_mesh: Option<Handle<Mesh>>,
}

#[allow(clippy::too_many_arguments)]
fn update_vehicles_system(
    frame: Res<LatestFrame>,
    ui: Res<LatestUi>,
    height_at: Res<HeightAt>,
    quality: Res<QualityTier>,
    mut pool: ResMut<VehiclePool>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut vehicles: Query<
        (
            &mut Transform,
            &mut Mesh3d,
            &MeshMaterial3d<StandardMaterial>,
            &mut Visibility,
        ),
        With<VehicleSlot>,
    >,
) {
    let Some(f) = &frame.0 else {
        return;
    };
    let unlit = quality.knobs().unlit_material;
    let box_mesh = pool
        .box_mesh
        .get_or_insert_with(|| {
            meshes.add(Cuboid::new(
                VEHICLE_WIDTH,
                VEHICLE_HEIGHT,
                VEHICLE_BASE_LENGTH,
            ))
        })
        .clone();
    let tram_mesh = pool
        .tram_mesh
        .get_or_insert_with(|| {
            meshes.add(Cuboid::new(
                VEHICLE_WIDTH * TRAM_WIDTH_MULT,
                VEHICLE_HEIGHT,
                VEHICLE_BASE_LENGTH * TRAM_LENGTH_MULT,
            ))
        })
        .clone();

    let vehicle_count = f.vehicle_count as usize;
    // Grow the entity pool (rare; only when this session has never had this
    // many vehicles on screen at once before).
    while pool.entities.len() < vehicle_count {
        let e = commands
            .spawn((
                Mesh3d(box_mesh.clone()),
                MeshMaterial3d(materials.add(StandardMaterial::default())),
                Transform::IDENTITY,
                Visibility::default(),
                VehicleSlot,
            ))
            .id();
        pool.entities.push(e);
    }

    for (i, &entity) in pool.entities.iter().enumerate() {
        let Ok((mut transform, mut mesh, material_handle, mut visibility)) =
            vehicles.get_mut(entity)
        else {
            continue;
        };
        if i >= vehicle_count {
            *visibility = Visibility::Hidden;
            continue;
        }
        let base = i * 6;
        let (Some(&x), Some(&y), Some(&heading), Some(&occupancy)) = (
            f.vehicles.get(base + 1),
            f.vehicles.get(base + 2),
            f.vehicles.get(base + 3),
            f.vehicles.get(base + 4),
        ) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let color_idx = f.vehicles.get(base + 5).copied().unwrap_or(0.0) as usize;
        let mode =
            ui.0.as_ref()
                .and_then(|u| u.routes.get(color_idx))
                .map(|r| r.mode)
                .unwrap_or(TransitMode::Bus);
        let is_tram = mode == TransitMode::Tram;

        let ground_y = height_at.sample(x, y);
        transform.translation = Vec3::new(x, ground_y + 3.0, y);
        transform.rotation = Quat::from_rotation_y(-heading);
        *visibility = Visibility::Visible;

        let desired_mesh = if is_tram { &tram_mesh } else { &box_mesh };
        if mesh.0 != *desired_mesh {
            mesh.0 = desired_mesh.clone();
        }

        let color = palette::vivid_route_color(color_idx);
        let brightness = 0.6 + occupancy.clamp(0.0, 1.0) * 0.4;
        if let Some(mat) = materials.get_mut(&material_handle.0) {
            mat.base_color = color;
            mat.emissive = palette::emissive(color, (if unlit { 1.0 } else { 0.4 }) * brightness);
            mat.unlit = unlit;
        }
    }
}
