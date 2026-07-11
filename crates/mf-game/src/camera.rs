//! RTS camera rig (spec §3.4 `camera.rs`): left-drag pan, wheel dolly,
//! right-drag orbit, WASD/edge pan, ground raycast for click-to-world, and
//! `zoom_to_fit`.
//!
//! Feel: input systems never poke `target`/`yaw`/`pitch`/`distance`
//! directly except while a drag is actively held (orbit, pan) -- those stay
//! 1:1 with the mouse so nothing lags under the cursor. Everything else
//! (wheel dolly, WASD/edge pan) writes the `_goal` twin, and
//! `camera_smoothing_system` eases the real value toward it every frame
//! with frame-rate-independent exponential smoothing. `zoom_to_fit_on_enter`
//! and the verify harness still assign `rig.target`/`.distance`/`.pitch`/
//! `.yaw` directly (screenshots can't wait out a smoothing curve); the
//! smoothing system detects those external writes and snaps the goal to
//! match instead of fighting them. See `advance_smoothing`.

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
    /// Smoothing goals: everything except an active drag writes here, and
    /// `camera_smoothing_system` chases these each frame. Kept in lockstep
    /// with the value (`target`/etc.) whenever an active drag or an
    /// external direct write happens, so there is never a stale goal
    /// fighting a freshly-set value.
    pub target_goal: Vec2,
    pub yaw_goal: f32,
    pub pitch_goal: f32,
    pub distance_goal: f32,
    /// World-space velocity of the last left-drag pan frame, carried into
    /// `target_goal` for a short decay after release (`PAN_RELEASE_DECAY_SECS`)
    /// so a pan doesn't feel like it slams to a dead stop. Not part of the
    /// public API: only `camera_input_system` reads/writes it.
    pan_release_velocity: Vec2,
    /// Seconds of release-glide remaining; 0 means no glide in progress.
    pan_release_timer: f32,
}

pub const MIN_DOLLY: f32 = 120.0;
pub const MAX_DOLLY: f32 = 20_000.0;
/// Pixels of mouse movement below which a click is a click, not a drag.
/// Consumed by `tools.rs`'s `tool_click_system` (v0.2 build tools, ship-plan
/// #25) to tell a clean click-to-build from the drag-pan handled above:
/// a release under this threshold, with egui not wanting the pointer, is a
/// world click; anything else is left alone as an ordinary pan gesture.
pub const CLICK_DRAG_THRESHOLD_PX: f32 = 8.0;

/// Wheel-dolly smoothing rate. Solve `1 - exp(-rate * t) = 0.95` for the
/// 95%-settle time: `t = 3.0 / rate`. 12.0 lands settle right at the
/// ~250ms target for dolly (fast, but eased enough that a hard scroll
/// doesn't feel like a snap-to).
const DOLLY_SMOOTH_RATE: f32 = 12.0;
/// WASD/edge-pan chase rate (drag-pan bypasses this: see
/// `camera_input_system`). 20.0 settles 95% at ~150ms, snappy enough that
/// the smoothing is barely perceptible while still killing per-frame
/// jitter.
const PAN_SMOOTH_RATE: f32 = 20.0;
/// Orbit smoothing rate. Right-drag orbit is written 1:1 (value and goal
/// together) while actively dragging, so this rate only ever applies to
/// the single-frame gap after an external write (zoom_to_fit, verify
/// harness) snaps the goal -- kept tight (same 150ms settle as pan) so
/// orbit never feels like it lags the mouse.
const ORBIT_SMOOTH_RATE: f32 = 20.0;
/// How long a released drag-pan's velocity keeps nudging the goal before
/// decaying to zero. Subtle by design (not full momentum/inertia); set to
/// 0.0 to disable the release glide entirely.
const PAN_RELEASE_DECAY_SECS: f32 = 0.12;
/// Keyboard orbit speed (radians/sec) for Q/E rotate. Q spins the camera
/// left (counter-clockwise looking down), E right. Writes the yaw GOAL so a
/// held key ramps smoothly through `camera_smoothing_system` just like WASD
/// pan does, rather than snapping per frame. ~1.6 rad/s = a full turn in
/// about four seconds, brisk but readable.
const KEY_ORBIT_SPEED: f32 = 1.6;
/// Upper bound on how far a single wheel-dolly frame's zoom-to-cursor recenter
/// may drag the pan target toward (or away from) the cursor's ground point,
/// as a fraction of the target->cursor gap. Keeps a fast multi-notch scroll
/// (or a near-zero distance) from yanking the whole map sideways in one
/// frame; the smoothing curve does the rest.
const ZOOM_CURSOR_MAX_SHIFT: f32 = 0.5;

