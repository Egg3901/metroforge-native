//! Photo / cinematic mode (self-contained): P toggles a detached free-fly
//! camera with HUD hidden, a local time-of-day scrubber, FOV control,
//! optional letterbox, F12 PNG capture to the Pictures directory, and
//! orbit-and-dolly Catmull-Rom keyframe paths for trailer shots.
//!
//! Leaving photo mode restores the exact prior [`CameraRig`], projection
//! FOV, and sim-driven day/night (clears [`PhotoModeRender::override_hour`]).
//! Sim speed is frozen on enter and restored on exit without touching
//! [`PauseState`] (so the pause overlay never appears).
//!
//! Cost when inactive: one key-poll system. Every other system is
//! `run_if(photo_mode_active)` — no free-fly, UI, or path evaluation.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_net::SimLink;
use mf_protocol::{SetSpeedPayload, ToSim, ToastTone};
use mf_render::PhotoModeRender;
use mf_state::LatestUi;

use crate::camera::CameraRig;
use crate::hud::ToastLog;
use crate::state::AppState;

/// Max keyframes on a cinematic path (orbit-and-dolly trailer shots).
pub const MAX_KEYFRAMES: usize = 4;
/// Min keyframes required before Play is allowed.
pub const MIN_KEYFRAMES: usize = 2;

const DEFAULT_FOV_Y: f32 = std::f32::consts::FRAC_PI_4; // 45°
const MIN_FOV_Y: f32 = 0.35; // ~20°
const MAX_FOV_Y: f32 = 1.75; // ~100°
/// Free-fly move speed (m/s) at "1x"; shift multiplies.
const FLY_SPEED: f32 = 180.0;
const FLY_FAST_MULT: f32 = 4.0;
/// Mouse-look sensitivity (radians per pixel).
const LOOK_SENS: f32 = 0.0025;
/// Velocity ease rate for free-fly (frame-rate-independent).
const FLY_SMOOTH_RATE: f32 = 8.0;
/// Seconds of wall time per keyframe segment during cinematic playback.
const SEGMENT_SECS: f32 = 3.5;

/// One camera pose on a cinematic path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraKeyframe {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
}

/// Snapshot restored when leaving photo mode.
#[derive(Debug, Clone, Copy)]
struct SavedView {
    rig: CameraRig,
    fov_y: f32,
    transform: Transform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CapturePhase {
    #[default]
    Idle,
    /// Hide chrome this frame; capture next.
    Arm,
    /// Screenshot spawned; waiting to re-show chrome.
    Fired,
}

#[derive(Resource, Debug)]
pub struct PhotoModeState {
    pub active: bool,
    /// Free-fly eye (roll locked at 0).
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    /// Local TOD scrubber (0..24). Pushed into [`PhotoModeRender`].
    pub scrub_hour: f32,
    pub letterbox: bool,
    pub keyframes: Vec<CameraKeyframe>,
    /// Playback progress in segment-space: 0 .. (n-1). `None` = stopped.
    pub play_t: Option<f32>,
    /// Hide the photo-mode egui chrome (used during F12 capture).
    pub hide_chrome: bool,
    /// Brief on-screen path toast inside photo mode (HUD is hidden).
    pub capture_toast: Option<String>,
    capture_phase: CapturePhase,
    saved: Option<SavedView>,
    /// Sim speed to restore on exit (`None` if we never froze it).
    saved_speed: Option<f64>,
    velocity: Vec3,
}

impl Default for PhotoModeState {
    fn default() -> Self {
        PhotoModeState {
            active: false,
            eye: Vec3::new(0.0, 1200.0, 1200.0),
            yaw: 0.0,
            pitch: -0.4,
            fov_y: DEFAULT_FOV_Y,
            scrub_hour: 12.0,
            letterbox: false,
            keyframes: Vec::new(),
            play_t: None,
            hide_chrome: false,
            capture_toast: None,
            capture_phase: CapturePhase::Idle,
            saved: None,
            saved_speed: None,
            velocity: Vec3::ZERO,
        }
    }
}

pub struct MfPhotoModePlugin;

impl Plugin for MfPhotoModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhotoModeState>()
            .add_systems(
                Update,
                (
                    photo_mode_toggle_system.run_if(in_state(AppState::InGame)),
                    (
                        photo_mode_fly_system,
                        photo_mode_cinematic_system,
                        photo_mode_apply_camera_system,
                        photo_mode_sync_render_system,
                        photo_mode_capture_system,
                    )
                        .chain()
                        .run_if(in_state(AppState::InGame))
                        .run_if(photo_mode_active),
                ),
            )
            .add_systems(
                EguiPrimaryContextPass,
                photo_mode_ui_system
                    .run_if(in_state(AppState::InGame))
                    .run_if(photo_mode_active),
            );
    }
}

