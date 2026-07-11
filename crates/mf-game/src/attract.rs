//! Main-menu "live diorama" (ship-plan #25, v0.4): while the player sits at
//! `AppState::MainMenu`, the real city (default preset, per `PendingInit`)
//! streams in from the sidecar and the camera slowly orbits over it behind
//! the menu, instead of the menu sitting over a static/empty scene.
//!
//! Two responsibilities live here, bundled per this wave's mission scope:
//!
//! 1. Kick off (and re-kick-off on a city change) an `init` for the
//!    MainMenu's preview city, and drive a slow cinematic camera orbit over
//!    it — [`AttractState`] + [`MfAttractPlugin`]'s `MainMenu`-gated systems.
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
//! `Transform` outside `InGame`. `camera.rs` is out of this wave's
//! ownership (see the mission brief), so rather than extending its
//! `run_if`s, this module carries its own miniature goal-chase +
//! transform-derivation for the `MainMenu` orbit only — mirroring the exact
//! precedent `reveal_input.rs` already set for `ease_strength` (a private
//! copy of `camera.rs`'s smoothing formula, "kept as its own copy here
//! since that one is private to `camera.rs`"). The two easers can never
//! fight: `camera.rs`'s versions only run `InGame`, this module's only run
//! `MainMenu`, and the states are mutually exclusive.
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
//!
//! ## Integration handoff
//!
//! `MfAttractPlugin` is NOT added to `main.rs`'s `.add_plugins(...)` tuple
//! yet — that tuple is a known hotspot several parallel v0.4 worktrees touch
//! this same wave (see `map_mode.rs`'s identical handoff note from v0.3),
//! and this wave's ownership was scoped to `main.rs`'s `mod attract;` line
//! only. Wiring `MfAttractPlugin` into the tuple is left for integration.
//! Every cross-module read here uses `Option<Res<_>>`/`Option<ResMut<_>>`
//! (mirroring `state.rs`'s own `Option<Res<SimLink>>` convention) precisely
//! so the rest of the app keeps compiling and running correctly even before
//! that wiring lands — nothing panics on a missing `AttractState`.
//!
//! `#![allow(dead_code)]` below covers the "never constructed"/"never used"
//! cascade this causes until that wiring lands — identical reasoning (and
//! precedent) to `map_mode.rs`'s own module doc for the same situation.
#![allow(dead_code)]

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy::winit::WinitWindows;
use mf_net::SimLink;
use mf_protocol::{InitPayload, SetSpeedPayload, ToSim};
use mf_render::BuildingsDenseCenter;
use mf_state::{CurrentCity, HeightAt};

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

/// Cinematic orbit yaw rate (radians/second). Slow by design — this is a
/// background diorama the player glances at while reading the menu, not
/// something they consciously track the way they would the RTS camera's own
/// orbit drag. A full revolution takes `TAU / ATTRACT_YAW_RATE` ≈ 209s.
const ATTRACT_YAW_RATE: f32 = 0.03;
/// Pitch goal: a steep elevated 3/4 view looking DOWN onto the city core.
/// Deliberately steeper than a shallow skyline-grazing angle: on the fog
/// tiers (Potato/Low) the building draw distance is short (3-6km) and the
/// terrain keeps going past it, so a shallow camera stares straight down a
/// long recession of raw un-built terrain and partial-fog road scribbles at
/// the horizon (the "paper map" the owner flagged). Pitching down keeps that
/// far horizon out of frame — the diorama reads as the city massing under
/// sky, and distance fog cleanly fades the frame edges — while still a clear
/// 3/4 view, not a flat top-down. (`verify.rs`'s `frame_elevated` uses a
/// shallower angle for in-game framing; chosen independently here.)
const ATTRACT_PITCH_GOAL: f32 = 0.82;
const ATTRACT_DISTANCE_GOAL: f32 = 1900.0;
/// Goal-chase settle rate for pitch/distance/target (see
/// `attract_smooth`/`attract_smooth_yaw`): deliberately gentler than
/// `camera.rs`'s own `ORBIT_SMOOTH_RATE`/`DOLLY_SMOOTH_RATE` (~150-250ms
/// settle) — a dreamy multi-second drift into the orbit framing reads as
/// "cinematic," where the RTS camera's snappy settle would read as a jarring
/// snap for a menu background nobody is actively steering.
const ATTRACT_SMOOTH_RATE: f32 = 2.0;

pub struct MfAttractPlugin;

impl Plugin for MfAttractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttractState>()
            .add_systems(OnEnter(AppState::MainMenu), attract_init_on_enter_system)
            .add_systems(
                Update,
                (attract_watch_preset_system, attract_orbit_system)
                    .run_if(in_state(AppState::MainMenu)),
            )
            .add_systems(Startup, set_window_icon_system);
    }
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
}

