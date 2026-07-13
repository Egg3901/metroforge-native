//! Vehicles (spec §3.3 `vehicles.rs`): a grow-only pool of plain entities
//! (never merged — hundreds at most, no instancing needed), positioned from
//! `LatestFrame`. Steady-state (pool already large enough for the busiest
//! tick seen so far) does **zero per-frame heap allocation**: no `Vec`s
//! built, no strings formatted, no new mesh/material assets — only
//! `Transform`/`Mesh3d`/material-field writes on entities the pool already
//! owns. The only allocations are the rare one-offs when the pool grows to
//! a new high-water mark of simultaneous vehicles.
//!
//! It also does **zero per-frame asset-dirtying** once the wire has gone
//! quiet: `LatestFrame` arrives at the sim's ~20Hz tick while this system
//! runs every render frame (60+ Hz), and `Assets<T>::get_mut` unconditionally
//! marks an asset dirty for GPU re-extract/re-upload regardless of whether
//! the write actually changed anything. The whole system early-outs unless
//! `LatestFrame` or `EffectiveKnobs` changed since last render frame, and even
//! within a changed frame each slot's material is only touched when its
//! (color, quantized brightness, unlit) triple actually differs from what's
//! already applied there.
//!
//! Color: per art-direction §4, the wire's `colorTable` is IGNORED — each
//! vehicle is painted from the client's vivid table (`palette::vivid_route_color`)
//! indexed by `routeColorIdx`. Materials are **shared by paint key**
//! `(color_idx, brightness_bucket, unlit)` so Bevy can batch draws across
//! vehicles that look identical; when a slot's paint changes, its
//! `MeshMaterial3d` handle is swapped to the cached material rather than
//! mutating a per-slot asset.
//!
//! Night headlights / cabin strips extend the same paint-key pattern: a
//! parallel grow-only pool of small emissive quads (cool-white front glow
//! for every mode; warm interior strip for tram/metro) shares materials by
//! `(LightKind, unlit)` — never per-vehicle `PointLight`s, so the
//! body-material batching from the perf audit stays intact. Night intensity
//! is written in place onto those few shared materials (not keyed by the
//! 65-step night bucket), so dusk/dawn cannot mint a new material every
//! bucket step.
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

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use bevy::scene::SceneRoot;

use mf_protocol::TransitMode;
use mf_state::{EffectiveKnobs, HeightAt, LatestFrame, LatestUi, QualityTier};

use crate::daynight::DayNightState;
use crate::models::ModelHandles;
use crate::palette;
use crate::RenderCacheStats;

/// Pilot B knob: swap the brick metro vehicle for the scripted 3-car metro
/// consist glTF (tools/blender/gen_train.py) at Medium+ tiers. Default ON for
/// metro; every other transit mode and the Potato/Low tiers keep the brick.
/// Flip to `false` to fall the whole game back to bricks.
const METRO_MODEL_SWAP: bool = true;

const VEHICLE_BASE_LENGTH: f32 = 10.0;
const VEHICLE_WIDTH: f32 = 4.5;
const VEHICLE_HEIGHT: f32 = 3.5;
const TRAM_LENGTH_MULT: f32 = 1.6;
const TRAM_WIDTH_MULT: f32 = 0.85;

/// Cool-white headlight / running-light glow.
const HEADLIGHT_COLOR: Color = Color::srgb(0.85, 0.92, 1.0);
/// Warm tram/metro cabin strip.
const CABIN_COLOR: Color = Color::srgb(1.0, 0.72, 0.35);
/// Hide vehicle lights until dusk has some weight.
const LIGHT_VISIBLE_NIGHT: f32 = 0.08;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum LightKind {
    Headlight,
    Cabin,
}

pub struct MfVehiclesPlugin;