/// `run_if` condition: photo mode is engaged. Also used by `camera.rs` to
/// park the RTS rig while free-fly owns the view.
pub fn photo_mode_active(state: Res<PhotoModeState>) -> bool {
    state.active
}

/// Inverse of [`photo_mode_active`] for `camera.rs` `run_if(not(...))`.
pub fn photo_mode_blocks_camera(state: Res<PhotoModeState>) -> bool {
    state.active
}

fn photo_mode_toggle_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<PhotoModeState>,
    mut render: ResMut<PhotoModeRender>,
    mut rigs: Query<(&mut CameraRig, &Transform, &Projection)>,
    link: Option<Res<SimLink>>,
    ui: Res<LatestUi>,
) {
    if !keys.just_pressed(KeyCode::KeyP) {
        return;
    }
    if state.active {
        exit_photo_mode(&mut state, &mut render, &mut rigs, link.as_deref());
    } else {
        enter_photo_mode(&mut state, &mut render, &mut rigs, link.as_deref(), &ui);
    }
}

fn enter_photo_mode(
    state: &mut PhotoModeState,
    render: &mut PhotoModeRender,
    rigs: &mut Query<(&mut CameraRig, &Transform, &Projection)>,
    link: Option<&SimLink>,
    ui: &LatestUi,
) {
    let Ok((rig, transform, projection)) = rigs.single_mut() else {
        return;
    };
    let fov_y = match projection {
        Projection::Perspective(p) => p.fov,
        _ => DEFAULT_FOV_Y,
    };
    state.saved = Some(SavedView {
        rig: *rig,
        fov_y,
        transform: *transform,
    });
    state.eye = transform.translation;
    let (yaw, pitch) = yaw_pitch_from_forward(transform.forward().as_vec3());
    state.yaw = yaw;
    state.pitch = pitch;
    state.fov_y = fov_y;
    state.velocity = Vec3::ZERO;
    state.play_t = None;
    state.keyframes.clear();
    state.hide_chrome = false;
    state.capture_toast = None;
    state.capture_phase = CapturePhase::Idle;

    // Seed scrubber from the live day/night hour if we can derive it from
    // the sim tick; otherwise noon.
    state.scrub_hour =
        ui.0.as_ref()
            .map(|s| ((s.tick % 1200) as f32 / 1200.0) * 24.0)
            .unwrap_or(12.0);

    let current_speed = ui.0.as_ref().map(|s| s.speed).unwrap_or(1.0);
    state.saved_speed = Some(if current_speed > 0.0 {
        current_speed
    } else {
        1.0
    });
    if let Some(link) = link {
        let _ = link
            .transport
            .send(ToSim::SetSpeed(SetSpeedPayload { speed: 0.0 }));
    }

    state.active = true;
    crate::design_system::set_photo_mode_hides_hud(true);
    render.active = true;
    render.override_hour = Some(state.scrub_hour);
    render.letterbox = state.letterbox;
}

