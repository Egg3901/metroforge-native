//! Top-down map mode (ship-plan #25, v0.3): `KeyM` toggles between the
//! normal RTS camera angle and a near-vertical, north-up overview.
//!
//! Keybind ownership: `M` is claimed HERE this wave (contention avoidance -
//! `input.rs` already owns Tab/Esc, `tools.rs` owns the build-tool hotkeys;
//! `M` was unclaimed by either, verified by grep before wiring this up).
//!
//! Drives the EXISTING `camera::CameraRig` (owned by `camera.rs`, spec
//! §3.4) rather than introducing a second camera path - this module only
//! reads `CameraRig`'s public fields and writes its `_goal` twins, never
//! `camera.rs` internals. Per `camera.rs`'s own module doc: every input
//! system except an active drag writes the `_goal` fields and lets
//! `camera_smoothing_system` ease the real value toward them each frame;
//! only `zoom_to_fit_on_enter` and the verify harness assign the raw
//! `target`/`yaw`/`pitch`/`distance` directly (screenshots can't wait out a
//! smoothing curve), and `camera_smoothing_system`'s external-write
//! detection (`advance_smoothing`, comparing against `RigLastOutput`) exists
//! specifically to snap the goal to match those direct writes without
//! fighting them. This module deliberately takes the FIRST path (goal-only
//! writes) both entering and exiting map mode, so the transition eases in
//! over the normal orbit-smoothing curve (`ORBIT_SMOOTH_RATE`/
//! `DOLLY_SMOOTH_RATE`) instead of snapping like a verify-harness frame
//! would - confirmed by reading `camera.rs` before wiring this, not
//! guessed: writing the raw fields here would just get treated as yet
//! another external write and re-smoothed from THAT value, which is not
//! what a deliberate "ease to map view" toggle should look like.
//!
//! `camera_input_system` (orbit/pan/dolly) still runs unmodified while map
//! mode is active. In particular, right-drag orbit writes `yaw`/`pitch`
//! directly as part of its "active drag" 1:1 path (by design, so a drag
//! never feels laggy) and would un-level the north-up/near-vertical framing
//! this module just eased into. ACCEPTABLE for v0.3 (explicitly not fixed
//! here, per mission scope): a player who orbits while map mode is active
//! simply drifts away from the intended framing rather than getting stuck
//! or crashing anything - deciding whether a future wave suppresses orbit
//! input while active, or treats any manual orbit as an implicit "exit map
//! mode", is left as a follow-up.
//!
//! Reveal (cursor-driven fog-of-war) needs no special handling while active:
//! it's already purely cursor-position-driven (`reveal_input.rs`, out of
//! this module's scope) and reads correctly from directly overhead the same
//! as from any other camera angle.
//!
//! INTEGRATION HANDOFF: `MfMapModePlugin` below is intentionally NOT added
//! to the `App` in `main.rs` yet. Mission scope for this wave keeps
//! `main.rs` untouched beyond its `mod map_mode;` declaration (that file's
//! `.add_plugins((...))` tuple is a known hotspot several parallel v0.3
//! worktrees touch this same wave) - wiring `MfMapModePlugin` into that
//! tuple is left for integration. `#![allow(dead_code)]` below covers the
//! resulting "never constructed" lint the same way `design_system.rs`
//! already does for its own forward-looking, not-yet-fully-consumed API -
//! see that file's module doc for the precedent.
#![allow(dead_code)]

use bevy::prelude::*;

use crate::camera::CameraRig;

/// Pitch (radians) map mode eases the camera toward. Deliberately
/// near-vertical rather than exactly `FRAC_PI_2` (straight down): a
/// perfectly overhead pitch flattens `camera_transform_system`'s horizontal
/// offset (`distance * pitch.cos()`) to zero, which is fine for the ribbon
/// geometry but leaves no sense of "forward" for a future north-indicator/
/// compass overlay to hang off of.
pub const MAP_MODE_PITCH: f32 = 1.52;
/// Yaw map mode eases toward. `0.0` is already north-up in
/// `camera_transform_system`'s convention (yaw=0 sits the camera on `+Z`
/// looking back toward `-Z`), so this is a plain reset to that existing
/// convention, not a new one.
pub const MAP_MODE_YAW: f32 = 0.0;
/// Floor on the distance map mode eases toward. A `max(current, ..)`, never
/// a hard set: this only ever pushes the camera OUT to at least an
/// overview-friendly range, it never zooms a player who's already further
/// out than this back in.
pub const MAP_MODE_MIN_DISTANCE: f32 = 2500.0;