impl Plugin for MfVehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VehiclePool>()
            .add_systems(
                Update,
                update_vehicles_system.in_set(crate::MfRenderSet::Dynamic),
            )
            // Recolors the async-spawned metro consist scene's body material
            // to the per-route tint, mirroring the brick's per-route paint
            // (Pilot B). Runs after the vehicle system so it sees this frame's
            // desired tint.
            .add_systems(
                Update,
                apply_metro_tint_system
                    .in_set(crate::MfRenderSet::Dynamic)
                    .after(update_vehicles_system),
            );
    }
}

#[derive(Component)]
struct VehicleSlot;

#[derive(Component)]
struct VehicleLightSlot;

/// Metro model instance parented-in-spirit to vehicle slot `index`. Carries
/// the desired per-route tint; `applied_tint` on the pool tracks whether the
/// (async-spawned) scene children have been recolored to it yet.
#[derive(Component)]
struct MetroModelSlot {
    #[allow(dead_code)]
    index: usize,
    /// Desired per-route body tint (updated by the vehicle system each frame).
    tint: Color,
    /// Packed sRGB of the tint currently baked into this instance's cloned
    /// scene materials; `None` until the async scene spawned and got recolored.
    applied: Option<u32>,
    /// Descendant entities whose material is the near-white BODY (identified
    /// once by luminance when the scene first spawns). Re-tint updates just
    /// these, so a dark route color can't defeat the luminance test on a
    /// subsequent recolor.
    body_entities: Vec<Entity>,
}

/// Pack an sRGB color into a u32 for cheap change detection.
fn pack_color(c: Color) -> u32 {
    let s = c.to_srgba();
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (q(s.red) << 16) | (q(s.green) << 8) | q(s.blue)
}

/// True on Medium/High (`QualityTier` has no `Ord`, so match explicitly).
fn is_medium_plus(tier: QualityTier) -> bool {
    matches!(tier, QualityTier::Medium | QualityTier::High)
}

#[derive(Resource, Default)]
struct VehiclePool {
    /// Grow-only: entities are spawned once and reused; excess slots beyond
    /// the current frame's `vehicle_count` are hidden, not despawned.
    entities: Vec<Entity>,
    box_mesh: Option<Handle<Mesh>>,
    tram_mesh: Option<Handle<Mesh>>,
    /// Shared materials keyed by quantized paint. Keys use the raw route
    /// index (same index → same color via [`palette::vivid_route_color`]),
    /// so the theoretical set is `routes × 65 brightness × 2 unlit × 2
    /// overlay`. Overlay dimming changes the mixed color, so it must be
    /// part of the key or a material minted while an overlay is open serves
    /// the washed-out color forever. Unused entries are pruned each paint
    /// pass so a long session cannot retain every occupancy bucket ever
    /// seen — only keys currently applied to a live slot stay alive.
    material_cache: HashMap<(usize, i32, bool, bool), Handle<StandardMaterial>>,
    /// Last-applied paint key per slot, parallel to `entities`.
    applied_paint: Vec<Option<(usize, i32, bool, bool)>>,
    /// Parallel light pool: two entities per vehicle (headlight + cabin).
    light_entities: Vec<[Entity; 2]>,
    headlight_mesh: Option<Handle<Mesh>>,
    cabin_mesh_bus: Option<Handle<Mesh>>,
    cabin_mesh_tram: Option<Handle<Mesh>>,
    /// Shared light materials: `(kind, unlit)` only. Night intensity is
    /// mutated in place (see [`sync_light_emissive`]) so the 65-step night
    /// bucket cannot grow this map across dusk/dawn.
    light_material_cache: HashMap<(LightKind, bool), Handle<StandardMaterial>>,
    applied_light_paint: Vec<[Option<(LightKind, bool)>; 2]>,
    /// Last night bucket this pass ran for. `DayNightState` is written every
    /// frame by the smoothing system, so `is_changed()` is always true and
    /// would defeat the 20 Hz skip-gate below; only a bucket step matters.
    last_night_bucket: Option<i32>,
    /// Pilot B: parallel metro-model instances, one per vehicle slot, spawned
    /// lazily the first time a slot is a Medium+ metro. `None` = not spawned
    /// yet. Reused (shown/hidden) like the brick slots.
    metro_models: Vec<Option<Entity>>,
}

