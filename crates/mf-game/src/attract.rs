//! Main-menu "live diorama" (ship-plan #25, v0.4): while the player sits at
//! `AppState::MainMenu`, the real city (default preset, per `PendingInit`)
//! streams in from the sidecar and a slow attract-mode camera path drifts
//! over it behind the menu — low-altitude oblique vantage points that keep
//! buildings volumetric, never a top-down paper-map framing.
//!
//! Two responsibilities live here, bundled per this wave's mission scope:
//!
//! 1. Kick off (and re-kick-off on a city change) an `init` for the
//!    MainMenu's preview city, drive the attract camera path, and lock
//!    golden-hour lighting via [`mf_state::AttractLighting`] —
//!    [`AttractState`] + [`MfAttractPlugin`]'s `MainMenu`-gated systems.
//! 2. Set the OS window icon once at startup (unrelated feature-wise, just
//!    riding along in the same wave) — [`set_window_icon_system`].
//!
//! ## Why this duplicates a slice of `camera.rs`
//!
//! `camera.rs`'s `camera_smoothing_system` (goal-chasing) and
//! `camera_transform_system` (`CameraRig` -> `Transform`) are BOTH gated
//! `run_if(in_state(AppState::InGame))` — verified by reading that file
//! before writing this one. Writing only `CameraRig`'s `_goal` fields while
//! in `MainMenu` (the pattern `map_mode.rs` uses) would therefore do
//! nothing visible: nothing chases those goals or re-derives the camera's
//! `Transform` outside `InGame`. This module carries its own miniature
//! goal-chase + transform-derivation for the `MainMenu` path only —
//! mirroring the exact precedent `reveal_input.rs` already set for
//! `ease_strength`. The two easers can never fight: `camera.rs`'s versions
//! only run `InGame`, this module's only run `MainMenu`, and the states are
//! mutually exclusive.
//!
//! ## Attract camera path
//!
//! Medium+ tiers crossfade between a handful of low-oblique vantage points
//! every ~20s, with a slow yaw drift inside each hold. Distance is clamped
//! below the tier's fog / building-draw envelope so the horizon never shows
//! raw unfogged terrain. Potato stays dirt cheap: one static framing, no
//! drift, still clamped so buildings fill the screen.
//!
//! ## Init / re-init flow
//!
//! - On first entering `MainMenu` (with `SimHello` in hand and
//!   `MF_AUTOSTART` unset — autostart owns the flow when it's set, see
//!   `state.rs`), sends `ToSim::Init` for `PendingInit`'s preset at a random
//!   seed plus `SetSpeed(30)`, and records `AttractState.inited_preset`.
//! - If the player changes the city picker while still at `MainMenu`,
//!   [`attract_watch_preset_system`] notices `PendingInit.preset_key` no
//!   longer matches `inited_preset` and re-inits for the new city.
//! - `state.rs`'s `send_init_system` (`OnEnter(Loading)`) consults
//!   `inited_preset`: if it already matches what the player is starting,
//!   the city is already streamed in, so it skips the redundant `init`
//!   (which would otherwise throw away everything attract-mode streamed)
//!   and just normalizes the clock back down from attract's 30x.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy::winit::WinitWindows;
use mf_net::SimLink;
use mf_protocol::{InitPayload, SetSpeedPayload, ToSim};
use mf_render::BuildingsDenseCenter;
use mf_state::{AttractLighting, CurrentCity, HeightAt, QualityTier};

use crate::camera::CameraRig;
use crate::state::{AppState, PendingInit, SimHello};

/// Tracks which city preset attract-mode has already streamed in via its own
/// `init`, so `state.rs`'s `send_init_system` can skip a redundant re-init
/// when the player hits Start on that same city, and so this module knows
/// whether/when to re-init itself (first entry, or a picked-a-different-city
/// change). `pub`: `state.rs` consults it directly (see module doc).
#[derive(Resource, Default)]
pub struct AttractState {
    pub inited_preset: Option<String>,
}

/// Per-visit camera path progress. Reset whenever attract re-inits a city so
/// a fresh preset doesn't inherit a mid-crossfade from the previous one.
#[derive(Resource, Default)]
struct AttractCameraState {
    /// Seconds into the current vantage hold (including its trailing crossfade).
    phase_t: f32,
    /// Index of the vantage currently held / fading *from*.
    index: usize,
    /// Potato: latch after the first static frame so we don't keep writing.
    potato_framed: bool,
}

