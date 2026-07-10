//! Cursor / close-camera "reveal" driver (issue #18): computes the
//! world-space hole `mf-render`'s building material should dissolve toward,
//! so players can see streets under dense building fabric. Mirrors the
//! `HeightAt`/`SubwayView` split already used across these crates:
//! `mf-state` holds the shared `RevealState` value, this module (which has
//! the cursor ray, `CameraRig`, and `SubwayView` all available) computes it
//! every frame, and `mf-render`'s `buildings.rs::apply_reveal_system` copies
//! it into the shared building material's shader uniform.

use bevy::prelude::*;
use mf_state::{HeightAt, RevealState, SubwayView};

use crate::camera::{screen_to_ground, CameraRig};

pub struct MfRevealInputPlugin;

impl Plugin for MfRevealInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            reveal_input_system.run_if(in_state(crate::state::AppState::InGame)),
        );
    }
}

/// Cursor-reveal hole radii (meters): tight enough to feel like "peering
/// around the mouse", not a general fog-of-war over the whole city.
const CURSOR_INNER_M: f32 = 60.0;
const CURSOR_OUTER_M: f32 = 180.0;

/// Below this `CameraRig::distance`, the "zoomed close" trigger widens the
/// cursor hole (see `widen_for_camera_distance`). Comfortably inside
/// `camera::MAX_DOLLY` (20,000) and well above `camera::MIN_DOLLY` (120), so
/// this only engages once the player has actually dollied in, not at the
/// default `zoom_to_fit` distance.
const CAMERA_CLOSE_DISTANCE_M: f32 = 500.0;
/// Owner's "camera reveal" radii formula (`distance * factor`), folded into
/// the single cursor-centered hole below rather than tracked as a second,
/// independent hole (see `widen_for_camera_distance`'s doc for why).
const CAMERA_INNER_FACTOR: f32 = 0.25;
const CAMERA_OUTER_FACTOR: f32 = 0.6;

/// How fast `RevealState.strength` eases toward its target (exponential
/// smoothing, same technique as `camera.rs`'s `smooth_toward`): rate 10.0
/// settles ~95% of the way in `3.0 / 10.0` = 300ms — slow enough the hole
/// visibly grows/heals instead of popping, fast enough it doesn't lag a
/// quick mouse-out.
const STRENGTH_EASE_RATE: f32 = 10.0;

/// Above this `SubwayView.t`, the reveal fully backs off (see
/// `target_strength`): subway view already squashes/hides building chunks
/// its own way (art-direction §7), so stacking the two effects would just
/// read as flicker.
const SUBWAY_T_GATE: f32 = 0.3;

fn reveal_input_system(
    time: Res<Time>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform, &CameraRig)>,
    height_at: Res<HeightAt>,
    subway: Res<SubwayView>,
    mut reveal: ResMut<RevealState>,
) {
    let Ok((camera, camera_transform, rig)) = cameras.single() else {
        return;
    };
    let dt = time.delta_secs();

    // Harness determinism: under MF_VERIFY_DIR the X server still reports a
    // default pointer position, which would put a reveal hole wherever that
    // stray pointer's ray lands and make screenshots non-comparable between
    // runs. Verify runs therefore ignore the real cursor entirely; setting
    // MF_FORCE_REVEAL pins the hole to the camera target instead so the
    // shader path still gets screenshot coverage.
    let in_verify = std::env::var_os("MF_VERIFY_DIR").is_some();
    let forced = std::env::var_os("MF_FORCE_REVEAL").map(|_| rig.target);
    let cursor_ground = if in_verify {
        forced
    } else {
        windows
            .single()
            .ok()
            .and_then(Window::cursor_position)
            .and_then(|pos| screen_to_ground(camera, camera_transform, &height_at, pos))
            .or(forced)
    };

    // (b) Cursor reveal is always the hole's center; when the camera has
    // also dollied in close, widen ITS radii by the camera-distance factor
    // rather than tracking a second, independent camera-target hole — in
    // practice "zoomed in" and "mousing over the buildings you zoomed in on"
    // are the same moment, so a second hole would just double-draw over the
    // same patch of city. This is the simpler of the two blend strategies
    // the spec offered, chosen for that reason.
    let (center, inner, outer, has_cursor) = match cursor_ground {
        Some(ground) => {
            let (inner, outer) =
                widen_for_camera_distance(CURSOR_INNER_M, CURSOR_OUTER_M, rig.distance);
            (ground, inner, outer, true)
        }
        // (d) No cursor position (window unfocused / cursor left the
        // window): keep the last center so the hole doesn't jump, but drive
        // strength toward 0 below so it eases shut instead of freezing open.
        None => (reveal.center, reveal.inner, reveal.outer, false),
    };

    let target = target_strength(has_cursor, subway.active, subway.t);
    reveal.center = center;
    reveal.inner = inner;
    reveal.outer = outer;
    reveal.strength = ease_strength(reveal.strength, target, STRENGTH_EASE_RATE, dt);
}

/// Widens the cursor-reveal radii when the camera has dollied inside
/// `CAMERA_CLOSE_DISTANCE_M`, taking whichever of the base cursor radius or
/// the distance-scaled camera radius is larger per axis — this is exactly
/// "use whichever gives the larger effect" and "widen when close" folded
/// into one component-wise `max`, since a camera radius smaller than the
/// base cursor radius should never shrink the hole. Pure function (no ECS
/// params) so the scaling curve is unit-testable without a Bevy `App`.
fn widen_for_camera_distance(base_inner: f32, base_outer: f32, distance: f32) -> (f32, f32) {
    if distance >= CAMERA_CLOSE_DISTANCE_M {
        return (base_inner, base_outer);
    }
    let camera_inner = distance * CAMERA_INNER_FACTOR;
    let camera_outer = distance * CAMERA_OUTER_FACTOR;
    (base_inner.max(camera_inner), base_outer.max(camera_outer))
}

