//! RTS camera rig (spec §3.4 `camera.rs`): left-drag pan, wheel dolly,
//! right-drag orbit, WASD/edge pan, ground raycast for click-to-world, and
//! `zoom_to_fit`.

use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy_egui::EguiContexts;
use mf_state::{CurrentCity, HeightAt};

/// Marker + spherical-coordinates state for the single RTS camera.
#[derive(Component, Debug, Clone, Copy)]
pub struct CameraRig {
    /// Ground point (world X, world Y — see coordinate convention: world Y
    /// maps to Bevy Z) the camera orbits/pans around.
    pub target: Vec2,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

pub const MIN_DOLLY: f32 = 120.0;
pub const MAX_DOLLY: f32 = 20_000.0;
/// Pixels of mouse movement below which a click is a click, not a drag.
/// Not consumed yet — v1 has no click-to-build interaction (spec §5: the
/// build panel is a stretch goal); wired here for `input.rs` to use once it
/// lands.
#[allow(dead_code)]
pub const CLICK_DRAG_THRESHOLD_PX: f32 = 8.0;

impl Default for CameraRig {
    fn default() -> Self {
        CameraRig {
            target: Vec2::ZERO,
            yaw: 0.0,
            pitch: 0.55, // looking down at roughly a 35-40 degree angle
            distance: 2_000.0,
        }
    }
}

pub struct MfCameraPlugin;

impl Plugin for MfCameraPlugin {
    fn build(&self, app: &mut App) {
        // The camera must exist from the very first frame, NOT OnEnter(InGame):
        // bevy_egui 0.36 attaches its primary context to the first spawned
        // `Camera`, so with no camera there is no egui context and every
        // pre-game screen (ConnectingSim/MainMenu/Loading) silently renders
        // nothing — shipped v0.1.0-alpha as a bare ClearColor void ("blue
        // screen") for every player. The verify harness never caught it
        // because MF_AUTOSTART skips straight past those states.
        app.add_systems(Startup, spawn_camera)
            .add_systems(
                OnEnter(crate::state::AppState::InGame),
                zoom_to_fit_on_enter,
            )
            .add_systems(
                Update,
                (camera_input_system, camera_transform_system)
                    .chain()
                    .run_if(in_state(crate::state::AppState::InGame)),
            );
    }
}

fn spawn_camera(mut commands: Commands, existing: Query<Entity, With<CameraRig>>) {
    if !existing.is_empty() {
        return;
    }
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 1200.0, 1200.0).looking_at(Vec3::ZERO, Vec3::Y),
        CameraRig::default(),
    ));
}

/// Frame the whole city (spec: "`zoom_to_fit` frames `worldSize`").
fn zoom_to_fit_on_enter(city: Res<CurrentCity>, mut rigs: Query<&mut CameraRig>) {
    let Some(static_city) = &city.static_city else {
        return;
    };
    for mut rig in &mut rigs {
        rig.target = Vec2::ZERO; // origin at city center (spec coordinate convention)
        rig.distance = (static_city.world_size as f32 * 0.75).clamp(MIN_DOLLY, MAX_DOLLY);
    }
}