fn material_for_paint(
    cache: &mut HashMap<(usize, i32, bool, bool), Handle<StandardMaterial>>,
    materials: &mut Assets<StandardMaterial>,
    paint_key: (usize, i32, bool, bool),
    color: Color,
    brightness: f32,
) -> Handle<StandardMaterial> {
    cache
        .entry(paint_key)
        .or_insert_with(|| {
            let (color_idx, brightness_bucket, unlit, overlay_dimmed) = paint_key;
            let _ = (color_idx, brightness_bucket, overlay_dimmed); // key already encodes these
            materials.add(StandardMaterial {
                base_color: color,
                emissive: palette::emissive(color, (if unlit { 1.0 } else { 0.4 }) * brightness),
                unlit,
                ..default()
            })
        })
        .clone()
}

fn light_material_for_paint(
    cache: &mut HashMap<(LightKind, bool), Handle<StandardMaterial>>,
    materials: &mut Assets<StandardMaterial>,
    paint_key: (LightKind, bool),
    night: f32,
) -> Handle<StandardMaterial> {
    cache
        .entry(paint_key)
        .or_insert_with(|| {
            let (kind, unlit) = paint_key;
            let color = match kind {
                LightKind::Headlight => HEADLIGHT_COLOR,
                LightKind::Cabin => CABIN_COLOR,
            };
            let strength = light_emissive_strength(kind, night);
            materials.add(StandardMaterial {
                base_color: color,
                emissive: palette::emissive(color, strength),
                unlit,
                alpha_mode: AlphaMode::Opaque,
                ..default()
            })
        })
        .clone()
}

fn light_emissive_strength(kind: LightKind, night: f32) -> f32 {
    let night = night.clamp(0.0, 1.0);
    match kind {
        // Strong emissive so Medium/High bloom picks them up; still a
        // readable bright spot on Low without bloom.
        LightKind::Headlight => 4.0 * night,
        LightKind::Cabin => 2.2 * night,
    }
}

/// Rewrite emissive on every cached light material when the night bucket
/// steps. Keeps the cache at most `2 kinds × 2 unlit = 4` entries for the
/// whole session instead of minting a new material per dusk/dawn bucket.
fn sync_light_emissive(
    cache: &HashMap<(LightKind, bool), Handle<StandardMaterial>>,
    materials: &mut Assets<StandardMaterial>,
    night: f32,
) {
    for (&(kind, _), handle) in cache {
        let color = match kind {
            LightKind::Headlight => HEADLIGHT_COLOR,
            LightKind::Cabin => CABIN_COLOR,
        };
        let strength = light_emissive_strength(kind, night);
        if let Some(mat) = materials.get_mut(handle) {
            mat.emissive = palette::emissive(color, strength);
        }
    }
}