/// (c) + (d): the reveal's target strength. 1.0 only when there IS a cursor
/// position AND subway view isn't (becoming) active; 0.0 otherwise —
/// covers both "no cursor" (d) and "subway active or transitioning past
/// `SUBWAY_T_GATE`" (c) with the same single knob, since both cases mean
/// "the effect should not be visible right now". `subway_active` (not just
/// `subway_t > SUBWAY_T_GATE`) is checked so a just-pressed Tab starts
/// backing the reveal off immediately, not only once `t` has eased past the
/// gate.
fn target_strength(has_cursor: bool, subway_active: bool, subway_t: f32) -> f32 {
    let subway_gate = subway_active || subway_t > SUBWAY_T_GATE;
    if has_cursor && !subway_gate {
        1.0
    } else {
        0.0
    }
}

/// Frame-rate-independent exponential smoothing toward `target`, identical
/// formula to `camera.rs`'s `smooth_toward` (kept as its own copy here since
/// that one is private to `camera.rs` and this is the only other place in
/// `mf-game` that needs it).
fn ease_strength(value: f32, target: f32, rate: f32, dt: f32) -> f32 {
    value + (target - value) * (1.0 - (-rate * dt).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_distance_does_not_widen_past_the_close_threshold() {
        let (inner, outer) =
            widen_for_camera_distance(CURSOR_INNER_M, CURSOR_OUTER_M, CAMERA_CLOSE_DISTANCE_M);
        assert_eq!((inner, outer), (CURSOR_INNER_M, CURSOR_OUTER_M));
        let (inner, outer) = widen_for_camera_distance(CURSOR_INNER_M, CURSOR_OUTER_M, 10_000.0);
        assert_eq!((inner, outer), (CURSOR_INNER_M, CURSOR_OUTER_M));
    }

    #[test]
    fn camera_distance_widens_radii_once_close_enough_to_exceed_base() {
        // At distance 400 (< the 500 gate): camera_inner = 100, camera_outer
        // = 240 — both bigger than the base cursor radii, so both widen.
        let (inner, outer) = widen_for_camera_distance(CURSOR_INNER_M, CURSOR_OUTER_M, 400.0);
        assert!((inner - 100.0).abs() < 0.001, "inner = {inner}");
        assert!((outer - 240.0).abs() < 0.001, "outer = {outer}");
    }

    #[test]
    fn camera_distance_never_shrinks_below_the_base_cursor_radius() {
        // At a very close distance (120, camera::MIN_DOLLY) the raw
        // camera-scaled radii (30 / 72) are SMALLER than the base cursor
        // radii (60 / 180) — the hole must not shrink just because the
        // camera is extremely close.
        let (inner, outer) = widen_for_camera_distance(CURSOR_INNER_M, CURSOR_OUTER_M, 120.0);
        assert_eq!((inner, outer), (CURSOR_INNER_M, CURSOR_OUTER_M));
    }

    #[test]
    fn target_strength_is_zero_without_a_cursor() {
        // (d): no cursor position must ease toward 0 regardless of subway
        // state.
        assert_eq!(target_strength(false, false, 0.0), 0.0);
    }

    #[test]
    fn target_strength_is_one_with_cursor_and_no_subway() {
        assert_eq!(target_strength(true, false, 0.0), 1.0);
    }

    #[test]
    fn target_strength_gates_on_subway_active_immediately() {
        // `active` flips the instant Tab is pressed, before `t` has eased
        // anywhere — the reveal must back off that same frame, not wait for
        // `t` to cross SUBWAY_T_GATE.
        assert_eq!(target_strength(true, true, 0.0), 0.0);
    }

    #[test]
    fn target_strength_gates_on_subway_t_past_threshold() {
        assert_eq!(target_strength(true, false, 0.31), 0.0);
        assert_eq!(target_strength(true, false, 0.29), 1.0);
    }

    #[test]
    fn ease_strength_settles_95_percent_within_300ms() {
        let value = ease_strength(0.0, 1.0, STRENGTH_EASE_RATE, 0.3);
        assert!(value >= 0.95, "expected >=95% settle at 300ms, got {value}");
    }

    #[test]
    fn ease_strength_reaches_zero_without_a_cursor_over_several_frames() {
        // Simulates the mouse leaving the window: strength must monotonically
        // decay to (effectively) zero, never jump or reverse direction.
        let mut strength = 1.0_f32;
        let dt = 1.0 / 60.0;
        let mut previous = strength;
        for _ in 0..120 {
            strength = ease_strength(strength, 0.0, STRENGTH_EASE_RATE, dt);
            assert!(
                strength <= previous,
                "strength went up: {previous} -> {strength}"
            );
            previous = strength;
        }
        assert!(
            strength < 0.001,
            "expected strength to have decayed to ~0, got {strength}"
        );
    }

    #[test]
    fn ease_strength_is_a_no_op_at_the_goal() {
        assert_eq!(ease_strength(1.0, 1.0, STRENGTH_EASE_RATE, 1.0 / 60.0), 1.0);
        assert_eq!(ease_strength(0.0, 0.0, STRENGTH_EASE_RATE, 1.0 / 60.0), 0.0);
    }
}