#[allow(clippy::too_many_arguments)]
fn camera_input_system(
    time: Res<Time>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: EventReader<MouseMotion>,
    mut wheel: EventReader<MouseWheel>,
    mut egui_contexts: EguiContexts,
    windows: Query<&Window>,
    mut rigs: Query<&mut CameraRig>,
    city: Res<CurrentCity>,
) {
    // Drain input events every frame regardless of whether egui is eating
    // them, so they don't pile up and get replayed later.
    let motion_delta: Vec2 = motion.read().map(|m| m.delta).sum();
    let wheel_delta: f32 = wheel.read().map(|w| w.y).sum();

    let over_egui = egui_contexts
        .ctx_mut()
        .map(|ctx| ctx.wants_pointer_input())
        .unwrap_or(false);
    if over_egui {
        return;
    }

    let Ok(mut rig) = rigs.single_mut() else {
        return;
    };
    let dt = time.delta_secs();
    let world_half = city
        .static_city
        .as_ref()
        .map(|c| c.world_size as f32 / 2.0)
        .unwrap_or(f32::MAX);

    // Wheel dolly.
    if wheel_delta != 0.0 {
        let zoom_factor = 1.0 - wheel_delta * 0.1;
        rig.distance = (rig.distance * zoom_factor).clamp(MIN_DOLLY, MAX_DOLLY);
    }

    // Right-drag orbit.
    if mouse_buttons.pressed(MouseButton::Right) {
        rig.yaw -= motion_delta.x * 0.005;
        rig.pitch = (rig.pitch - motion_delta.y * 0.005).clamp(0.1, 1.4);
    } else if mouse_buttons.pressed(MouseButton::Left) {
        // Left-drag pan: move the ground target opposite the drag,
        // scaled by distance so pan speed feels constant on screen.
        let pan_scale = rig.distance * 0.0015;
        let yaw_cos = rig.yaw.cos();
        let yaw_sin = rig.yaw.sin();
        // Screen-space drag -> world-space (rotated by yaw so "up" on screen
        // moves along the camera's forward-ground axis).
        let right = Vec2::new(yaw_cos, -yaw_sin);
        let fwd = Vec2::new(yaw_sin, yaw_cos);
        rig.target -= right * motion_delta.x * pan_scale;
        rig.target += fwd * motion_delta.y * pan_scale;
    }

    // WASD / edge pan.
    let mut pan_dir = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        pan_dir.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        pan_dir.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        pan_dir.x += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        pan_dir.x -= 1.0;
    }
    if let Ok(window) = windows.single() {
        let edge_px = 12.0;
        if let Some(cursor) = window.cursor_position() {
            if cursor.x <= edge_px {
                pan_dir.x -= 1.0;
            } else if cursor.x >= window.width() - edge_px {
                pan_dir.x += 1.0;
            }
            if cursor.y <= edge_px {
                pan_dir.y += 1.0;
            } else if cursor.y >= window.height() - edge_px {
                pan_dir.y -= 1.0;
            }
        }
    }
    if pan_dir != Vec2::ZERO {
        let speed = rig.distance * 0.6; // faster pan when zoomed out
        let yaw_cos = rig.yaw.cos();
        let yaw_sin = rig.yaw.sin();
        let right = Vec2::new(yaw_cos, -yaw_sin);
        let fwd = Vec2::new(yaw_sin, yaw_cos);
        rig.target += (right * pan_dir.x + fwd * pan_dir.y).normalize_or_zero() * speed * dt;
    }

    rig.target = rig
        .target
        .clamp(Vec2::splat(-world_half), Vec2::splat(world_half));
}

fn camera_transform_system(
    height_at: Res<HeightAt>,
    mut cams: Query<(&CameraRig, &mut Transform)>,
) {
    for (rig, mut transform) in &mut cams {
        let ground_y = height_at.sample(rig.target.x, rig.target.y);
        let target_world = Vec3::new(rig.target.x, ground_y, rig.target.y);
        let horiz = rig.distance * rig.pitch.cos();
        let offset = Vec3::new(
            rig.yaw.sin() * horiz,
            rig.distance * rig.pitch.sin(),
            rig.yaw.cos() * horiz,
        );
        transform.translation = target_world + offset;
        *transform = transform.looking_at(target_world, Vec3::Y);
    }
}

/// Ray-cast from a screen point through the given camera onto the ground
/// plane (`y = heightAt(x,z)`, approximated as flat at the target's height
/// since `mf-render`'s real terrain sampler isn't wired in yet). Returns
/// world-space `(x, y)` (i.e. Bevy `(x, z)`) per the coordinate convention.
///
/// Not called yet — v1 has no click-to-build interaction (spec §5 stretch
/// goal); exposed for `input.rs` to use once ground clicks drive a build
/// command (buildStation on click, etc.).
#[allow(dead_code)]
pub fn screen_to_ground(
    camera: &Camera,
    camera_transform: &GlobalTransform,
    height_at: &HeightAt,
    screen_pos: Vec2,
) -> Option<Vec2> {
    let ray = camera
        .viewport_to_world(camera_transform, screen_pos)
        .ok()?;
    let plane_y = height_at.sample(0.0, 0.0);
    let denom = ray.direction.y;
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane_y - ray.origin.y) / denom;
    if t < 0.0 {
        return None;
    }
    let hit = ray.origin + *ray.direction * t;
    Some(Vec2::new(hit.x, hit.z))
}