/// One low-altitude oblique framing relative to [`BuildingsDenseCenter`].
/// Pitch stays well below a top-down map angle so facades stay volumetric.
#[derive(Clone, Copy)]
struct AttractVantage {
    target_offset: Vec2,
    yaw: f32,
    pitch: f32,
    /// Nominal dolly distance before the per-tier fog/draw clamp.
    distance: f32,
}

/// Four skyline vantages; Medium+ crossfades through them. Potato uses only
/// the first (static). Distances are intentionally close — elevated 2 km
/// pull-backs read as a flat paper map at the horizon.
const ATTRACT_VANTAGES: [AttractVantage; 4] = [
    AttractVantage {
        target_offset: Vec2::ZERO,
        yaw: 0.55,
        pitch: 0.30,
        distance: 820.0,
    },
    AttractVantage {
        target_offset: Vec2::new(160.0, -110.0),
        yaw: 1.95,
        pitch: 0.26,
        distance: 700.0,
    },
    AttractVantage {
        target_offset: Vec2::new(-130.0, 180.0),
        yaw: 3.55,
        pitch: 0.34,
        distance: 980.0,
    },
    AttractVantage {
        target_offset: Vec2::new(95.0, 140.0),
        yaw: 5.05,
        pitch: 0.28,
        distance: 760.0,
    },
];

/// Hold each vantage this long before crossfading to the next.
const ATTRACT_HOLD_SECS: f32 = 20.0;
/// Trailing portion of each hold spent blending into the next vantage.
const ATTRACT_CROSSFADE_SECS: f32 = 4.0;
/// Slow yaw drift (rad/s) inside a hold — enough to feel alive, not dizzy.
const ATTRACT_DRIFT_YAW_RATE: f32 = 0.012;
/// Steep pitch floor for the fog tiers (Potato/Low). Deliberately steeper
/// than the oblique Medium+ vantages: on the fog tiers the building draw
/// distance is short (3-6km) and the terrain keeps going past it, so a
/// shallow camera stares straight down a long recession of raw un-built
/// terrain and partial-fog road scribbles at the horizon (the "paper map"
/// the owner flagged). Flooring the applied pitch to this on Potato/Low keeps
/// that far horizon out of frame — the diorama reads as the city massing
/// under sky and distance fog cleanly fades the frame edges — while Medium+
/// keeps the full oblique cinematic vantages.
const ATTRACT_PITCH_GOAL: f32 = 0.82;
/// Goal-chase settle rate for pitch/distance/target/yaw (see
/// `attract_smooth`/`attract_smooth_yaw`): deliberately gentler than
/// `camera.rs`'s own `ORBIT_SMOOTH_RATE`/`DOLLY_SMOOTH_RATE` (~150-250ms
/// settle) — a dreamy multi-second drift into the framing reads as
/// "cinematic," where the RTS camera's snappy settle would read as a jarring
/// snap for a menu background nobody is actively steering.
const ATTRACT_SMOOTH_RATE: f32 = 2.0;
/// Fraction of the fog-end / building-draw envelope the camera may use.
/// Staying well inside keeps the horizon inside fog (Potato/Low) or before
/// hard building culls (Medium), so raw unfogged terrain never shows.
const ATTRACT_ENVELOPE_FRACTION: f32 = 0.42;

pub struct MfAttractPlugin;

impl Plugin for MfAttractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttractState>()
            .init_resource::<AttractCameraState>()
            .add_systems(OnEnter(AppState::MainMenu), attract_init_on_enter_system)
            .add_systems(
                OnEnter(AppState::MainMenu),
                attract_lighting_on_enter_system,
            )
            // Keep golden hour through Loading (still showing the diorama);
            // release once gameplay owns the clock.
            .add_systems(OnEnter(AppState::InGame), attract_lighting_on_exit_system)
            .add_systems(
                Update,
                (attract_watch_preset_system, attract_orbit_system)
                    .run_if(in_state(AppState::MainMenu)),
            )
            .add_systems(Startup, set_window_icon_system);
    }
}

fn attract_lighting_on_enter_system(mut attract: ResMut<AttractLighting>) {
    attract.active = true;
}

fn attract_lighting_on_exit_system(mut attract: ResMut<AttractLighting>) {
    attract.active = false;
}