fn exit_photo_mode(
    state: &mut PhotoModeState,
    render: &mut PhotoModeRender,
    rigs: &mut Query<(&mut CameraRig, &Transform, &Projection)>,
    link: Option<&SimLink>,
) {
    if let (Some(saved), Ok((mut rig, mut transform, mut projection))) =
        (state.saved.take(), rigs.single_mut())
    {
        *rig = saved.rig;
        *transform = saved.transform;
        if let Projection::Perspective(p) = projection.into_inner() {
            p.fov = saved.fov_y;
        }
    }
    if let Some(speed) = state.saved_speed.take() {
        if let Some(link) = link {
            let _ = link
                .transport
                .send(ToSim::SetSpeed(SetSpeedPayload { speed }));
        }
    }
    state.active = false;
    state.play_t = None;
    state.hide_chrome = false;
    state.capture_phase = CapturePhase::Idle;
    state.velocity = Vec3::ZERO;
    crate::design_system::set_photo_mode_hides_hud(false);
    *render = PhotoModeRender::default();
}

fn photo_mode_fly_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut motion: EventReader<MouseMotion>,
    mut egui_contexts: EguiContexts,
    mut state: ResMut<PhotoModeState>,
) {
    // Cinematic playback owns the pose; don't fight it.
    if state.play_t.is_some() {
        motion.clear();
        return;
    }

    let over_egui = egui_contexts
        .ctx_mut()
        .map(|ctx| ctx.wants_pointer_input())
        .unwrap_or(false);

    let motion_delta: Vec2 = motion.read().map(|m| m.delta).sum();
    if !over_egui && mouse_buttons.pressed(MouseButton::Right) {
        state.yaw -= motion_delta.x * LOOK_SENS;
        state.pitch = (state.pitch - motion_delta.y * LOOK_SENS).clamp(-1.45, 1.45);
    }

    let mut wish = Vec3::ZERO;
    let forward = fly_forward(state.yaw, state.pitch);
    let right = Vec3::new(state.yaw.cos(), 0.0, -state.yaw.sin());
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        wish += forward;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        wish -= forward;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        wish += right;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        wish -= right;
    }
    if keys.pressed(KeyCode::KeyE) || keys.pressed(KeyCode::Space) {
        wish += Vec3::Y;
    }
    if keys.pressed(KeyCode::KeyQ) || keys.pressed(KeyCode::ControlLeft) {
        wish -= Vec3::Y;
    }
    let speed = if keys.pressed(KeyCode::ShiftLeft) {
        FLY_SPEED * FLY_FAST_MULT
    } else {
        FLY_SPEED
    };
    let wish_vel = wish.normalize_or_zero() * speed;
    let dt = time.delta_secs();
    let t = 1.0 - (-FLY_SMOOTH_RATE * dt).exp();
    state.velocity = state.velocity.lerp(wish_vel, t);
    state.eye += state.velocity * dt;
}

fn photo_mode_cinematic_system(time: Res<Time>, mut state: ResMut<PhotoModeState>) {
    let Some(t) = state.play_t else {
        return;
    };
    let n = state.keyframes.len();
    if n < MIN_KEYFRAMES {
        state.play_t = None;
        return;
    }
    let max_t = (n - 1) as f32;
    let new_t = t + time.delta_secs() / SEGMENT_SECS;
    if new_t >= max_t {
        // Land exactly on the last keyframe and stop.
        if let Some(last) = state.keyframes.last().copied() {
            apply_keyframe_to_state(&mut state, last);
        }
        state.play_t = None;
        return;
    }
    state.play_t = Some(new_t);
    let sample = sample_path(&state.keyframes, new_t);
    apply_keyframe_to_state(&mut state, sample);
}

fn apply_keyframe_to_state(state: &mut PhotoModeState, kf: CameraKeyframe) {
    state.eye = kf.position;
    state.yaw = kf.yaw;
    state.pitch = kf.pitch;
    state.fov_y = kf.fov_y.clamp(MIN_FOV_Y, MAX_FOV_Y);
    state.velocity = Vec3::ZERO;
}

fn photo_mode_apply_camera_system(
    mut state: ResMut<PhotoModeState>,
    mut cams: Query<(&mut Transform, &mut Projection), With<CameraRig>>,
) {
    let Ok((mut transform, mut projection)) = cams.single_mut() else {
        return;
    };
    state.fov_y = state.fov_y.clamp(MIN_FOV_Y, MAX_FOV_Y);
    *transform = fly_transform(state.eye, state.yaw, state.pitch);
    if let Projection::Perspective(p) = projection.into_inner() {
        p.fov = state.fov_y;
    }
}