impl Default for CameraRig {
    fn default() -> Self {
        let target = Vec2::ZERO;
        let yaw = 0.0;
        let pitch = 0.55; // looking down at roughly a 35-40 degree angle
        let distance = 2_000.0;
        CameraRig {
            target,
            yaw,
            pitch,
            distance,
            target_goal: target,
            yaw_goal: yaw,
            pitch_goal: pitch,
            distance_goal: distance,
            pan_release_velocity: Vec2::ZERO,
            pan_release_timer: 0.0,
        }
    }
}

/// Snapshot of what `camera_smoothing_system` produced last frame, used
/// only to tell an external direct write (zoom_to_fit_on_enter, the verify
/// harness) apart from the system's own goal-chasing: if `rig.target` (etc)
/// no longer matches what we last wrote, someone else set it, so the goal
/// snaps to match instead of easing toward a stale target. See
/// `advance_smoothing`.
#[derive(Component, Debug, Clone, Copy)]
struct RigLastOutput {
    target: Vec2,
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// False only for the first frame after spawn, when there is no prior
    /// output to compare against (every channel counts as "externally
    /// written" that frame, which is correct: it should just adopt
    /// `CameraRig::default()`'s already-matching goal, a no-op).
    initialized: bool,
}

impl Default for RigLastOutput {
    fn default() -> Self {
        RigLastOutput {
            target: Vec2::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            distance: 0.0,
            initialized: false,
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
                (
                    camera_input_system,
                    camera_smoothing_system,
                    camera_transform_system,
                )
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
        // Default `PerspectiveProjection::far` is 1000m -- far too tight for
        // this camera, which can dolly out to `MAX_DOLLY` (20km) from its
        // target. Widened so distant geometry (and the sky dome, see
        // `mf-render`'s `sky.rs`, which needs headroom inside this plane to
        // stay fully behind the world at any camera position) isn't clipped.
        Projection::Perspective(PerspectiveProjection {
            far: 60_000.0,
            ..default()
        }),
        Transform::from_xyz(0.0, 1200.0, 1200.0).looking_at(Vec3::ZERO, Vec3::Y),
        // Horizon distance fog (mf-render's sky.rs keeps the color in sync
        // with the theme/day-night sky gradient); start/end tuned to the
        // city scale this camera actually operates at.
        DistanceFog {
            color: Color::WHITE,
            falloff: FogFalloff::Linear {
                start: 8_000.0,
                end: 55_000.0,
            },
            ..default()
        },
        CameraRig::default(),
        RigLastOutput::default(),
    ));
}