/// `MF_AUTOSTART` (see `state.rs`'s `autostart_system`) owns the whole
/// MainMenu -> Loading flow when set: it skips the city picker entirely, so
/// attract-mode must do nothing at all (no init, no orbit setup) rather than
/// race it. A malformed/empty value is "unset", same convention as
/// `state.rs`'s own check.
fn autostart_env_set() -> bool {
    std::env::var("MF_AUTOSTART")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

/// Pure decision: should entering `MainMenu` right now send an attract-mode
/// `init`? All three conditions from the mission brief, ANDed:
/// - `hello_present`: the sidecar handshake has actually completed (there is
///   a preset list / world size to work with).
/// - `!autostart_set`: `MF_AUTOSTART` isn't about to take over the flow.
/// - `inited_preset.is_none()`: attract hasn't already streamed a city in
///   (covers both "already ran once this MainMenu visit" and "the player
///   already played a session and came back" — either way, re-initing here
///   would restart the sim and discard whatever city data is already live).
fn should_attract_init(
    hello_present: bool,
    autostart_set: bool,
    inited_preset: &Option<String>,
) -> bool {
    hello_present && !autostart_set && inited_preset.is_none()
}

/// Pure decision: given attract already inited `inited_preset`, has the
/// player since picked a different city (so a re-init is owed)? `None`
/// (attract never inited anything, e.g. `MF_AUTOSTART` was set, or `Hello`
/// hadn't arrived yet) never re-inits here — that's `should_attract_init`'s
/// job on a later `OnEnter`, not this system's.
fn should_attract_reinit(
    inited_preset: &Option<String>,
    current_preset: &str,
    autostart_set: bool,
) -> bool {
    if autostart_set {
        return false;
    }
    match inited_preset {
        Some(inited) => inited.as_str() != current_preset,
        None => false,
    }
}

/// Sends the attract-mode `init` (random seed, `SetSpeed(30)` so the city
/// visibly comes alive fast behind the menu) and records `inited_preset`.
fn send_attract_init(link: &SimLink, preset_key: &str) {
    let _ = link.transport.send(ToSim::Init(InitPayload {
        seed: crate::state::rand_seed(),
        difficulty: mf_protocol::Difficulty::Normal,
        size: None,
        preset_key: Some(preset_key.to_string()),
        rules: None,
    }));
    let _ = link
        .transport
        .send(ToSim::SetSpeed(SetSpeedPayload { speed: 30.0 }));
}

fn attract_init_on_enter_system(
    hello: Res<SimHello>,
    pending: Res<PendingInit>,
    mut attract: ResMut<AttractState>,
    mut cam: ResMut<AttractCameraState>,
    link: Option<Res<SimLink>>,
) {
    if !should_attract_init(
        hello.0.is_some(),
        autostart_env_set(),
        &attract.inited_preset,
    ) {
        return;
    }
    let Some(link) = &link else {
        tracing::warn!("mf-attract: entered MainMenu with no SimLink, skipping attract init");
        return;
    };
    send_attract_init(link, &pending.preset_key);
    attract.inited_preset = Some(pending.preset_key.clone());
    *cam = AttractCameraState::default();
}

/// Watches for the player picking a different city in the `MainMenu` combo
/// box (see `hud.rs`'s `main_menu_hud_system`) while attract-mode is already
/// showing a previous one, and re-inits for the new pick. Deliberately does
/// NOT use Bevy's built-in `Res::is_changed()` change-detection: `hud.rs`'s
/// city picker takes `&mut pending.preset_key` unconditionally every single
/// frame it draws the combo box (egui's `selectable_value` needs a mutable
/// reference to compare against, whether or not the player actually clicks
/// anything), which marks the whole `PendingInit` resource "changed" every
/// frame regardless of whether the value moved — so `is_changed()` would
/// fire constantly and this system would re-init on every frame. Comparing
/// the actual string value (`should_attract_reinit`) is the only reliable
/// signal.
fn attract_watch_preset_system(
    pending: Res<PendingInit>,
    mut attract: ResMut<AttractState>,
    mut cam: ResMut<AttractCameraState>,
    link: Option<Res<SimLink>>,
) {
    if !should_attract_reinit(
        &attract.inited_preset,
        &pending.preset_key,
        autostart_env_set(),
    ) {
        return;
    }
    let Some(link) = &link else {
        return;
    };
    send_attract_init(link, &pending.preset_key);
    attract.inited_preset = Some(pending.preset_key.clone());
    *cam = AttractCameraState::default();
}

/// Max camera distance for attract framing on this tier: a fraction of the
/// fog-end (Potato/Low) or building-draw distance (Medium), so the look
/// stays inside the envelope that hides pop-in / raw horizon terrain.
fn attract_distance_cap(quality: QualityTier) -> f32 {
    let knobs = quality.knobs();
    let envelope = knobs
        .fog
        .map(|(_, end)| end)
        .or(knobs.building_draw_distance_m)
        .unwrap_or(14_000.0);
    envelope * ATTRACT_ENVELOPE_FRACTION
}

fn smoothstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Crossfade weight inside a hold: 0 for most of the hold, then eases 0→1
/// across the trailing [`ATTRACT_CROSSFADE_SECS`].
fn crossfade_weight(phase_t: f32) -> f32 {
    let fade_start = (ATTRACT_HOLD_SECS - ATTRACT_CROSSFADE_SECS).max(0.0);
    if phase_t <= fade_start {
        0.0
    } else {
        smoothstep01((phase_t - fade_start) / ATTRACT_CROSSFADE_SECS)
    }
}

/// Advance path clock; returns the (possibly wrapped) phase and vantage index.
fn advance_attract_phase(phase_t: f32, index: usize, dt: f32) -> (f32, usize) {
    let mut t = phase_t + dt;
    let mut i = index;
    let n = ATTRACT_VANTAGES.len();
    while t >= ATTRACT_HOLD_SECS {
        t -= ATTRACT_HOLD_SECS;
        i = (i + 1) % n;
    }
    (t, i)
}

/// Resolve the current path sample: blended vantage goals + optional yaw drift.
fn sample_attract_path(
    phase_t: f32,
    index: usize,
    dense_center: Vec2,
    distance_cap: f32,
    drift: bool,
) -> (Vec2, f32, f32, f32) {
    let n = ATTRACT_VANTAGES.len();
    let from = &ATTRACT_VANTAGES[index % n];
    let to = &ATTRACT_VANTAGES[(index + 1) % n];
    let w = crossfade_weight(phase_t);
    let target = dense_center + from.target_offset.lerp(to.target_offset, w);
    let mut yaw = from.yaw + shortest_angle_delta(from.yaw, to.yaw) * w;
    if drift {
        // Drift only during the hold proper so the crossfade itself stays clean.
        let drift_t = phase_t.min((ATTRACT_HOLD_SECS - ATTRACT_CROSSFADE_SECS).max(0.0));
        yaw += drift_t * ATTRACT_DRIFT_YAW_RATE;
    }
    let pitch = from.pitch + (to.pitch - from.pitch) * w;
    let distance = (from.distance + (to.distance - from.distance) * w).min(distance_cap);
    (target, yaw, pitch, distance)
}

/// Frame-rate-independent exponential smoothing, identical formula to
/// `camera.rs`'s private `smooth_toward` (own copy for the same reason
/// `reveal_input.rs`'s `ease_strength` is its own copy — see module doc).
fn attract_smooth(value: f32, goal: f32, rate: f32, dt: f32) -> f32 {
    value + (goal - value) * (1.0 - (-rate * dt).exp())
}

fn attract_smooth_vec2(value: Vec2, goal: Vec2, rate: f32, dt: f32) -> Vec2 {
    Vec2::new(
        attract_smooth(value.x, goal.x, rate, dt),
        attract_smooth(value.y, goal.y, rate, dt),
    )
}

/// Shortest signed angular delta from `from` to `to` (radians), wrapped into
/// `(-PI, PI]`. Needed because vantage yaws span the circle: naively easing
/// across a wrap would spin the camera the long way.
fn shortest_angle_delta(from: f32, to: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let raw = to - from;
    raw - (raw / tau).round() * tau
}

/// Wrap-aware yaw easing: eases along the shortest angular path to `goal`
/// instead of the raw numeric difference `attract_smooth` would use.
fn attract_smooth_yaw(value: f32, goal: f32, rate: f32, dt: f32) -> f32 {
    let delta = shortest_angle_delta(value, goal);
    value + delta * (1.0 - (-rate * dt).exp())
}

/// Re-derives the camera's `Transform` from `CameraRig`, identical math to
/// `camera.rs`'s private `camera_transform_system` (that system never runs
/// in `MainMenu` — see module doc — so this is the `MainMenu`-side
/// equivalent, not a second competing writer).
fn apply_attract_transform(rig: &CameraRig, height_at: &HeightAt, transform: &mut Transform) {
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

/// Drives the attract camera path while `MainMenu` is showing city data
/// (`CurrentCity.masks_complete()`): samples the vantage path (or a static
/// Potato frame), points at `BuildingsDenseCenter`, and eases `CameraRig` +
/// the actual `Transform` toward that framing every frame.
fn attract_orbit_system(
    time: Res<Time>,
    city: Res<CurrentCity>,
    dense_center: Res<BuildingsDenseCenter>,
    height_at: Res<HeightAt>,
    quality: Res<QualityTier>,
    mut cam: ResMut<AttractCameraState>,
    mut rigs: Query<(&mut CameraRig, &mut Transform)>,
) {
    if !city.masks_complete() {
        return;
    }
    let Ok((mut rig, mut transform)) = rigs.single_mut() else {
        return;
    };
    let dt = time.delta_secs();
    let cap = attract_distance_cap(*quality);
    let potato = matches!(*quality, QualityTier::Potato);

    let (target, yaw, pitch, distance) = if potato {
        if cam.potato_framed {
            // Static: still re-apply transform in case HeightAt refined, but
            // don't advance the path or chase new goals.
            apply_attract_transform(&rig, &height_at, &mut transform);
            return;
        }
        cam.potato_framed = true;
        sample_attract_path(0.0, 0, dense_center.0, cap, false)
    } else {
        let (phase_t, index) = advance_attract_phase(cam.phase_t, cam.index, dt);
        cam.phase_t = phase_t;
        cam.index = index;
        sample_attract_path(phase_t, index, dense_center.0, cap, true)
    };

    // Fog tiers (Potato/Low) have a short building-draw distance with raw
    // terrain past it; the oblique vantage pitches would let that far horizon
    // into frame (the "paper map" #76 fixed). Floor the applied pitch to the
    // steep goal on those tiers so the frame looks down onto the city massing
    // and the horizon stays out — Medium+ keeps the full oblique framing.
    let pitch = if matches!(*quality, QualityTier::Potato | QualityTier::Low) {
        pitch.max(ATTRACT_PITCH_GOAL)
    } else {
        pitch
    };

    rig.yaw_goal = yaw;
    rig.pitch_goal = pitch;
    rig.distance_goal = distance;
    rig.target_goal = target;

    if potato {
        // Snap once so Potato pays no easing cost and never drifts.
        rig.yaw = yaw;
        rig.pitch = pitch;
        rig.distance = distance;
        rig.target = target;
    } else {
        rig.yaw = attract_smooth_yaw(rig.yaw, rig.yaw_goal, ATTRACT_SMOOTH_RATE, dt);
        rig.pitch = attract_smooth(rig.pitch, rig.pitch_goal, ATTRACT_SMOOTH_RATE, dt);
        rig.distance = attract_smooth(rig.distance, rig.distance_goal, ATTRACT_SMOOTH_RATE, dt);
        rig.target = attract_smooth_vec2(rig.target, rig.target_goal, ATTRACT_SMOOTH_RATE, dt);
    }

    apply_attract_transform(&rig, &height_at, &mut transform);
}

// ---------------------------------------------------------------------------
// Window icon (item 3 of this wave's mission — unrelated to the diorama
// above, bundled into this module per mission scope).
// ---------------------------------------------------------------------------

const ICON_SIZE: u32 = 64;

// Art-direction palette (matches `hud.rs`'s `TEXT_COLOR`/`ACCENT`, copied
// here rather than made `pub` in `hud.rs` — that file is owned by a parallel
// agent this wave, see mission brief).
const ICON_WHITE: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
const ICON_BORDER: [u8; 4] = [0x17, 0x18, 0x1c, 0xff]; // rich black
const ICON_ACCENT: [u8; 4] = [0x00, 0x7a, 0xff, 0xff]; // metro blue
const ICON_TRANSPARENT: [u8; 4] = [0x00, 0x00, 0x00, 0x00];

/// True if the pixel centered at `(px, py)` falls inside a rounded square of
/// `size x size` with corner radius `radius`, using the standard
/// clamp-to-nearest-corner signed-distance test: for any point that isn't
/// within `radius` of a corner region, clamping leaves it unchanged (`dx`/
/// `dy` both `0`), so the distance check trivially passes; only actual
/// corner cutoffs can fail it.
fn inside_rounded_square(px: f32, py: f32, size: f32, radius: f32) -> bool {
    let nearest_x = px.clamp(radius, size - radius);
    let nearest_y = py.clamp(radius, size - radius);
    let dx = px - nearest_x;
    let dy = py - nearest_y;
    dx * dx + dy * dy <= radius * radius
}

/// Same test, inset by `inset` on every side (used to carve the border
/// band: outside this but inside the outer `inside_rounded_square` is
/// border).
fn inside_rounded_square_inset(px: f32, py: f32, size: f32, radius: f32, inset: f32) -> bool {
    let inner_size = size - 2.0 * inset;
    if inner_size <= 0.0 {
        return false;
    }
    let inner_radius = (radius - inset).max(0.0);
    inside_rounded_square(px - inset, py - inset, inner_size, inner_radius)
}

/// One pixel of the icon: white rounded square, rich-black border, one
/// diagonal (top-left to bottom-right) accent-blue stripe through the
/// middle — the same square + diagonal wordmark motif used across the
/// brand. Pure function of pixel coordinates so it's directly unit-testable
/// without building a whole image.
fn icon_pixel(x: u32, y: u32, size: u32) -> [u8; 4] {
    let size_f = size as f32;
    let radius = size_f * 0.18;
    let border = size_f * 0.07;
    let stripe_half = size_f * 0.09;
    // Sample at the pixel CENTER, not its corner, so a 1px-thick feature at
    // an exact boundary doesn't flicker between rows/columns.
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;

    if !inside_rounded_square(px, py, size_f, radius) {
        return ICON_TRANSPARENT;
    }
    if !inside_rounded_square_inset(px, py, size_f, radius, border) {
        return ICON_BORDER;
    }
    if (px - py).abs() <= stripe_half {
        return ICON_ACCENT;
    }
    ICON_WHITE
}

/// Row-major RGBA bytes for the whole `size x size` icon (`icon_pixel` per
/// pixel). No image asset file is embedded — generated at startup so
/// there's nothing to keep in sync with the art-direction palette by hand.
fn generate_icon_rgba(size: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity((size as usize) * (size as usize) * 4);
    for y in 0..size {
        for x in 0..size {
            buf.extend_from_slice(&icon_pixel(x, y, size));
        }
    }
    buf
}

/// One-shot: sets the OS window icon (taskbar/titlebar/alt-tab) from
/// [`generate_icon_rgba`]. Not a plain `Startup` system: `WinitWindows`'s
/// entry for the primary window is populated by `bevy_winit`'s own
/// window-creation systems, which aren't guaranteed to have run yet by the
/// time an ordinary `Startup` system executes (the same caution `camera.rs`
/// documents for bevy_egui's context, and the same fix `quality_boot.rs`'s
/// `resolve_quality_system` / `hud.rs`'s `setup_egui_style_system` already
/// use elsewhere in this crate) — retry every `Update` tick via the `done`
/// latch until the window actually exists.
///
/// API confirmed by reading the vendored `bevy_winit` 0.16.1 / `winit` 0.30
/// sources rather than assumed: `WinitWindows` (`bevy::winit::WinitWindows`,
/// re-exported from `bevy_internal` behind the `bevy_winit` feature, which
/// is on by default) is a `NonSend` resource (`!Send`/`!Sync` by a
/// `PhantomData<*const ()>` marker — window-icon calls must stay on the
/// main thread); `WinitWindows::get_window(Entity) ->
/// Option<&WindowWrapper<winit::window::Window>>`; `WindowWrapper<W>`
/// derefs to `W`; `winit::window::Window::set_window_icon(Option<Icon>)`;
/// `winit::window::Icon::from_rgba(Vec<u8>, width, height) ->
/// Result<Icon, BadIcon>`.
fn set_window_icon_system(
    mut done: Local<bool>,
    windows: Query<Entity, With<PrimaryWindow>>,
    winit_windows: NonSend<WinitWindows>,
) {
    if *done {
        return;
    }
    let Ok(entity) = windows.single() else {
        return;
    };
    let Some(window) = winit_windows.get_window(entity) else {
        return;
    };
    *done = true;

    let rgba = generate_icon_rgba(ICON_SIZE);
    match winit::window::Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE) {
        Ok(icon) => window.set_window_icon(Some(icon)),
        Err(e) => tracing::warn!("mf-attract: failed to build window icon: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- attract-init decision table ---------------------------------------

    #[test]
    fn inits_when_hello_present_no_autostart_never_inited() {
        assert!(should_attract_init(true, false, &None));
    }

    #[test]
    fn does_not_init_without_hello() {
        assert!(!should_attract_init(false, false, &None));
    }

    #[test]
    fn does_not_init_when_autostart_is_set() {
        assert!(!should_attract_init(true, true, &None));
    }

    #[test]
    fn does_not_init_twice() {
        assert!(!should_attract_init(true, false, &Some("nyc".to_string())));
    }

    #[test]
    fn does_not_init_when_autostart_and_already_inited_both_true() {
        // Belt-and-suspenders: either condition alone already blocks it.
        assert!(!should_attract_init(true, true, &Some("nyc".to_string())));
    }

    // --- preset-changed re-init decision table -----------------------------

    #[test]
    fn reinits_when_preset_differs_from_inited() {
        assert!(should_attract_reinit(
            &Some("nyc".to_string()),
            "boston",
            false
        ));
    }

    #[test]
    fn does_not_reinit_when_preset_matches_inited() {
        assert!(!should_attract_reinit(
            &Some("nyc".to_string()),
            "nyc",
            false
        ));
    }

    #[test]
    fn does_not_reinit_before_any_attract_init_happened() {
        // inited_preset == None means attract never ran (autostart was set,
        // or Hello hadn't arrived) — nothing to react to yet.
        assert!(!should_attract_reinit(&None, "boston", false));
    }

    #[test]
    fn does_not_reinit_when_autostart_is_set() {
        assert!(!should_attract_reinit(
            &Some("nyc".to_string()),
            "boston",
            true
        ));
    }

    // --- path / envelope ----------------------------------------------------

    #[test]
    fn distance_cap_stays_inside_potato_fog_end() {
        let cap = attract_distance_cap(QualityTier::Potato);
        let fog_end = QualityTier::Potato.knobs().fog.unwrap().1;
        assert!(cap < fog_end);
        assert!((cap - fog_end * ATTRACT_ENVELOPE_FRACTION).abs() < 1e-3);
    }

    #[test]
    fn distance_cap_uses_building_draw_when_fog_absent() {
        let cap = attract_distance_cap(QualityTier::Medium);
        let draw = QualityTier::Medium
            .knobs()
            .building_draw_distance_m
            .unwrap();
        assert!((cap - draw * ATTRACT_ENVELOPE_FRACTION).abs() < 1e-3);
    }

    #[test]
    fn sampled_distance_never_exceeds_cap() {
        let cap = 500.0;
        let (_, _, _, d) = sample_attract_path(0.0, 0, Vec2::ZERO, cap, false);
        assert!(d <= cap);
        // Mid-crossfade should also respect the cap.
        let mid = ATTRACT_HOLD_SECS - ATTRACT_CROSSFADE_SECS * 0.5;
        let (_, _, _, d2) = sample_attract_path(mid, 0, Vec2::ZERO, cap, true);
        assert!(d2 <= cap);
    }

    #[test]
    fn crossfade_weight_is_zero_during_hold_and_one_at_end() {
        assert_eq!(crossfade_weight(0.0), 0.0);
        assert_eq!(
            crossfade_weight(ATTRACT_HOLD_SECS - ATTRACT_CROSSFADE_SECS),
            0.0
        );
        assert!((crossfade_weight(ATTRACT_HOLD_SECS) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn advance_phase_wraps_to_next_vantage_after_hold() {
        let (t, i) = advance_attract_phase(ATTRACT_HOLD_SECS - 0.1, 0, 0.2);
        assert_eq!(i, 1);
        assert!((t - 0.1).abs() < 1e-5);
    }

    #[test]
    fn sample_at_hold_start_matches_from_vantage() {
        let (target, yaw, pitch, distance) =
            sample_attract_path(0.0, 0, Vec2::new(10.0, 20.0), 10_000.0, false);
        let v = &ATTRACT_VANTAGES[0];
        assert!((target - (Vec2::new(10.0, 20.0) + v.target_offset)).length() < 1e-4);
        assert!((yaw - v.yaw).abs() < 1e-5);
        assert!((pitch - v.pitch).abs() < 1e-5);
        assert!((distance - v.distance).abs() < 1e-5);
    }

    #[test]
    fn sample_at_hold_end_matches_next_vantage() {
        let (target, yaw, pitch, distance) =
            sample_attract_path(ATTRACT_HOLD_SECS, 0, Vec2::ZERO, 10_000.0, false);
        let v = &ATTRACT_VANTAGES[1];
        assert!((target - v.target_offset).length() < 1e-3);
        assert!((yaw - v.yaw).abs() < 1e-4);
        assert!((pitch - v.pitch).abs() < 1e-4);
        assert!((distance - v.distance).abs() < 1e-3);
    }

    #[test]
    fn vantage_pitches_stay_oblique_not_top_down() {
        for v in &ATTRACT_VANTAGES {
            // ~0.22..0.40 rad keeps facades volumetric; 0.5+ reads as a map.
            assert!(
                (0.20..0.40).contains(&v.pitch),
                "pitch {} outside oblique band",
                v.pitch
            );
        }
    }

    // --- wrap-aware yaw easing ----------------------------------------------

    #[test]
    fn shortest_angle_delta_is_small_across_the_tau_wrap() {
        let tau = std::f32::consts::TAU;
        let delta = shortest_angle_delta(tau - 0.01, 0.02);
        assert!(delta > 0.0, "expected forward progress, got {delta}");
        assert!(
            delta < 0.1,
            "expected a small delta across the wrap, got {delta}"
        );
    }

    #[test]
    fn shortest_angle_delta_matches_plain_difference_away_from_the_wrap() {
        let delta = shortest_angle_delta(1.0, 1.5);
        assert!((delta - 0.5).abs() < 1e-6);
    }

    #[test]
    fn attract_smooth_yaw_advances_forward_across_a_wrap_without_snapping_back() {
        let tau = std::f32::consts::TAU;
        let value = tau - 0.01;
        let goal = 0.02;
        let next = attract_smooth_yaw(value, goal, ATTRACT_SMOOTH_RATE, 1.0 / 60.0);
        let forward_progress = if next < value {
            next + tau - value
        } else {
            next - value
        };
        assert!(
            (0.0..0.05).contains(&forward_progress),
            "expected small forward progress across the wrap, got raw {next} (progress {forward_progress})"
        );
    }

    #[test]
    fn attract_smooth_yaw_is_a_no_op_at_the_goal() {
        let v = attract_smooth_yaw(1.2, 1.2, ATTRACT_SMOOTH_RATE, 1.0 / 60.0);
        assert!((v - 1.2).abs() < 1e-6);
    }

    // --- generic smoothing ----------------------------------------------------

    #[test]
    fn attract_smooth_moves_partway_not_instantly() {
        let v = attract_smooth(0.0, 100.0, ATTRACT_SMOOTH_RATE, 1.0 / 60.0);
        assert!(v > 0.0 && v < 100.0);
    }

    #[test]
    fn attract_smooth_settles_toward_goal_over_many_steps() {
        let mut v = 0.0_f32;
        for _ in 0..600 {
            v = attract_smooth(v, 2200.0, ATTRACT_SMOOTH_RATE, 1.0 / 60.0);
        }
        assert!((v - 2200.0).abs() < 1.0);
    }

    // --- icon pixels ----------------------------------------------------------

    #[test]
    fn icon_buffer_has_the_right_length() {
        let buf = generate_icon_rgba(ICON_SIZE);
        assert_eq!(buf.len(), (ICON_SIZE as usize) * (ICON_SIZE as usize) * 4);
    }

    #[test]
    fn icon_corner_pixel_is_transparent() {
        assert_eq!(icon_pixel(0, 0, ICON_SIZE), ICON_TRANSPARENT);
        assert_eq!(icon_pixel(ICON_SIZE - 1, 0, ICON_SIZE), ICON_TRANSPARENT);
        assert_eq!(icon_pixel(0, ICON_SIZE - 1, ICON_SIZE), ICON_TRANSPARENT);
        assert_eq!(
            icon_pixel(ICON_SIZE - 1, ICON_SIZE - 1, ICON_SIZE),
            ICON_TRANSPARENT
        );
    }

    #[test]
    fn icon_center_pixel_is_the_accent_stripe() {
        assert_eq!(icon_pixel(32, 32, ICON_SIZE), ICON_ACCENT);
    }

    #[test]
    fn icon_top_edge_midpoint_is_border() {
        // Close to the top edge, away from any corner: border color.
        assert_eq!(icon_pixel(32, 2, ICON_SIZE), ICON_BORDER);
    }

    #[test]
    fn icon_interior_off_stripe_pixel_is_white() {
        // Away from the border and far from the diagonal stripe band.
        assert_eq!(icon_pixel(50, 14, ICON_SIZE), ICON_WHITE);
    }
}