fn photo_mode_sync_render_system(state: Res<PhotoModeState>, mut render: ResMut<PhotoModeRender>) {
    render.active = state.active;
    render.override_hour = Some(state.scrub_hour.rem_euclid(24.0));
    render.letterbox = state.letterbox;
}

fn photo_mode_capture_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<PhotoModeState>,
    mut commands: Commands,
    mut toasts: ResMut<ToastLog>,
) {
    match state.capture_phase {
        CapturePhase::Idle => {
            if keys.just_pressed(KeyCode::F12) {
                state.hide_chrome = true;
                state.capture_phase = CapturePhase::Arm;
            }
        }
        CapturePhase::Arm => {
            let path = screenshot_path();
            let path_str = path.display().to_string();
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path_str.clone()));
            state.capture_toast = Some(path_str.clone());
            toasts
                .0
                .push((format!("Screenshot saved: {path_str}"), ToastTone::Good));
            if toasts.0.len() > 20 {
                let excess = toasts.0.len() - 20;
                toasts.0.drain(0..excess);
            }
            state.capture_phase = CapturePhase::Fired;
        }
        CapturePhase::Fired => {
            // One frame of clean output; restore chrome.
            state.hide_chrome = false;
            state.capture_phase = CapturePhase::Idle;
        }
    }
}

fn photo_mode_ui_system(mut contexts: EguiContexts, mut state: ResMut<PhotoModeState>) -> Result {
    if state.hide_chrome {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Photo mode")
        .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0])
        .resizable(false)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new("P exit · F12 capture · RMB look · WASD fly · Q/E up-down")
                    .small()
                    .color(egui::Color32::from_rgb(0x88, 0x88, 0x88)),
            );
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label("Time of day");
                ui.add(
                    egui::Slider::new(&mut state.scrub_hour, 0.0..=24.0)
                        .suffix("h")
                        .fixed_decimals(1),
                );
            });
            ui.horizontal(|ui| {
                ui.label("FOV");
                let mut deg = state.fov_y.to_degrees();
                if ui
                    .add(egui::Slider::new(&mut deg, 20.0..=100.0).suffix("°"))
                    .changed()
                {
                    state.fov_y = deg.to_radians().clamp(MIN_FOV_Y, MAX_FOV_Y);
                }
            });
            ui.checkbox(&mut state.letterbox, "Letterbox (clean, no grain)");

            ui.separator();
            ui.label(format!(
                "Cinematic path ({}/{} keyframes)",
                state.keyframes.len(),
                MAX_KEYFRAMES
            ));
            ui.horizontal(|ui| {
                let can_add = state.keyframes.len() < MAX_KEYFRAMES && state.play_t.is_none();
                if ui
                    .add_enabled(can_add, egui::Button::new("Drop keyframe"))
                    .clicked()
                {
                    state.keyframes.push(CameraKeyframe {
                        position: state.eye,
                        yaw: state.yaw,
                        pitch: state.pitch,
                        fov_y: state.fov_y,
                    });
                }
                if ui
                    .add_enabled(
                        !state.keyframes.is_empty() && state.play_t.is_none(),
                        egui::Button::new("Clear"),
                    )
                    .clicked()
                {
                    state.keyframes.clear();
                }
                let playing = state.play_t.is_some();
                if playing {
                    if ui.button("Stop").clicked() {
                        state.play_t = None;
                    }
                } else {
                    let can_play = state.keyframes.len() >= MIN_KEYFRAMES;
                    if ui
                        .add_enabled(can_play, egui::Button::new("Play"))
                        .clicked()
                    {
                        state.play_t = Some(0.0);
                        if let Some(first) = state.keyframes.first().copied() {
                            apply_keyframe_to_state(&mut state, first);
                        }
                    }
                }
            });

            if let Some(toast) = &state.capture_toast {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(0x34, 0xc7, 0x59), toast);
            }
        });
    Ok(())
}