/// Frame the whole city (spec: "`zoom_to_fit` frames `worldSize`"). Writes
/// `target`/`distance` directly (not the goals): `camera_smoothing_system`
/// detects the mismatch against last frame's output and snaps the goal to
/// match, so this still takes effect on the very next rendered frame.
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
    cameras: Query<(&Camera, &GlobalTransform)>,
    height_at: Res<HeightAt>,
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

    // Wheel dolly: multiply the GOAL (not the smoothed value), so
    // successive notches from a fast scroll accumulate continuously
    // instead of each one restarting from wherever smoothing has gotten
    // to. `camera_smoothing_system` eases `distance` toward this.
    if wheel_delta != 0.0 {
        let old_goal = rig.distance_goal;
        let new_goal = apply_wheel_dolly(old_goal, wheel_delta);
        rig.distance_goal = new_goal;
        // Zoom toward the cursor: recenter the pan goal so the ground point
        // under the mouse stays (roughly) put as the camera dollies in/out,
        // the way every map/city-builder zoom behaves. Written to the GOAL,
        // in lockstep with the distance goal above, so it eases in on the
        // same smoothing curve instead of snapping. A missing camera/cursor
        // (headless, off-window) just skips the recenter and dollies about
        // the current target, which is the old behavior.
        if let (Some(cursor), Ok((camera, cam_tf))) = (window_cursor(&windows), cameras.single()) {
            if let Some(cursor_ground) = screen_to_ground(camera, cam_tf, &height_at, cursor) {
                rig.target_goal =
                    zoom_toward_cursor(rig.target_goal, cursor_ground, old_goal, new_goal);
            }
        }
    }

    // Right-drag or middle-drag orbit.
    if mouse_buttons.pressed(MouseButton::Right) || mouse_buttons.pressed(MouseButton::Middle) {
        rig.yaw -= motion_delta.x * 0.005;
        rig.pitch = (rig.pitch - motion_delta.y * 0.005).clamp(0.1, 1.4);
        // Active drag: value and goal move together, 1:1 with the mouse.
        // Orbit under the cursor must never feel like it's lagging.
        rig.yaw_goal = rig.yaw;
        rig.pitch_goal = rig.pitch;
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
        let delta_world = -right * motion_delta.x * pan_scale + fwd * motion_delta.y * pan_scale;
        rig.target += delta_world;
        // Active drag: 1:1, same reasoning as orbit above.
        rig.target_goal = rig.target;
        // Track velocity for the post-release glide (see below); refreshed
        // every dragging frame so the timer only counts down once the
        // button actually comes up.
        if dt > 0.0 {
            rig.pan_release_velocity = delta_world / dt;
        }
        rig.pan_release_timer = PAN_RELEASE_DECAY_SECS;
    } else if PAN_RELEASE_DECAY_SECS > 0.0 && rig.pan_release_timer > 0.0 {
        // Residual glide after drag-pan release: nudge the GOAL (not the
        // value) by the last drag velocity, linearly decaying to zero over
        // PAN_RELEASE_DECAY_SECS, then let normal smoothing chase it. Subtle
        // by construction (decay window is short); PAN_RELEASE_DECAY_SECS
        // = 0.0 disables this branch entirely.
        let (new_goal, new_timer) = pan_release_step(
            rig.target_goal,
            rig.pan_release_velocity,
            rig.pan_release_timer,
            dt,
        );
        rig.target_goal = new_goal;
        rig.pan_release_timer = new_timer;
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
        // WASD/edge pan writes the GOAL only; camera_smoothing_system eases
        // the value toward it, so held-key panning still ramps smoothly
        // frame to frame instead of jumping by a raw per-frame delta.
        rig.target_goal += (right * pan_dir.x + fwd * pan_dir.y).normalize_or_zero() * speed * dt;
    }

    // Q/E keyboard orbit. Writes the yaw GOAL (never the value directly) so
    // it rides the same smoothing as WASD pan; a held key sweeps the camera
    // around the target at a constant angular rate. Q = counter-clockwise
    // (left), E = clockwise (right), matching most city-builders' rotate.
    let mut yaw_dir = 0.0;
    if keys.pressed(KeyCode::KeyE) {
        yaw_dir += 1.0;
    }
    if keys.pressed(KeyCode::KeyQ) {
        yaw_dir -= 1.0;
    }
    if yaw_dir != 0.0 {
        rig.yaw_goal += yaw_dir * KEY_ORBIT_SPEED * dt;
    }

    // World-bounds clamp on BOTH the value and the goal: if only the goal
    // were clamped, a value still easing in from outside the bound (e.g.
    // right after an external write) would overshoot past it before
    // catching up; clamping only the value would leave the goal camped
    // just past the edge, so smoothing keeps trying to push past the wall
    // every frame. Clamping both means there is never a goal on the far
    // side of a value for smoothing to oscillate against.
    let bounds_min = Vec2::splat(-world_half);
    let bounds_max = Vec2::splat(world_half);
    rig.target = rig.target.clamp(bounds_min, bounds_max);
    rig.target_goal = rig.target_goal.clamp(bounds_min, bounds_max);
}

/// Applies one wheel-dolly notch (or an accumulated multi-notch delta) to a
/// distance goal: `zoom_factor = 1 - wheel_delta * 0.1` keeps the existing
/// "10% per notch" feel, clamped to the dolly range. Pure function (no ECS
/// params) so the clamping behavior is unit-testable directly.
fn apply_wheel_dolly(distance_goal: f32, wheel_delta: f32) -> f32 {
    let zoom_factor = 1.0 - wheel_delta * 0.1;
    (distance_goal * zoom_factor).clamp(MIN_DOLLY, MAX_DOLLY)
}