/// Rig state captured the instant map mode is entered, restored on exit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SavedRig {
    target: Vec2,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

/// Map-mode on/off + the rig snapshot to restore on exit. `saved.is_some()`
/// IS the "active" flag (see [`MapModeState::is_active`]) rather than a
/// separate `bool` alongside it, so there is no representable state where
/// "active" and "have a rig to restore" disagree.
#[derive(Resource, Default)]
pub struct MapModeState {
    saved: Option<SavedRig>,
}

impl MapModeState {
    /// Whether map mode is currently active. Exposed for any future system
    /// (a HUD "MAP" badge, `reveal_input.rs`, etc.) that wants to react to
    /// the mode without reaching into this module's private fields.
    pub fn is_active(&self) -> bool {
        self.saved.is_some()
    }
}

/// Pure goal computation for ENTERING map mode, given the rig's current
/// (pre-toggle) distance. Split out from the system so the distance-floor
/// math is unit-testable without spinning up a Bevy `App`/`Query`, same
/// convention `camera.rs` uses for `apply_wheel_dolly`/`pan_release_step`.
/// Returns `(yaw_goal, pitch_goal, distance_goal)`.
fn enter_goals(current_distance: f32) -> (f32, f32, f32) {
    (
        MAP_MODE_YAW,
        MAP_MODE_PITCH,
        current_distance.max(MAP_MODE_MIN_DISTANCE),
    )
}

pub struct MfMapModePlugin;

impl Plugin for MfMapModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapModeState>().add_systems(
            Update,
            map_mode_toggle_system.run_if(in_state(crate::state::AppState::InGame)),
        );
    }
}

fn map_mode_toggle_system(
    keys: Res<ButtonInput<KeyCode>>,
    mut map_mode: ResMut<MapModeState>,
    mut rigs: Query<&mut CameraRig>,
) {
    if !keys.just_pressed(KeyCode::KeyM) {
        return;
    }
    let Ok(mut rig) = rigs.single_mut() else {
        return;
    };

    match map_mode.saved {
        None => {
            // Enter: snapshot the rig's CURRENT VALUES (not goals) - if a
            // wheel-dolly or WASD pan is mid-ease when `M` is pressed, this
            // restores to where the camera actually was, not to wherever it
            // was still easing toward.
            map_mode.saved = Some(SavedRig {
                target: rig.target,
                yaw: rig.yaw,
                pitch: rig.pitch,
                distance: rig.distance,
            });
            let (yaw_goal, pitch_goal, distance_goal) = enter_goals(rig.distance);
            rig.yaw_goal = yaw_goal;
            rig.pitch_goal = pitch_goal;
            rig.distance_goal = distance_goal;
        }
        Some(saved) => {
            // Exit: ease back to the saved rig via goals - same smooth path
            // as entering, see the module doc's `camera.rs` walkthrough.
            rig.target_goal = saved.target;
            rig.yaw_goal = saved.yaw;
            rig.pitch_goal = saved.pitch;
            rig.distance_goal = saved.distance;
            map_mode.saved = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_goals_raises_distance_to_floor_when_zoomed_in() {
        let (yaw, pitch, distance) = enter_goals(800.0);
        assert_eq!(yaw, MAP_MODE_YAW);
        assert_eq!(pitch, MAP_MODE_PITCH);
        assert_eq!(distance, MAP_MODE_MIN_DISTANCE);
    }

    #[test]
    fn enter_goals_keeps_distance_when_already_zoomed_out_further() {
        // `max(current, floor)`, never a hard set - a player already
        // further out than the floor must not get pulled in.
        let (_, _, distance) = enter_goals(9_000.0);
        assert_eq!(distance, 9_000.0);
    }

    #[test]
    fn enter_goals_at_exactly_the_floor_is_a_no_op() {
        let (_, _, distance) = enter_goals(MAP_MODE_MIN_DISTANCE);
        assert_eq!(distance, MAP_MODE_MIN_DISTANCE);
    }

    #[test]
    fn map_mode_state_defaults_to_inactive() {
        assert!(!MapModeState::default().is_active());
    }
}