fn screenshot_path() -> PathBuf {
    let dir = directories::UserDirs::new()
        .and_then(|u| u.picture_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dir.join(format!("metroforge-{stamp}.png"))
}

fn fly_forward(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        yaw.cos() * pitch.cos(),
    )
    .normalize_or_zero()
}

fn fly_transform(eye: Vec3, yaw: f32, pitch: f32) -> Transform {
    let forward = fly_forward(yaw, pitch);
    // Roll locked: world up projected orthogonal to forward.
    Transform::from_translation(eye).looking_to(forward, Vec3::Y)
}

fn yaw_pitch_from_forward(forward: Vec3) -> (f32, f32) {
    let f = forward.normalize_or_zero();
    let pitch = f.y.asin().clamp(-1.45, 1.45);
    let yaw = f.x.atan2(f.z);
    (yaw, pitch)
}

// ---------------------------------------------------------------------
// Catmull-Rom path math (unit-tested)
// ---------------------------------------------------------------------

/// Uniform Catmull-Rom on a scalar channel. `t` in [0, 1] between p1 and p2.
pub fn catmull_rom_f32(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

/// Uniform Catmull-Rom on a Vec3.
pub fn catmull_rom_vec3(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    Vec3::new(
        catmull_rom_f32(p0.x, p1.x, p2.x, p3.x, t),
        catmull_rom_f32(p0.y, p1.y, p2.y, p3.y, t),
        catmull_rom_f32(p0.z, p1.z, p2.z, p3.z, t),
    )
}

/// Shortest-path delta from `from` to `to` on a circle of period `period`.
pub fn shortest_angle_delta(from: f32, to: f32, period: f32) -> f32 {
    let half = period * 0.5;
    let mut d = (to - from).rem_euclid(period);
    if d > half {
        d -= period;
    }
    d
}

/// Unwrap yaw samples so Catmull-Rom never spins the long way across ±π.
pub fn unwrap_yaw_series(yaws: &[f32]) -> Vec<f32> {
    if yaws.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(yaws.len());
    out.push(yaws[0]);
    for i in 1..yaws.len() {
        let prev = out[i - 1];
        let delta = shortest_angle_delta(prev, yaws[i], std::f32::consts::TAU);
        out.push(prev + delta);
    }
    out
}

/// Sample a keyframe path at segment-space `t` in `[0, n-1]`.
/// Endpoints are duplicated so 2-point paths still Catmull-Rom cleanly.
pub fn sample_path(keyframes: &[CameraKeyframe], t: f32) -> CameraKeyframe {
    let n = keyframes.len();
    assert!(
        n >= MIN_KEYFRAMES,
        "sample_path requires at least {MIN_KEYFRAMES} keyframes"
    );
    let max_t = (n - 1) as f32;
    let t = t.clamp(0.0, max_t);
    let seg = t.floor() as usize;
    let seg = seg.min(n - 2);
    let local = (t - seg as f32).clamp(0.0, 1.0);

    let i0 = seg.saturating_sub(1);
    let i1 = seg;
    let i2 = (seg + 1).min(n - 1);
    let i3 = (seg + 2).min(n - 1);

    let yaws = unwrap_yaw_series(&keyframes.iter().map(|k| k.yaw).collect::<Vec<_>>());

    CameraKeyframe {
        position: catmull_rom_vec3(
            keyframes[i0].position,
            keyframes[i1].position,
            keyframes[i2].position,
            keyframes[i3].position,
            local,
        ),
        yaw: catmull_rom_f32(yaws[i0], yaws[i1], yaws[i2], yaws[i3], local),
        pitch: catmull_rom_f32(
            keyframes[i0].pitch,
            keyframes[i1].pitch,
            keyframes[i2].pitch,
            keyframes[i3].pitch,
            local,
        ),
        fov_y: catmull_rom_f32(
            keyframes[i0].fov_y,
            keyframes[i1].fov_y,
            keyframes[i2].fov_y,
            keyframes[i3].fov_y,
            local,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kf(x: f32, y: f32, z: f32, yaw: f32) -> CameraKeyframe {
        CameraKeyframe {
            position: Vec3::new(x, y, z),
            yaw,
            pitch: 0.0,
            fov_y: DEFAULT_FOV_Y,
        }
    }

    #[test]
    fn catmull_rom_interpolates_endpoints_of_segment() {
        // At t=0 the curve must pass through p1; at t=1 through p2.
        let p0 = Vec3::new(0.0, 0.0, 0.0);
        let p1 = Vec3::new(1.0, 0.0, 0.0);
        let p2 = Vec3::new(2.0, 0.0, 0.0);
        let p3 = Vec3::new(3.0, 0.0, 0.0);
        let a = catmull_rom_vec3(p0, p1, p2, p3, 0.0);
        let b = catmull_rom_vec3(p0, p1, p2, p3, 1.0);
        assert!((a - p1).length() < 1e-5, "t=0 -> {a:?}");
        assert!((b - p2).length() < 1e-5, "t=1 -> {b:?}");
    }

    #[test]
    fn catmull_rom_midpoint_is_between_neighbors_on_line() {
        let p0 = Vec3::new(0.0, 0.0, 0.0);
        let p1 = Vec3::new(1.0, 0.0, 0.0);
        let p2 = Vec3::new(2.0, 0.0, 0.0);
        let p3 = Vec3::new(3.0, 0.0, 0.0);
        let mid = catmull_rom_vec3(p0, p1, p2, p3, 0.5);
        assert!(mid.x > 1.0 && mid.x < 2.0, "mid.x = {}", mid.x);
        assert!(mid.y.abs() < 1e-5 && mid.z.abs() < 1e-5);
    }

    #[test]
    fn sample_path_hits_first_and_last_keyframe() {
        let keys = vec![
            kf(0.0, 10.0, 0.0, 0.0),
            kf(100.0, 20.0, 0.0, 0.5),
            kf(200.0, 30.0, 50.0, 1.0),
        ];
        let first = sample_path(&keys, 0.0);
        let last = sample_path(&keys, 2.0);
        assert!((first.position - keys[0].position).length() < 1e-4);
        assert!((last.position - keys[2].position).length() < 1e-4);
        assert!((first.yaw - keys[0].yaw).abs() < 1e-4);
        assert!((last.yaw - keys[2].yaw).abs() < 1e-4);
    }

    #[test]
    fn sample_path_two_keyframes_is_well_defined() {
        let keys = vec![kf(0.0, 0.0, 0.0, 0.0), kf(10.0, 0.0, 0.0, 0.0)];
        let mid = sample_path(&keys, 0.5);
        assert!(mid.position.x > 0.0 && mid.position.x < 10.0);
    }

    #[test]
    fn unwrap_yaw_avoids_long_way_across_pi() {
        // 3.0 → -3.0 is a short hop across ±π, not a near-full turn.
        let unwrapped = unwrap_yaw_series(&[3.0, -3.0]);
        assert_eq!(unwrapped.len(), 2);
        let delta = unwrapped[1] - unwrapped[0];
        assert!(
            delta.abs() < std::f32::consts::PI,
            "expected short unwrap, got delta {delta}"
        );
    }

    #[test]
    fn shortest_angle_delta_picks_short_arc() {
        let d = shortest_angle_delta(3.0, -3.0, std::f32::consts::TAU);
        assert!(d.abs() < 1.0, "got {d}");
    }

    #[test]
    fn sample_path_clamps_out_of_range_t() {
        let keys = vec![kf(0.0, 0.0, 0.0, 0.0), kf(1.0, 0.0, 0.0, 0.0)];
        let under = sample_path(&keys, -5.0);
        let over = sample_path(&keys, 99.0);
        assert!((under.position - keys[0].position).length() < 1e-4);
        assert!((over.position - keys[1].position).length() < 1e-4);
    }

    #[test]
    fn fly_transform_roll_is_locked() {
        // looking_to with world up must keep local +Y roughly upright
        // (no bank) for level pitch.
        let tf = fly_transform(Vec3::ZERO, 0.7, 0.0);
        let up = tf.up();
        assert!(up.y > 0.9, "expected near-world-up local up, got {up:?}");
    }
}