/// Recenters the pan `target` toward `cursor_ground` as the camera dollies
/// from `old_dist` to `new_dist`, so the ground point under the mouse stays
/// (approximately, for the fixed camera pitch) fixed on screen through a
/// zoom. The shift fraction is `1 - new_dist/old_dist` — positive (toward
/// the cursor) when zooming in, negative (away) when zooming out, zero when
/// the distance is unchanged — clamped to +/-`ZOOM_CURSOR_MAX_SHIFT` so one
/// aggressive scroll frame can't fling the map. Pure function for testing.
fn zoom_toward_cursor(target: Vec2, cursor_ground: Vec2, old_dist: f32, new_dist: f32) -> Vec2 {
    if old_dist <= 0.0 {
        return target;
    }
    let frac = (1.0 - new_dist / old_dist).clamp(-ZOOM_CURSOR_MAX_SHIFT, ZOOM_CURSOR_MAX_SHIFT);
    target + (cursor_ground - target) * frac
}

/// The single primary window's cursor position, if any. Small helper so the
/// wheel-dolly zoom-to-cursor branch reads cleanly and the borrow of
/// `windows` stays scoped.
fn window_cursor(windows: &Query<&Window>) -> Option<Vec2> {
    windows.single().ok().and_then(|w| w.cursor_position())
}

/// One frame of the post-drag-pan release glide: nudges `goal` by
/// `velocity * dt`, scaled by the fraction of `PAN_RELEASE_DECAY_SECS`
/// remaining (so the nudge linearly fades to nothing rather than cutting
/// off abruptly), and counts `timer` down. Pure function for unit testing.
fn pan_release_step(goal: Vec2, velocity: Vec2, timer: f32, dt: f32) -> (Vec2, f32) {
    let frac = (timer / PAN_RELEASE_DECAY_SECS).clamp(0.0, 1.0);
    let new_goal = goal + velocity * dt * frac;
    let new_timer = (timer - dt).max(0.0);
    (new_goal, new_timer)
}

/// Frame-rate-independent critically-damped exponential smoothing: moves
/// `value` a fraction `1 - exp(-rate * dt)` of the way to `goal` each call.
/// This is the exact solution of `dv/dt = rate * (goal - v)`, so stepping
/// it at any dt granularity (60fps, 30fps, a hitch) lands on the same value
/// for the same elapsed real time — never a per-frame lerp constant that
/// speeds up or slows down with frame rate.
fn smooth_toward(value: f32, goal: f32, rate: f32, dt: f32) -> f32 {
    value + (goal - value) * (1.0 - (-rate * dt).exp())
}

fn smooth_toward_vec2(value: Vec2, goal: Vec2, rate: f32, dt: f32) -> Vec2 {
    Vec2::new(
        smooth_toward(value.x, goal.x, rate, dt),
        smooth_toward(value.y, goal.y, rate, dt),
    )
}

/// Detects external direct writes (comparing against `last`, this system's
/// own output from the previous frame) and snaps the corresponding goal to
/// match, then eases every channel toward its goal by `dt`. Pure function
/// (no ECS `Query`/`Res`) so the settle-time and snap-detection behavior is
/// unit-testable without an `App`.
///
/// Why the comparison is safe: an active drag writes `target`/`yaw`/`pitch`
/// and their goals to the SAME value in `camera_input_system` this frame,
/// so `rig.target != last.target` is true but the subsequent snap
/// (`target_goal = target`) is a no-op — smoothing then computes
/// `value + (goal - value) * f` with `goal == value`, which is `value`
/// exactly, so a drag never gets nudged off of the cursor's position by
/// this system. Goal-only writers (wheel dolly, WASD/edge pan) never touch
/// `target`/`distance` directly, so on an ordinary frame `rig.target` still
/// equals `last.target` and no snap fires — the goal update from this
/// frame's input is left alone to be chased normally. Only a genuine
/// external write (zoom_to_fit_on_enter, the verify harness) changes the
/// value without touching the goal, which is exactly the case this
/// function exists to catch.
fn advance_smoothing(rig: &mut CameraRig, last: &mut RigLastOutput, dt: f32) {
    if !last.initialized || rig.target != last.target {
        rig.target_goal = rig.target;
    }
    if !last.initialized || rig.distance != last.distance {
        rig.distance_goal = rig.distance;
    }
    if !last.initialized || rig.yaw != last.yaw {
        rig.yaw_goal = rig.yaw;
    }
    if !last.initialized || rig.pitch != last.pitch {
        rig.pitch_goal = rig.pitch;
    }

    rig.target = smooth_toward_vec2(rig.target, rig.target_goal, PAN_SMOOTH_RATE, dt);
    rig.distance = smooth_toward(rig.distance, rig.distance_goal, DOLLY_SMOOTH_RATE, dt);
    rig.yaw = smooth_toward(rig.yaw, rig.yaw_goal, ORBIT_SMOOTH_RATE, dt);
    rig.pitch = smooth_toward(rig.pitch, rig.pitch_goal, ORBIT_SMOOTH_RATE, dt);

    last.target = rig.target;
    last.distance = rig.distance;
    last.yaw = rig.yaw;
    last.pitch = rig.pitch;
    last.initialized = true;
}