/// Drop body-paint materials that no live slot currently references. Without
/// this, every occupancy bucket a vehicle ever visits stays pinned in
/// `Assets<StandardMaterial>` for the rest of the session.
fn prune_material_cache(pool: &mut VehiclePool) {
    let live: HashSet<(usize, i32, bool, bool)> =
        pool.applied_paint.iter().flatten().copied().collect();
    pool.material_cache.retain(|k, _| live.contains(k));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_vehicles_system(
    frame: Res<LatestFrame>,
    ui: Res<LatestUi>,
    height_at: Res<HeightAt>,
    effective: Res<EffectiveKnobs>,
    theme: Res<mf_state::Theme>,
    colorblind: Res<mf_state::ColorblindMode>,
    overlay: Res<mf_state::OverlayState>,
    day_night: Res<DayNightState>,
    mut pool: ResMut<VehiclePool>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut vehicles: Query<
        (
            &mut Transform,
            &mut Mesh3d,
            &mut MeshMaterial3d<StandardMaterial>,
            &mut Visibility,
        ),
        (With<VehicleSlot>, Without<VehicleLightSlot>),
    >,
    mut lights: Query<
        (
            &mut Transform,
            &mut Mesh3d,
            &mut MeshMaterial3d<StandardMaterial>,
            &mut Visibility,
        ),
        (With<VehicleLightSlot>, Without<VehicleSlot>),
    >,
    // Pilot B params bundled into one tuple SystemParam to stay within Bevy's
    // 16-param limit: (quality tier, loaded model handles, metro model pool).
    metro: (
        Res<QualityTier>,
        Option<Res<ModelHandles>>,
        Query<
            (&mut Transform, &mut Visibility, &mut MetroModelSlot),
            (
                With<MetroModelSlot>,
                Without<VehicleSlot>,
                Without<VehicleLightSlot>,
            ),
        >,
    ),
    mut stats: ResMut<RenderCacheStats>,
) {
    let (quality, model_handles, mut metro_slots) = metro;
    // `LatestFrame` arrives at the sim's ~20Hz tick while this system runs
    // every render frame (60+ Hz); `EffectiveKnobs` / `Theme` / overlay / night
    // change independently and flip paint. None changing means nothing about a
    // vehicle's position, mesh choice or paint could possibly be different
    // from what's already applied, so skip the whole pass.
    let frame_changed = frame.is_changed();
    let night_bucket = (day_night.night_factor.clamp(0.0, 1.0) * 64.0).round() as i32;
    let night_changed = pool.last_night_bucket != Some(night_bucket);
    if !frame_changed
        && !effective.is_changed()
        && !theme.is_changed()
        && !colorblind.is_changed()
        && !overlay.is_changed()
        && !night_changed
    {
        return;
    }
    pool.last_night_bucket = Some(night_bucket);
    let Some(f) = &frame.0 else {
        return;
    };
    let unlit = effective.0.unlit_material;
    let night_factor = day_night.night_factor.clamp(0.0, 1.0);
    // Theme / colorblind switches change `vivid_route_color` for the same
    // color_idx — drop the paint cache so vehicles pick up the new palette.
    if theme.is_changed() || colorblind.is_changed() {
        pool.material_cache.clear();
        for slot in &mut pool.applied_paint {
            *slot = None;
        }
        pool.light_material_cache.clear();
        for slot in &mut pool.applied_light_paint {
            *slot = [None, None];
        }
    }
    // Night bucket stepped: rewrite emissive on the (few) shared light
    // materials in place rather than minting a new handle per bucket.
    if night_changed && !pool.light_material_cache.is_empty() {
        sync_light_emissive(&pool.light_material_cache, &mut materials, night_factor);
    }
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
    let headlight_mesh = pool
        .headlight_mesh
        .get_or_insert_with(|| meshes.add(Cuboid::new(1.6, 0.55, 0.35)))
        .clone();
    let cabin_mesh_bus = pool
        .cabin_mesh_bus
        .get_or_insert_with(|| {
            meshes.add(Cuboid::new(
                VEHICLE_WIDTH * 0.55,
                0.35,
                VEHICLE_BASE_LENGTH * 0.7,
            ))
        })
        .clone();
    let cabin_mesh_tram = pool
        .cabin_mesh_tram
        .get_or_insert_with(|| {
            meshes.add(Cuboid::new(
                VEHICLE_WIDTH * TRAM_WIDTH_MULT * 0.55,
                0.35,
                VEHICLE_BASE_LENGTH * TRAM_LENGTH_MULT * 0.75,
            ))
        })
        .clone();

    let vehicle_count = f.vehicle_count as usize;
    let lights_on = night_factor >= LIGHT_VISIBLE_NIGHT;
    // Grow the entity pool (rare; only when this session has never had this
    // many vehicles on screen at once before). Only meaningful when a new
    // frame actually arrived — `vehicle_count` can't move on a
    // quality-only-changed pass.
    if frame_changed {
        while pool.entities.len() < vehicle_count {
            // Placeholder material; first paint pass swaps to a cached handle.
            let mat = materials.add(StandardMaterial::default());
            let e = commands
                .spawn((
                    Mesh3d(box_mesh.clone()),
                    MeshMaterial3d(mat),
                    Transform::IDENTITY,
                    Visibility::default(),
                    VehicleSlot,
                ))
                .id();
            pool.entities.push(e);
            pool.applied_paint.push(None);

            let head_mat = materials.add(StandardMaterial::default());
            let cabin_mat = materials.add(StandardMaterial::default());
            let head = commands
                .spawn((
                    Mesh3d(headlight_mesh.clone()),
                    MeshMaterial3d(head_mat),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    VehicleLightSlot,
                ))
                .id();
            let cabin = commands
                .spawn((
                    Mesh3d(cabin_mesh_bus.clone()),
                    MeshMaterial3d(cabin_mat),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    VehicleLightSlot,
                ))
                .id();
            pool.light_entities.push([head, cabin]);
            pool.applied_light_paint.push([None, None]);
        }
    }

    let slot_count = pool.entities.len();
    for i in 0..slot_count {
        let entity = pool.entities[i];
        let Ok((mut transform, mut mesh, mut material_handle, mut visibility)) =
            vehicles.get_mut(entity)
        else {
            continue;
        };
        if i >= vehicle_count {
            // Visibility only needs writing when new frame data could have
            // changed which slots are in range.
            if frame_changed {
                *visibility = Visibility::Hidden;
                if let Some([head, cabin]) = pool.light_entities.get(i).copied() {
                    for light_e in [head, cabin] {
                        if let Ok((_, _, _, mut light_vis)) = lights.get_mut(light_e) {
                            *light_vis = Visibility::Hidden;
                        }
                    }
                }
            }
            continue;
        }
        let base = i * 6;
        let (Some(&x), Some(&y), Some(&heading), Some(&occupancy)) = (
            f.vehicles.get(base + 1),
            f.vehicles.get(base + 2),
            f.vehicles.get(base + 3),
            f.vehicles.get(base + 4),
        ) else {
            if frame_changed {
                *visibility = Visibility::Hidden;
                if let Some([head, cabin]) = pool.light_entities.get(i).copied() {
                    for light_e in [head, cabin] {
                        if let Ok((_, _, _, mut light_vis)) = lights.get_mut(light_e) {
                            *light_vis = Visibility::Hidden;
                        }
                    }
                }
            }
            continue;
        };
        let color_idx = f.vehicles.get(base + 5).copied().unwrap_or(0.0) as usize;

        let mode =
            ui.0.as_ref()
                .and_then(|u| u.routes.get(color_idx))
                .map(|r| r.mode)
                .unwrap_or(TransitMode::Bus);
        let is_tram_like = matches!(mode, TransitMode::Tram | TransitMode::Metro);
        let is_tram_mesh = mode == TransitMode::Tram;
        // Pilot B: this slot shows the scripted 3-car metro consist instead of
        // the brick when the knob is on, the tier is Medium+, the route is
        // Metro, and the model assets have loaded.
        let metro_active = METRO_MODEL_SWAP
            && is_medium_plus(*quality)
            && mode == TransitMode::Metro
            && model_handles.is_some();
        let length = if is_tram_mesh {
            VEHICLE_BASE_LENGTH * TRAM_LENGTH_MULT
        } else {
            VEHICLE_BASE_LENGTH
        };

        // Transform/mesh-shape only depend on wire data, so only rewrite
        // them when a new frame actually arrived — a quality-only-changed
        // pass (e.g. toggling unlit) can't move a vehicle or turn a bus into
        // a tram.
        if frame_changed {
            let ground_y = height_at.sample(x, y);
            transform.translation = Vec3::new(x, ground_y + 3.0, y);
            transform.rotation = Quat::from_rotation_y(-heading);
            // Brick is hidden when the metro model takes over this slot.
            *visibility = if metro_active {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };

            let desired_mesh = if is_tram_mesh { &tram_mesh } else { &box_mesh };
            if mesh.0 != *desired_mesh {
                mesh.0 = desired_mesh.clone();
            }
        }

        let mut color = palette::vivid_route_color(color_idx);
        if overlay.mode != mf_state::OverlayMode::Off {
            // Owner rule: active overlays reduce the network's color strength.
            color = color.mix(&Color::WHITE, 0.6);
        }
        let brightness = 0.6 + occupancy.clamp(0.0, 1.0) * 0.4;
        // Quantize to 1/64 steps: `occupancy` (and thus `brightness`) drifts
        // continuously tick to tick, and comparing raw floats would defeat
        // this cache on essentially every changed frame for a difference no
        // player could see.
        let brightness_bucket = (brightness * 64.0).round() as i32;
        let overlay_dimmed = overlay.mode != mf_state::OverlayMode::Off;
        let paint_key = (color_idx, brightness_bucket, unlit, overlay_dimmed);
        if pool.applied_paint.get(i).copied().flatten() != Some(paint_key) {
            let handle = material_for_paint(
                &mut pool.material_cache,
                &mut materials,
                paint_key,
                color,
                brightness,
            );
            material_handle.0 = handle;
            if let Some(slot) = pool.applied_paint.get_mut(i) {
                *slot = Some(paint_key);
            }
        }

        // --- Pilot B: metro consist model swap (parallel model pool) ---
        if METRO_MODEL_SWAP {
            if pool.metro_models.len() <= i {
                pool.metro_models.resize(i + 1, None);
            }
            if metro_active {
                if let Some(handles) = &model_handles {
                    // Model base sits at y=0 authored; the brick center is
                    // ground+3.0, so drop the model to the deck.
                    let base = transform.translation - Vec3::Y * 3.0;
                    let model_xf = Transform {
                        translation: base,
                        rotation: transform.rotation,
                        scale: Vec3::ONE,
                    };
                    match pool.metro_models[i] {
                        Some(e) => {
                            if let Ok((mut mxf, mut mvis, mut mslot)) = metro_slots.get_mut(e) {
                                if frame_changed {
                                    *mxf = model_xf;
                                }
                                *mvis = Visibility::Visible;
                                mslot.tint = color;
                            }
                        }
                        None => {
                            let e = commands
                                .spawn((
                                    SceneRoot(handles.train_metro.clone()),
                                    model_xf,
                                    Visibility::Visible,
                                    MetroModelSlot {
                                        index: i,
                                        tint: color,
                                        applied: None,
                                        body_entities: Vec::new(),
                                    },
                                ))
                                .id();
                            pool.metro_models[i] = Some(e);
                        }
                    }
                }
            } else if let Some(e) = pool.metro_models.get(i).copied().flatten() {
                if let Ok((_, mut mvis, _)) = metro_slots.get_mut(e) {
                    *mvis = Visibility::Hidden;
                }
            }
        }

        // --- Night headlights / cabin strips (batched paint-key materials) ---
        let Some([head_e, cabin_e]) = pool.light_entities.get(i).copied() else {
            continue;
        };
        let vehicle_xf = *transform;

        // Headlight: cool white quad at the vehicle front.
        if let Ok((mut light_xf, mut light_mesh, mut light_mat, mut light_vis)) =
            lights.get_mut(head_e)
        {
            if frame_changed {
                let local = Vec3::new(0.0, 0.6, length * 0.5 + 0.15);
                light_xf.translation = vehicle_xf.translation + vehicle_xf.rotation * local;
                light_xf.rotation = vehicle_xf.rotation;
                if light_mesh.0 != headlight_mesh {
                    light_mesh.0 = headlight_mesh.clone();
                }
            }
            *light_vis = if lights_on && *visibility != Visibility::Hidden {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
            let light_key = (LightKind::Headlight, unlit);
            if pool.applied_light_paint.get(i).and_then(|s| s[0]) != Some(light_key) {
                light_mat.0 = light_material_for_paint(
                    &mut pool.light_material_cache,
                    &mut materials,
                    light_key,
                    night_factor,
                );
                if let Some(slot) = pool.applied_light_paint.get_mut(i) {
                    slot[0] = Some(light_key);
                }
            }
        }

        // Cabin warm strip: tram/metro only.
        if let Ok((mut light_xf, mut light_mesh, mut light_mat, mut light_vis)) =
            lights.get_mut(cabin_e)
        {
            let show_cabin = lights_on && is_tram_like && *visibility != Visibility::Hidden;
            if frame_changed {
                let local = Vec3::new(0.0, VEHICLE_HEIGHT * 0.55, 0.0);
                light_xf.translation = vehicle_xf.translation + vehicle_xf.rotation * local;
                light_xf.rotation = vehicle_xf.rotation;
                let desired = if is_tram_mesh {
                    &cabin_mesh_tram
                } else {
                    &cabin_mesh_bus
                };
                if light_mesh.0 != *desired {
                    light_mesh.0 = desired.clone();
                }
            }
            *light_vis = if show_cabin {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
            if show_cabin {
                let light_key = (LightKind::Cabin, unlit);
                if pool.applied_light_paint.get(i).and_then(|s| s[1]) != Some(light_key) {
                    light_mat.0 = light_material_for_paint(
                        &mut pool.light_material_cache,
                        &mut materials,
                        light_key,
                        night_factor,
                    );
                    if let Some(slot) = pool.applied_light_paint.get_mut(i) {
                        slot[1] = Some(light_key);
                    }
                }
            }
        }
    }

    // Pilot B: hide metro models for slots beyond the current vehicle count
    // (the brick pool hides those slots too; keep the model pool in step).
    if frame_changed {
        for i in vehicle_count..pool.metro_models.len() {
            if let Some(e) = pool.metro_models[i] {
                if let Ok((_, mut mvis, _)) = metro_slots.get_mut(e) {
                    *mvis = Visibility::Hidden;
                }
            }
        }
    }

    prune_material_cache(&mut pool);
    stats.vehicle_slots = pool.entities.len();
    stats.vehicle_material_cache = pool.material_cache.len();
    stats.vehicle_light_material_cache = pool.light_material_cache.len();
}

/// Pilot B tint pass: walks each metro model scene's descendants and recolors
/// the near-white BODY material to the route tint (per-instance material
/// clone, so windows/roof/bogies keep their neutral palette). Cheap: skips any
/// instance already baked to its current tint. The body meshes are located
/// once by luminance the first time the async scene is present, then reused.
fn apply_metro_tint_system(
    mut slots: Query<(Entity, &mut MetroModelSlot)>,
    children: Query<&Children>,
    mut mesh_mats: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (root, mut slot) in &mut slots {
        let want = pack_color(slot.tint);
        if slot.applied == Some(want) {
            continue;
        }
        // First time: discover the body-material mesh entities by luminance.
        if slot.body_entities.is_empty() {
            let mut stack = vec![root];
            let mut found = Vec::new();
            while let Some(e) = stack.pop() {
                if let Ok(kids) = children.get(e) {
                    for c in kids.iter() {
                        stack.push(c);
                    }
                }
                if let Ok(mm) = mesh_mats.get(e) {
                    if let Some(mat) = materials.get(&mm.0) {
                        let b = mat.base_color.to_srgba();
                        let lum = 0.299 * b.red + 0.587 * b.green + 0.114 * b.blue;
                        if lum > 0.7 {
                            found.push(e);
                        }
                    }
                }
            }
            if found.is_empty() {
                // Scene not spawned yet — retry next frame.
                continue;
            }
            slot.body_entities = found;
        }
        // Re-tint the recorded body meshes (per-instance material clone).
        let tint = slot.tint;
        let mut done = true;
        for &e in &slot.body_entities {
            if let Ok(mut mm) = mesh_mats.get_mut(e) {
                let cloned = materials.get(&mm.0).map(|m| {
                    let mut c = m.clone();
                    c.base_color = tint;
                    c
                });
                if let Some(c) = cloned {
                    mm.0 = materials.add(c);
                } else {
                    done = false;
                }
            } else {
                done = false;
            }
        }
        if done {
            slot.applied = Some(want);
        }
    }
}