/// Advances a yaw goal by `dt * ATTRACT_YAW_RATE`, wrapped into `[0, TAU)`
/// rather than accumulating unbounded — an idle menu left open for a long
/// session must not let this grow into an `f32` precision problem. Pure so
/// the wrap behavior is directly unit-testable.
fn advance_yaw_goal(current_goal: f32, dt: f32) -> f32 {
    (current_goal + dt * ATTRACT_YAW_RATE).rem_euclid(std::f32::consts::TAU)
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
/// `(-PI, PI]`. Needed because the yaw goal wraps at `TAU` (see
/// `advance_yaw_goal`): naively easing `value` toward a goal that just
/// wrapped from ~`TAU` back to ~`0` would compute a huge NEGATIVE delta and
/// visibly spin the camera backward for a frame. Taking the shortest path
/// means the wrap is invisible — the eased value just keeps advancing
/// forward through it.
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

/// Drives the slow cinematic orbit while `MainMenu` is showing city data
/// (`CurrentCity.masks_complete()`): advances the yaw goal, points the
/// target at `mf_render`'s `BuildingsDenseCenter` (the interesting part of
/// the city, same reasoning `verify.rs` uses for its own framing), and eases
/// `CameraRig` + the actual `Transform` toward that framing every frame (see
/// module doc for why this module carries its own easer/transform-deriver
/// rather than relying on `camera.rs`'s `InGame`-only systems).
fn attract_orbit_system(
    time: Res<Time>,
    city: Res<CurrentCity>,
    dense_center: Res<BuildingsDenseCenter>,
    height_at: Res<HeightAt>,
    mut rigs: Query<(&mut CameraRig, &mut Transform)>,
) {
    if !city.masks_complete() {
        return;
    }
    let Ok((mut rig, mut transform)) = rigs.single_mut() else {
        return;
    };
    let dt = time.delta_secs();

    rig.yaw_goal = advance_yaw_goal(rig.yaw_goal, dt);
    rig.pitch_goal = ATTRACT_PITCH_GOAL;
    rig.distance_goal = ATTRACT_DISTANCE_GOAL;
    rig.target_goal = dense_center.0;

    rig.yaw = attract_smooth_yaw(rig.yaw, rig.yaw_goal, ATTRACT_SMOOTH_RATE, dt);
    rig.pitch = attract_smooth(rig.pitch, rig.pitch_goal, ATTRACT_SMOOTH_RATE, dt);
    rig.distance = attract_smooth(rig.distance, rig.distance_goal, ATTRACT_SMOOTH_RATE, dt);
    rig.target = attract_smooth_vec2(rig.target, rig.target_goal, ATTRACT_SMOOTH_RATE, dt);

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

    // --- yaw goal wrap -------------------------------------------------------

    #[test]
    fn yaw_goal_advances_by_rate_times_dt() {
        let next = advance_yaw_goal(1.0, 2.0);
        assert!((next - (1.0 + 2.0 * ATTRACT_YAW_RATE)).abs() < 1e-6);
    }

    #[test]
    fn yaw_goal_wraps_past_tau_back_into_zero_tau_range() {
        let tau = std::f32::consts::TAU;
        let next = advance_yaw_goal(tau - 0.01, 1.0); // + 0.03 rad crosses TAU
        assert!(
            (0.0..tau).contains(&next),
            "goal {next} not wrapped into [0, TAU)"
        );
        // And it's the expected small positive remainder, not some other
        // value entirely.
        assert!((next - (ATTRACT_YAW_RATE - 0.01)).abs() < 1e-5);
    }

    #[test]
    fn yaw_goal_never_grows_unbounded_over_many_steps() {
        let tau = std::f32::consts::TAU;
        let mut goal = 0.0_f32;
        for _ in 0..100_000 {
            goal = advance_yaw_goal(goal, 1.0 / 60.0);
            assert!((0.0..tau).contains(&goal));
        }
    }

    // --- wrap-aware yaw easing ----------------------------------------------

    #[test]
    fn shortest_angle_delta_is_small_across_the_tau_wrap() {
        let tau = std::f32::consts::TAU;
        // value just below TAU, goal just above 0 (having wrapped) — the
        // real angular distance is tiny (going forward), not almost a full
        // circle backward.
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
        let goal = 0.02; // goal has wrapped past TAU back near 0
        let next = attract_smooth_yaw(value, goal, ATTRACT_SMOOTH_RATE, 1.0 / 60.0);
        // Continuing forward past TAU (possibly wrapping itself) is fine;
        // jumping backward toward ~3.14 (halfway around) would indicate the
        // naive-difference bug this function exists to avoid.
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