fn camera_smoothing_system(time: Res<Time>, mut rigs: Query<(&mut CameraRig, &mut RigLastOutput)>) {
    let dt = time.delta_secs();
    for (mut rig, mut last) in &mut rigs {
        advance_smoothing(&mut rig, &mut last, dt);
    }
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
/// First real caller: `reveal_input.rs`'s cursor-reveal driver (issue #18),
/// which needs the world point under the mouse every frame. Still exposed
/// for `input.rs` to reuse once ground clicks also drive a build command
/// (buildStation on click, etc. — spec §5 stretch goal).
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- settle-time bounds -------------------------------------------

    #[test]
    fn dolly_settles_95_percent_within_250ms() {
        let value = smooth_toward(0.0, 100.0, DOLLY_SMOOTH_RATE, 0.25);
        assert!(value >= 95.0, "expected >=95% settle at 250ms, got {value}");
        // Sanity: it isn't instant, i.e. there is actually a curve.
        let early = smooth_toward(0.0, 100.0, DOLLY_SMOOTH_RATE, 0.05);
        assert!(early < 90.0, "dolly reached goal too fast: {early}");
    }

    #[test]
    fn pan_settles_95_percent_within_150ms() {
        let value = smooth_toward(0.0, 100.0, PAN_SMOOTH_RATE, 0.15);
        assert!(value >= 95.0, "expected >=95% settle at 150ms, got {value}");
        let early = smooth_toward(0.0, 100.0, PAN_SMOOTH_RATE, 0.03);
        assert!(early < 90.0, "pan reached goal too fast: {early}");
    }

    #[test]
    fn orbit_settles_within_150ms() {
        let value = smooth_toward(0.0, 100.0, ORBIT_SMOOTH_RATE, 0.15);
        assert!(value >= 95.0, "expected >=95% settle at 150ms, got {value}");
    }

    // --- frame-rate independence ---------------------------------------

    #[test]
    fn smoothing_is_frame_rate_independent() {
        // One big step over 0.3s must match many small steps summing to
        // the same 0.3s, because the exact exponential solution composes:
        // e^{-r*a} * e^{-r*b} == e^{-r*(a+b)}. A naive per-frame lerp
        // constant would NOT have this property (it would depend on step
        // count), which is exactly the frame-rate-dependence bug this
        // formula avoids.
        let rate = 15.0;
        let goal = 500.0;
        let one_big_step = smooth_toward(0.0, goal, rate, 0.3);

        let mut small_steps = 0.0_f32;
        let dt = 0.3 / 37.0; // odd, non-power-of-two step count on purpose
        for _ in 0..37 {
            small_steps = smooth_toward(small_steps, goal, rate, dt);
        }

        assert!(
            (one_big_step - small_steps).abs() < 0.01,
            "big step {one_big_step} vs small steps {small_steps} diverged"
        );
    }

    #[test]
    fn smoothing_never_overshoots_or_reverses() {
        // Monotonic approach to the goal from below; never past it, never
        // backward. Floatiness/oscillation would show up as either.
        let mut value = 0.0_f32;
        let goal = 1000.0;
        for _ in 0..120 {
            let next = smooth_toward(value, goal, PAN_SMOOTH_RATE, 1.0 / 60.0);
            assert!(next >= value, "value went backward: {value} -> {next}");
            assert!(next <= goal, "value overshot goal: {next} > {goal}");
            value = next;
        }
    }

    // --- clamping ---------------------------------------------------------

    #[test]
    fn wheel_dolly_goal_clamps_to_min() {
        // A long run of zoom-in notches must not push the goal below
        // MIN_DOLLY, even accumulating from a value already near it.
        let mut goal = MIN_DOLLY * 2.0;
        for _ in 0..50 {
            goal = apply_wheel_dolly(goal, 10.0); // large zoom-in notch
        }
        assert_eq!(goal, MIN_DOLLY);
    }

    #[test]
    fn wheel_dolly_goal_clamps_to_max() {
        let mut goal = MAX_DOLLY / 2.0;
        for _ in 0..50 {
            goal = apply_wheel_dolly(goal, -10.0); // large zoom-out notch
        }
        assert_eq!(goal, MAX_DOLLY);
    }

    // --- zoom toward cursor ------------------------------------------------

    #[test]
    fn zoom_in_moves_target_toward_cursor() {
        let target = Vec2::new(0.0, 0.0);
        let cursor = Vec2::new(1000.0, 0.0);
        // Zoom in: new < old, so target should slide toward the cursor.
        let out = zoom_toward_cursor(target, cursor, 1000.0, 900.0);
        assert!(out.x > 0.0 && out.x < cursor.x, "got {out:?}");
        // 10% dolly-in -> 10% of the gap closed.
        assert!((out.x - 100.0).abs() < 0.001, "got {out:?}");
    }

    #[test]
    fn zoom_out_moves_target_away_from_cursor() {
        let target = Vec2::new(0.0, 0.0);
        let cursor = Vec2::new(1000.0, 0.0);
        // Zoom out: new > old, negative fraction, target moves away.
        let out = zoom_toward_cursor(target, cursor, 1000.0, 1100.0);
        assert!(out.x < 0.0, "got {out:?}");
    }

    #[test]
    fn zoom_unchanged_distance_is_a_no_op() {
        let target = Vec2::new(42.0, -7.0);
        let out = zoom_toward_cursor(target, Vec2::new(1000.0, 1000.0), 1500.0, 1500.0);
        assert_eq!(out, target);
    }

    #[test]
    fn zoom_shift_is_clamped_against_a_violent_scroll() {
        // A near-total collapse in distance would otherwise slam the target
        // all the way onto the cursor in one frame; the clamp caps it.
        let target = Vec2::new(0.0, 0.0);
        let cursor = Vec2::new(1000.0, 0.0);
        let out = zoom_toward_cursor(target, cursor, 1000.0, 1.0);
        assert!(
            (out.x - cursor.x * ZOOM_CURSOR_MAX_SHIFT).abs() < 0.001,
            "expected clamp to {}, got {out:?}",
            cursor.x * ZOOM_CURSOR_MAX_SHIFT
        );
    }

    #[test]
    fn zoom_toward_cursor_degenerate_zero_distance_is_safe() {
        let target = Vec2::new(5.0, 5.0);
        // old_dist == 0 must not divide-by-zero into NaN; returns target.
        assert_eq!(
            zoom_toward_cursor(target, Vec2::new(9.0, 9.0), 0.0, 100.0),
            target
        );
    }

    #[test]
    fn wheel_dolly_notches_accumulate_continuously() {
        // Two notches back to back (simulating a fast scroll within one
        // frame's accumulated wheel_delta, or two consecutive frames
        // before smoothing has caught up) must compound multiplicatively
        // on the goal, not each reset from some intermediate smoothed
        // value.
        let start = 1000.0;
        let one_notch = apply_wheel_dolly(start, 1.0);
        let two_notches = apply_wheel_dolly(one_notch, 1.0);
        let expected = start * 0.9 * 0.9;
        assert!((two_notches - expected).abs() < 0.001);
    }

    // --- release glide ----------------------------------------------------

    #[test]
    fn pan_release_glide_decays_to_zero() {
        let mut goal = Vec2::ZERO;
        let mut timer = PAN_RELEASE_DECAY_SECS;
        let velocity = Vec2::new(1000.0, 0.0);
        let dt = 1.0 / 60.0;
        let mut steps = 0;
        while timer > 0.0 && steps < 1000 {
            let (new_goal, new_timer) = pan_release_step(goal, velocity, timer, dt);
            goal = new_goal;
            timer = new_timer;
            steps += 1;
        }
        assert_eq!(timer, 0.0, "release timer must fully decay");
        assert!(goal.x > 0.0, "glide should have nudged the goal forward");
        // Bounded: the total glide displacement is well under one full
        // second of travel at the drag velocity (it's supposed to be
        // subtle, not a second wind).
        assert!(goal.x < velocity.x * PAN_RELEASE_DECAY_SECS);
    }

    #[test]
    fn pan_release_step_is_a_no_op_once_timer_is_spent() {
        // With the timer already at zero (the resting state, and what
        // `camera_input_system`'s `PAN_RELEASE_DECAY_SECS > 0.0 &&
        // rig.pan_release_timer > 0.0` guard falls through to once the
        // glide has fully decayed, or immediately if the const is ever set
        // to 0.0) the goal must not move at all.
        let goal = Vec2::new(42.0, -7.0);
        let (new_goal, new_timer) = pan_release_step(goal, Vec2::new(1000.0, 0.0), 0.0, 1.0 / 60.0);
        assert_eq!(new_goal, goal);
        assert_eq!(new_timer, 0.0);
    }

    // --- external-write snap -----------------------------------------------

    #[test]
    fn external_write_snaps_goal_immediately() {
        // Simulate zoom_to_fit_on_enter / the verify harness: a system
        // outside this module sets rig.target/.distance directly, without
        // touching the goals, then the smoothing system runs once. The
        // rig's on-screen value must be the new value THIS SAME CALL, not
        // eased in over time.
        let mut rig = CameraRig::default();
        let mut last = RigLastOutput {
            target: rig.target,
            yaw: rig.yaw,
            pitch: rig.pitch,
            distance: rig.distance,
            initialized: true,
        };

        // A few ordinary frames of goal-chasing first, so `last` reflects
        // real smoothed output (not just defaults).
        rig.distance_goal = 5000.0;
        advance_smoothing(&mut rig, &mut last, 1.0 / 60.0);
        assert!(rig.distance > 2_000.0 && rig.distance < 5000.0);

        // External direct write: verify.rs's frame_street()-style call.
        rig.target = Vec2::new(4321.0, -1234.0);
        rig.distance = 220.0;
        rig.pitch = 0.28;
        rig.yaw = 0.5;

        advance_smoothing(&mut rig, &mut last, 1.0 / 60.0);

        assert_eq!(rig.target, Vec2::new(4321.0, -1234.0));
        assert_eq!(rig.distance, 220.0);
        assert_eq!(rig.pitch, 0.28);
        assert_eq!(rig.yaw, 0.5);
        // And the goals must have snapped too, so a subsequent frame with
        // no new input doesn't drift away from the direct write.
        assert_eq!(rig.target_goal, rig.target);
        assert_eq!(rig.distance_goal, rig.distance);
    }

    #[test]
    fn ordinary_goal_chase_is_not_mistaken_for_external_write() {
        // The inverse check: when only the goal changes (wheel dolly /
        // WASD path), the value must ease in gradually, NOT snap — that
        // would defeat the entire point of smoothing.
        let mut rig = CameraRig::default();
        let mut last = RigLastOutput {
            target: rig.target,
            yaw: rig.yaw,
            pitch: rig.pitch,
            distance: rig.distance,
            initialized: true,
        };

        rig.distance_goal = 10_000.0; // goal-only write, e.g. wheel dolly
        advance_smoothing(&mut rig, &mut last, 1.0 / 60.0);

        // One 60fps frame at DOLLY_SMOOTH_RATE moves ~18% of the remaining
        // distance (1 - exp(-12/60)); the point of this test is just that
        // it is a partial step, not an instant jump to the 10,000 goal.
        assert!(
            rig.distance > 2_000.0 && rig.distance < 5_000.0,
            "expected a partial eased step short of the goal, got {}",
            rig.distance
        );
    }

    #[test]
    fn first_frame_after_spawn_is_a_no_op() {
        // CameraRig::default() already has goal == value everywhere, so the
        // uninitialized-last-output path (every channel treated as an
        // external write) must not visibly move anything.
        let mut rig = CameraRig::default();
        let mut last = RigLastOutput::default();
        let before = rig;
        advance_smoothing(&mut rig, &mut last, 1.0 / 60.0);
        assert_eq!(rig.target, before.target);
        assert_eq!(rig.distance, before.distance);
        assert_eq!(rig.yaw, before.yaw);
        assert_eq!(rig.pitch, before.pitch);
    }
}
